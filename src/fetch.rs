//! Fetching remote lists over HTTP(S).
//!
//! One shared client with a timeout, a streamed size cap and a redirect
//! policy. A refresh sends the validators from the last successful response,
//! and a `304 Not Modified` keeps the list's rules without reparsing them.
//! Any failure leaves the list's last good rules in place.

use std::{fmt, sync::Arc, time::Duration, time::SystemTime};

use reqwest::{Client, StatusCode, Url, dns::Resolve, header, redirect};

use crate::{
    bootstrap::BootstrapResolver,
    config::Config,
    lists::{DEFAULT_MAX_LIST_BYTES, ListSet, ListState, Source, Validators},
};

/// The default timeout for fetching one list.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_REDIRECTS: usize = 10;

/// May a redirect from `original` lead to `next`? Only to `https`, or to
/// `http` when the original URL was `http` too: a list fetched over TLS is
/// never silently downgraded.
pub fn redirect_allowed(original: &Url, next: &Url) -> bool {
    match next.scheme() {
        "https" => true,
        "http" => original.scheme() == "http",
        _ => false,
    }
}

/// Why a fetch failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    Timeout,
    /// The server answered with a non-success status (other than 304).
    Status(u16),
    /// The body is, or grew, over the size limit.
    TooLarge {
        limit: u64,
    },
    /// Anything else: connection failure, bad redirect, malformed response.
    Request(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("timed out"),
            Self::Status(code) => write!(f, "HTTP status {code}"),
            Self::TooLarge { limit } => write!(f, "larger than the {limit}-byte limit"),
            Self::Request(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for FetchError {}

impl FetchError {
    fn from_reqwest(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::Timeout;
        }
        // The cause chain carries the useful part (refused, reset, policy).
        let mut reason = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            reason.push_str(": ");
            reason.push_str(&cause.to_string());
            source = cause.source();
        }
        Self::Request(reason)
    }
}

/// What a successful fetch produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Fetched {
        body: String,
        validators: Validators,
    },
    NotModified,
}

/// A client for fetching lists.
pub struct Fetcher {
    client: Client,
    max_bytes: u64,
}

impl Fetcher {
    /// A fetcher that resolves list hosts with the system resolver.
    pub fn new(timeout: Duration, max_bytes: u64) -> Result<Self, FetchError> {
        Self::build(timeout, max_bytes, None)
    }

    /// A fetcher with the limits from the defaults, resolving list hosts only
    /// through the configured bootstrap resolver, or through the system
    /// resolver when none is configured.
    pub fn from_config(config: &Config) -> Result<Self, FetchError> {
        let resolver: Option<Arc<dyn Resolve>> = match &config.filter.bootstrap {
            Some(bootstrap) => {
                let resolver = BootstrapResolver::new(bootstrap)
                    .map_err(|e| FetchError::Request(e.to_string()))?;
                Some(Arc::new(resolver))
            }
            None => None,
        };
        Self::build(DEFAULT_TIMEOUT, DEFAULT_MAX_LIST_BYTES, resolver)
    }

    /// A fetcher that resolves list hosts only through `resolver`.
    pub fn with_resolver(
        timeout: Duration,
        max_bytes: u64,
        resolver: Arc<dyn Resolve>,
    ) -> Result<Self, FetchError> {
        Self::build(timeout, max_bytes, Some(resolver))
    }

    fn build(
        timeout: Duration,
        max_bytes: u64,
        resolver: Option<Arc<dyn Resolve>>,
    ) -> Result<Self, FetchError> {
        let policy = redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let allowed = attempt
                .previous()
                .first()
                .is_some_and(|original| redirect_allowed(original, attempt.url()));
            if allowed {
                attempt.follow()
            } else {
                let target = attempt.url().to_string();
                attempt.error(format!("redirect to {target} is not allowed"))
            }
        });
        let mut builder = Client::builder()
            .timeout(timeout)
            .redirect(policy)
            .user_agent(concat!("hickory-filter/", env!("CARGO_PKG_VERSION")));
        if let Some(resolver) = resolver {
            builder = builder.dns_resolver(resolver);
        }
        let client = builder.build().map_err(|e| FetchError::from_reqwest(&e))?;
        Ok(Self { client, max_bytes })
    }

    /// GET `url`, sending the validators of the last successful fetch. The
    /// body is read in chunks and abandoned as soon as it passes the limit.
    pub async fn fetch(
        &self,
        url: &str,
        validators: &Validators,
    ) -> Result<FetchOutcome, FetchError> {
        let mut request = self.client.get(url);
        if let Some(etag) = &validators.etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &validators.last_modified {
            request = request.header(header::IF_MODIFIED_SINCE, modified);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?;

        match response.status() {
            StatusCode::NOT_MODIFIED => return Ok(FetchOutcome::NotModified),
            status if status.is_success() => {}
            status => return Err(FetchError::Status(status.as_u16())),
        }
        let too_large = FetchError::TooLarge {
            limit: self.max_bytes,
        };
        if response
            .content_length()
            .is_some_and(|len| len > self.max_bytes)
        {
            return Err(too_large);
        }
        let header_text = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let validators = Validators {
            etag: header_text(header::ETAG),
            last_modified: header_text(header::LAST_MODIFIED),
        };

        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::from_reqwest(&e))?
        {
            if body.len() as u64 + chunk.len() as u64 > self.max_bytes {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchOutcome::Fetched {
            body: String::from_utf8_lossy(&body).into_owned(),
            validators,
        })
    }
}

/// Fetch one remote list and record the result on its state. A failure keeps
/// the list's last good rules. Does nothing for a `file://` list.
pub async fn refresh_remote(fetcher: &Fetcher, list: &mut ListState, now: SystemTime) {
    let Source::Remote(url) = &list.source else {
        return;
    };
    let url = url.clone();
    match fetcher.fetch(&url, &list.validators).await {
        Ok(FetchOutcome::Fetched { body, validators }) => list.apply_load(&body, validators, now),
        Ok(FetchOutcome::NotModified) if list.has_loaded() => list.record_not_modified(now),
        Ok(FetchOutcome::NotModified) => list.record_failure(format!(
            "GET {url}: 304 Not Modified, but there is no previous copy"
        )),
        Err(error) => list.record_failure(format!("GET {url}: {error}")),
    }
}

/// Fetch every remote list, one after another. The refresher bounds the
/// concurrency; startup uses this.
pub async fn load_remote(fetcher: &Fetcher, set: &mut ListSet, now: SystemTime) {
    for list in set.lists_mut() {
        refresh_remote(fetcher, list, now).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn redirects_go_to_https_or_stay_on_http() {
        let http = url("http://lists.example/a.txt");
        let https = url("https://lists.example/a.txt");
        // From https: only to https.
        assert!(redirect_allowed(&https, &url("https://cdn.example/a.txt")));
        assert!(!redirect_allowed(&https, &url("http://cdn.example/a.txt")));
        // From http: to http or https.
        assert!(redirect_allowed(&http, &url("http://cdn.example/a.txt")));
        assert!(redirect_allowed(&http, &url("https://cdn.example/a.txt")));
        // Never to another scheme.
        for scheme in [
            "ftp://cdn.example/a",
            "file:///etc/passwd",
            "data:text/plain,x",
        ] {
            assert!(!redirect_allowed(&http, &url(scheme)), "{scheme}");
            assert!(!redirect_allowed(&https, &url(scheme)), "{scheme}");
        }
    }

    #[test]
    fn errors_say_what_went_wrong() {
        assert_eq!(FetchError::Timeout.to_string(), "timed out");
        assert_eq!(FetchError::Status(503).to_string(), "HTTP status 503");
        assert!(
            FetchError::TooLarge { limit: 10 }
                .to_string()
                .contains("10-byte limit")
        );
    }
}
