//! The optional on-disk list cache.
//!
//! With a cache directory configured, every successfully fetched remote list
//! is stored there with its validators, one file per list. On startup a list
//! that can't be fetched is served from its cached copy, so a restart with the
//! list server down still comes up with the rules it had. Local `file://`
//! lists are never cached, and without a cache directory nothing is written.
//!
//! A file holds a small header (format, URL, validators) and then the body, and
//! is written to a temporary file in the same directory and renamed into
//! place, so a reader never sees a half-written copy and a crash leaves the
//! previous one intact.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    fetch::FetchOutcome,
    lists::{Source, Validators},
    refresh::ListLoader,
};

const MAGIC: &str = "hickory-filter list cache 1";

/// A list read back from the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedList {
    pub body: String,
    pub validators: Validators,
}

/// FNV-1a: a stable hash for naming cache files, the same on every run.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The cache file for `url`: a fixed-width hash, so any URL makes a safe name.
pub fn file_name(url: &str) -> String {
    format!("{:016x}.cache", fnv1a(url.as_bytes()))
}

fn encode(url: &str, body: &str, validators: &Validators) -> String {
    let mut text = format!("{MAGIC}\nurl: {url}\n");
    if let Some(etag) = &validators.etag {
        text.push_str(&format!("etag: {etag}\n"));
    }
    if let Some(modified) = &validators.last_modified {
        text.push_str(&format!("last-modified: {modified}\n"));
    }
    text.push('\n');
    text.push_str(body);
    text
}

/// Parse a cache file. `None` unless it has the right format and was written
/// for exactly `url` (a hash collision, or a corrupt or foreign file, is not a
/// cached copy).
fn decode(url: &str, text: &str) -> Option<CachedList> {
    let (header, body) = text.split_once("\n\n")?;
    let mut lines = header.lines();
    if lines.next()? != MAGIC || lines.next()?.strip_prefix("url: ")? != url {
        return None;
    }
    let mut validators = Validators::default();
    for line in lines {
        if let Some(etag) = line.strip_prefix("etag: ") {
            validators.etag = Some(etag.to_owned());
        } else if let Some(modified) = line.strip_prefix("last-modified: ") {
            validators.last_modified = Some(modified.to_owned());
        }
    }
    Some(CachedList {
        body: body.to_owned(),
        validators,
    })
}

/// The cached copy of `url` in `dir`, if there is a valid one.
pub fn read(dir: &Path, url: &str) -> Option<CachedList> {
    let text = fs::read_to_string(dir.join(file_name(url))).ok()?;
    decode(url, &text)
}

/// Store `body` and `validators` as the cached copy of `url`, atomically.
pub fn write(dir: &Path, url: &str, body: &str, validators: &Validators) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let name = file_name(url);
    let temporary = dir.join(format!("{name}.tmp{}", std::process::id()));
    let result = fs::write(&temporary, encode(url, body, validators))
        .and_then(|()| fs::rename(&temporary, dir.join(&name)));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Wraps a loader with the cache. Remote lists are stored after every
/// successful fetch; the first load of each list in this process also sends the
/// cached validators, and falls back to the cached copy when the fetch fails.
pub struct CachedLoader<L> {
    inner: L,
    dir: PathBuf,
    /// Lists already loaded once in this process: the startup fallback only
    /// applies to the first load.
    seen: Mutex<HashSet<String>>,
    last_write_error: Mutex<Option<String>>,
}

impl<L: ListLoader> CachedLoader<L> {
    pub fn new(inner: L, dir: PathBuf) -> Self {
        Self {
            inner,
            dir,
            seen: Mutex::new(HashSet::new()),
            last_write_error: Mutex::new(None),
        }
    }

    /// Why the last attempt to write the cache failed, if it did. A failed
    /// write never fails the load.
    pub fn last_write_error(&self) -> Option<String> {
        self.last_write_error.lock().expect("lock").clone()
    }

    async fn read_cached(&self, url: &str) -> Option<CachedList> {
        let (dir, url) = (self.dir.clone(), url.to_owned());
        tokio::task::spawn_blocking(move || read(&dir, &url))
            .await
            .ok()
            .flatten()
    }

    async fn store(&self, url: &str, body: &str, validators: &Validators) {
        let (dir, url) = (self.dir.clone(), url.to_owned());
        let (body, validators) = (body.to_owned(), validators.clone());
        let written = tokio::task::spawn_blocking(move || write(&dir, &url, &body, &validators))
            .await
            .map_err(|e| io::Error::other(e.to_string()))
            .and_then(|result| result);
        *self.last_write_error.lock().expect("lock") = written
            .err()
            .map(|e| format!("cannot write the list cache in {}: {e}", self.dir.display()));
    }
}

impl<L: ListLoader> ListLoader for CachedLoader<L> {
    async fn load(&self, source: &Source, validators: &Validators) -> Result<FetchOutcome, String> {
        let Source::Remote(url) = source else {
            return self.inner.load(source, validators).await;
        };
        let first = self.seen.lock().expect("lock").insert(url.clone());
        let cached = if first {
            self.read_cached(url).await
        } else {
            None
        };
        // On the first load, ask the server whether the cached copy is current.
        let send = match &cached {
            Some(cached) => &cached.validators,
            None => validators,
        };
        match (self.inner.load(source, send).await, cached) {
            (Ok(FetchOutcome::Fetched { body, validators }), _) => {
                self.store(url, &body, &validators).await;
                Ok(FetchOutcome::Fetched { body, validators })
            }
            // Unchanged since the cached copy: the cache is the content.
            (Ok(FetchOutcome::NotModified), Some(cached)) | (Err(_), Some(cached)) => {
                Ok(FetchOutcome::Fetched {
                    body: cached.body,
                    validators: cached.validators,
                })
            }
            (Ok(FetchOutcome::NotModified), None) => Ok(FetchOutcome::NotModified),
            (Err(reason), None) => Err(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validators(etag: Option<&str>, modified: Option<&str>) -> Validators {
        Validators {
            etag: etag.map(str::to_owned),
            last_modified: modified.map(str::to_owned),
        }
    }

    #[test]
    fn a_cached_list_round_trips_with_its_validators() {
        let url = "https://lists.example/a.txt";
        let v = validators(Some("\"v1\""), Some("Wed, 21 Oct 2026 07:28:00 GMT"));
        let text = encode(url, "ads.example\n\ntrack.example\n", &v);
        assert_eq!(
            decode(url, &text),
            Some(CachedList {
                body: "ads.example\n\ntrack.example\n".to_owned(),
                validators: v
            })
        );
    }

    #[test]
    fn validators_are_optional() {
        let url = "https://lists.example/a.txt";
        let text = encode(url, "ads.example\n", &Validators::default());
        let cached = decode(url, &text).expect("decodes");
        assert_eq!(cached.validators, Validators::default());
        assert_eq!(cached.body, "ads.example\n");
    }

    #[test]
    fn a_file_for_another_url_or_a_corrupt_file_is_not_a_cached_copy() {
        let text = encode("https://lists.example/a.txt", "x\n", &Validators::default());
        assert_eq!(decode("https://lists.example/b.txt", &text), None);
        for bad in [
            "",
            "garbage",
            "hickory-filter list cache 1\n",
            "wrong magic\nurl: u\n\nbody",
        ] {
            assert_eq!(decode("u", bad), None, "{bad:?}");
        }
    }

    #[test]
    fn file_names_are_stable_distinct_and_safe() {
        let a = file_name("https://lists.example/a.txt");
        assert_eq!(a, file_name("https://lists.example/a.txt"));
        assert_ne!(a, file_name("https://lists.example/b.txt"));
        assert_eq!(a.len(), 16 + ".cache".len());
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'.' || b.is_ascii_lowercase())
        );
        // The hash is fixed, so cache files survive upgrades and restarts.
        assert_eq!(file_name(""), "cbf29ce484222325.cache");
    }

    #[test]
    fn writing_replaces_the_copy_atomically_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let url = "https://lists.example/a.txt";
        write(dir.path(), url, "old.example\n", &Validators::default()).expect("write");
        write(
            dir.path(),
            url,
            "new.example\n",
            &validators(Some("\"v2\""), None),
        )
        .expect("write");

        let cached = read(dir.path(), url).expect("read");
        assert_eq!(cached.body, "new.example\n");
        assert_eq!(cached.validators.etag.as_deref(), Some("\"v2\""));
        let names: Vec<String> = fs::read_dir(dir.path())
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![file_name(url)],
            "only the cache file: {names:?}"
        );
    }

    #[test]
    fn the_cache_directory_is_created_on_the_first_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        assert!(!nested.exists());
        write(
            &nested,
            "https://lists.example/a.txt",
            "x\n",
            &Validators::default(),
        )
        .expect("write");
        assert!(nested.is_dir());
    }

    #[test]
    fn reading_a_missing_cache_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(dir.path(), "https://lists.example/a.txt"), None);
        assert_eq!(
            read(&dir.path().join("nope"), "https://lists.example/a.txt"),
            None
        );
    }
}
