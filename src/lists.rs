//! Rule lists: where they come from and what the server remembers about them.
//!
//! Each list keeps its own last good content in a `ListState`, so a list that
//! fails to load or refresh never loses the rules it already has, and one
//! failing list never affects another. A `ListSet` holds every configured list
//! and hands all their rules to the rule-set builder.
//!
//! This module loads `file://` lists. Remote lists are fetched by the HTTP
//! fetcher, which records its results through the same `ListState` methods.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use crate::{
    config::{Config, Interval},
    rules::{ListDecl, ParsedList, Rule, RuleError, RuleLine, parse_list, parse_rule_line},
};

/// The outcome of loading one list, ready to apply to its state.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadResult {
    /// New content, already parsed.
    Loaded {
        parsed: ParsedList,
        validators: Validators,
    },
    NotModified,
    Failed(String),
}

/// The default maximum size of one list, in bytes (64 MiB).
pub const DEFAULT_MAX_LIST_BYTES: u64 = 64 * 1024 * 1024;

/// Where a list's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    /// An `http://` or `https://` URL.
    Remote(String),
}

impl Source {
    /// The source a list declaration's URL names.
    pub fn from_url(url: &str) -> Result<Self, String> {
        match url.split_once("://") {
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("file") => {
                file_path(url).map(Self::File)
            }
            Some((scheme, _))
                if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
            {
                Ok(Self::Remote(url.to_owned()))
            }
            _ => Err(format!("`{url}` is not a file, http or https URL")),
        }
    }
}

/// The path in a `file:///absolute/path` URL. `file://localhost/path` is
/// accepted too; `%XX` escapes are decoded.
pub fn file_path(url: &str) -> Result<PathBuf, String> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| format!("`{url}` is not a file URL"))?;
    let path = match rest.strip_prefix("localhost") {
        Some(path) if path.starts_with('/') => path,
        _ => rest,
    };
    if !path.starts_with('/') {
        return Err(format!(
            "`{url}` must give an absolute path, as in `file:///etc/hickory-filter/list.txt`"
        ));
    }
    Ok(PathBuf::from(percent_decode(path)?))
}

fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("`{text}` has an invalid %-escape"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("`{text}` is not valid UTF-8 after decoding"))
}

/// The validators from the last successful fetch of a remote list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// How the last attempt to load or refresh a list went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastResult {
    NotAttempted,
    Loaded,
    NotModified,
    Failed(String),
}

/// One list and everything remembered about it.
#[derive(Debug, Clone)]
pub struct ListState {
    pub decl: ListDecl,
    pub source: Source,
    /// When true, startup fails if the list has never loaded.
    pub required: bool,
    /// How often the list is refreshed; `0` disables refresh.
    pub refresh: Interval,
    /// The rules of the last successful load. Kept when a refresh fails.
    /// Shared, so a rule-set rebuild can take a cheap snapshot of every list.
    pub rules: Arc<Vec<Rule>>,
    /// Lines skipped as invalid in the last successful load.
    pub skipped: usize,
    pub validators: Validators,
    pub last_success: Option<SystemTime>,
    pub last_result: LastResult,
}

impl ListState {
    pub fn new(decl: ListDecl, required: bool, refresh: Interval) -> Result<Self, String> {
        let source = Source::from_url(&decl.url)?;
        Ok(Self {
            decl,
            source,
            required,
            refresh,
            rules: Arc::new(Vec::new()),
            skipped: 0,
            validators: Validators::default(),
            last_success: None,
            last_result: LastResult::NotAttempted,
        })
    }

    /// Has this list ever loaded successfully?
    pub fn has_loaded(&self) -> bool {
        self.last_success.is_some()
    }

    /// Replace the list's rules with the parsed `text`: its new last good
    /// content. Invalid lines are skipped and counted, never failing the list.
    pub fn apply_load(&mut self, text: &str, validators: Validators, now: SystemTime) {
        let parsed = parse_list(self.decl.list_type, self.decl.action, text);
        self.rules = Arc::new(parsed.rules);
        self.skipped = parsed.skipped;
        self.validators = validators;
        self.last_success = Some(now);
        self.last_result = LastResult::Loaded;
    }

    /// Record the result of a load or refresh made off the owning task. Returns
    /// true when the list's rules changed, so the caller knows a new rule set
    /// is needed. A `304` with no previous copy is a failure.
    pub fn apply(&mut self, result: LoadResult, now: SystemTime) -> bool {
        match result {
            LoadResult::Loaded { parsed, validators } => {
                let changed = !self.has_loaded() || *self.rules != parsed.rules;
                self.rules = Arc::new(parsed.rules);
                self.skipped = parsed.skipped;
                self.validators = validators;
                self.last_success = Some(now);
                self.last_result = LastResult::Loaded;
                changed
            }
            LoadResult::NotModified => {
                if self.has_loaded() {
                    self.record_not_modified(now);
                } else {
                    self.record_failure(
                        "304 Not Modified, but there is no previous copy".to_owned(),
                    );
                }
                false
            }
            LoadResult::Failed(reason) => {
                self.record_failure(reason);
                false
            }
        }
    }

    /// The server said the content is unchanged: keep the rules, count the
    /// refresh as a success.
    pub fn record_not_modified(&mut self, now: SystemTime) {
        self.last_success = Some(now);
        self.last_result = LastResult::NotModified;
    }

    /// A load or refresh failed: keep serving the last good rules and remember
    /// why.
    pub fn record_failure(&mut self, reason: String) {
        self.last_result = LastResult::Failed(reason);
    }

    /// Load a `file://` list from disk, with the same bookkeeping as any other
    /// load. Does nothing for a remote list.
    pub fn load_file(&mut self, max_bytes: u64, now: SystemTime) {
        let Source::File(path) = &self.source else {
            return;
        };
        match read_list_file(path, max_bytes) {
            Ok(text) => self.apply_load(&text, Validators::default(), now),
            Err(reason) => self.record_failure(reason),
        }
    }
}

/// Read a list file. A list over `max_bytes` fails; bytes that aren't valid
/// UTF-8 become replacement characters, so their lines are skipped later
/// instead of failing the list.
pub fn read_list_file(path: &Path, max_bytes: u64) -> Result<String, String> {
    let describe = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    let metadata = fs::metadata(path).map_err(describe)?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "{} is {} bytes, over the {max_bytes}-byte limit",
            path.display(),
            metadata.len()
        ));
    }
    let bytes = fs::read(path).map_err(describe)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A `required` list that has never loaded: startup must fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredListError {
    pub url: String,
    pub reason: String,
}

impl fmt::Display for RequiredListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "required list `{}` could not be loaded: {}",
            self.url, self.reason
        )
    }
}

impl std::error::Error for RequiredListError {}

/// Every configured list.
#[derive(Debug, Clone, Default)]
pub struct ListSet {
    lists: Vec<ListState>,
}

impl ListSet {
    /// The lists in `config`, none loaded yet. A list's refresh interval is its
    /// own, or the global one.
    pub fn from_config(config: &Config) -> Result<Self, RuleError> {
        let mut lists = Vec::new();
        for entry in &config.filter.lists {
            let not_a_list = |reason: String| RuleError {
                rule: entry.rule.clone(),
                reason,
            };
            let RuleLine::List(decl) = parse_rule_line(&entry.rule)? else {
                return Err(not_a_list(
                    "a list entry must be `<action> list <type> <url>`".to_owned(),
                ));
            };
            let refresh = entry.refresh.unwrap_or(config.filter.refresh);
            lists.push(ListState::new(decl, entry.required, refresh).map_err(not_a_list)?);
        }
        Ok(Self { lists })
    }

    pub fn lists(&self) -> &[ListState] {
        &self.lists
    }

    pub fn lists_mut(&mut self) -> &mut [ListState] {
        &mut self.lists
    }

    /// Load every `file://` list. Remote lists are left to the fetcher.
    pub fn load_files(&mut self, max_bytes: u64, now: SystemTime) {
        for list in &mut self.lists {
            list.load_file(max_bytes, now);
        }
    }

    /// Fail for the first `required` list that has never loaded, naming it.
    pub fn check_required(&self) -> Result<(), RequiredListError> {
        for list in &self.lists {
            if list.required && !list.has_loaded() {
                let reason = match &list.last_result {
                    LastResult::Failed(reason) => reason.clone(),
                    _ => "it has not been loaded".to_owned(),
                };
                return Err(RequiredListError {
                    url: list.decl.url.clone(),
                    reason,
                });
            }
        }
        Ok(())
    }

    /// Every list's last good rules, shared: a snapshot a rule-set rebuild can
    /// take without copying the rules.
    pub fn snapshot(&self) -> Vec<Arc<Vec<Rule>>> {
        self.lists
            .iter()
            .map(|list| Arc::clone(&list.rules))
            .collect()
    }

    /// The rules of every list's last good content.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.lists.iter().flat_map(|list| list.rules.iter())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        rules::{Action, ListType, Matcher},
        ruleset::RuleSet,
    };

    fn config(toml: &str) -> Config {
        Config::parse(
            Path::new("test.toml"),
            &format!("upstreams = [\"192.0.2.53\"]\n{toml}"),
        )
        .expect("valid config")
    }

    fn list_config(url: &str, extra: &str) -> Config {
        config(&format!(
            "[[filter.lists]]\nrule = \"block list domain {url}\"\n{extra}\n"
        ))
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn domains(set: &ListSet) -> Vec<String> {
        set.rules()
            .map(|rule| match &rule.matcher {
                Matcher::Domain(name) => name.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(
            file_path("file:///etc/hickory-filter/list.txt"),
            Ok(PathBuf::from("/etc/hickory-filter/list.txt"))
        );
        assert_eq!(
            file_path("file://localhost/etc/list.txt"),
            Ok(PathBuf::from("/etc/list.txt"))
        );
        assert_eq!(
            file_path("file:///tmp/my%20list.txt"),
            Ok(PathBuf::from("/tmp/my list.txt"))
        );
        for bad in [
            "file://list.txt",
            "file://example/etc/list",
            "file:///a%zz",
            "/etc/x",
        ] {
            assert!(file_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sources_are_told_apart_by_scheme() {
        assert!(matches!(Source::from_url("file:///a"), Ok(Source::File(_))));
        assert!(matches!(
            Source::from_url("https://lists.example/a.txt"),
            Ok(Source::Remote(_))
        ));
        assert!(matches!(
            Source::from_url("HTTP://lists.example/a.txt"),
            Ok(Source::Remote(_))
        ));
        assert!(Source::from_url("ftp://lists.example/a").is_err());
    }

    #[test]
    fn lists_come_from_the_config_with_their_own_or_the_global_refresh() {
        let set = ListSet::from_config(&config(
            "[filter]\nrefresh = \"12h\"\n[[filter.lists]]\nrule = \"block list hosts https://lists.example/a.txt\"\nrefresh = \"1h\"\nrequired = true\n[[filter.lists]]\nrule = \"allow list domain file:///etc/allow.txt\"\n",
        ))
        .expect("lists");
        let [a, b] = set.lists() else {
            panic!("{set:?}")
        };
        assert_eq!(a.decl.list_type, ListType::Hosts);
        assert_eq!(a.decl.action, Action::Block);
        assert_eq!(a.refresh, Interval(Duration::from_secs(3600)));
        assert!(a.required && !b.required);
        assert_eq!(b.refresh, Interval(Duration::from_secs(12 * 3600)));
        assert!(matches!(a.source, Source::Remote(_)));
        assert!(matches!(b.source, Source::File(_)));
        assert!(!a.has_loaded());
        assert_eq!(a.last_result, LastResult::NotAttempted);
    }

    #[test]
    fn an_initial_load_reads_the_file_and_counts_skipped_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("block.txt");
        fs::write(
            &path,
            "# header\nads.example\n\nbad name\ntrack.example # c\n",
        )
        .expect("write");
        let mut set = ListSet::from_config(&list_config(&file_url(&path), "")).expect("lists");

        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        let list = &set.lists()[0];
        assert!(list.has_loaded());
        assert_eq!(list.last_result, LastResult::Loaded);
        assert_eq!(list.last_success, Some(now()));
        assert_eq!(list.skipped, 1);
        assert_eq!(domains(&set), vec!["ads.example", "track.example"]);
        assert!(set.rules().all(|rule| rule.action == Action::Block));
        assert!(set.check_required().is_ok());
    }

    #[test]
    fn a_required_list_that_is_missing_fails_startup_naming_the_list() {
        let missing = "/nonexistent/hickory-filter/block.txt";
        let mut set = ListSet::from_config(&list_config(
            &format!("file://{missing}"),
            "required = true",
        ))
        .expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        let err = set.check_required().expect_err("startup must fail");
        let text = err.to_string();
        assert!(text.contains(missing), "{text}");
        assert!(text.contains("required list"), "{text}");
        assert!(text.contains("cannot read"), "{text}");
    }

    #[test]
    fn an_optional_list_that_is_missing_starts_and_records_the_failure() {
        let mut set =
            ListSet::from_config(&list_config("file:///nonexistent/block.txt", "")).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        assert!(set.check_required().is_ok());
        let list = &set.lists()[0];
        assert!(!list.has_loaded());
        assert!(matches!(&list.last_result, LastResult::Failed(r) if r.contains("cannot read")));
        assert_eq!(set.rules().count(), 0);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("block.txt");
        fs::write(&path, "ads.example\n").expect("write");
        let mut set = ListSet::from_config(&list_config(&file_url(&path), "")).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        fs::remove_file(&path).expect("remove");
        let later = now() + Duration::from_secs(60);
        set.load_files(DEFAULT_MAX_LIST_BYTES, later);

        let list = &set.lists()[0];
        assert!(matches!(list.last_result, LastResult::Failed(_)));
        assert_eq!(list.last_success, Some(now()), "the last success is kept");
        assert_eq!(domains(&set), vec!["ads.example"], "the rules are kept");
    }

    #[test]
    fn a_successful_refresh_replaces_the_rules_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("block.txt");
        fs::write(&path, "old.example\n").expect("write");
        let mut set = ListSet::from_config(&list_config(&file_url(&path), "")).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        fs::write(&path, "new.example\nother.example\n").expect("write");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now() + Duration::from_secs(60));

        assert_eq!(domains(&set), vec!["new.example", "other.example"]);
    }

    #[test]
    fn a_list_over_the_size_limit_fails_and_keeps_the_last_good_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("block.txt");
        fs::write(&path, "ads.example\n").expect("write");
        let mut set = ListSet::from_config(&list_config(&file_url(&path), "")).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        fs::write(&path, "x".repeat(100)).expect("write");
        set.load_files(50, now() + Duration::from_secs(60));

        let list = &set.lists()[0];
        assert!(matches!(&list.last_result, LastResult::Failed(r) if r.contains("limit")));
        assert_eq!(domains(&set), vec!["ads.example"]);
    }

    #[test]
    fn invalid_utf8_skips_that_line_without_failing_the_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("block.txt");
        fs::write(&path, b"ads.example\nb\xff\xfed.example\ntrack.example\n").expect("write");
        let mut set = ListSet::from_config(&list_config(&file_url(&path), "")).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        let list = &set.lists()[0];
        assert_eq!(list.last_result, LastResult::Loaded);
        assert_eq!(list.skipped, 1);
        assert_eq!(domains(&set), vec!["ads.example", "track.example"]);
    }

    #[test]
    fn one_failing_list_does_not_affect_another() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.txt");
        fs::write(&good, "ads.example\n").expect("write");
        let mut set = ListSet::from_config(&config(&format!(
            "[[filter.lists]]\nrule = \"block list domain file:///nonexistent/a.txt\"\n[[filter.lists]]\nrule = \"block list domain {}\"\n",
            file_url(&good)
        )))
        .expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        assert!(!set.lists()[0].has_loaded());
        assert!(set.lists()[1].has_loaded());
        assert_eq!(domains(&set), vec!["ads.example"]);
    }

    #[test]
    fn remote_lists_are_left_alone_by_file_loading() {
        let mut set = ListSet::from_config(&config(
            "[[filter.lists]]\nrule = \"block list hosts https://lists.example/hosts.txt\"\n",
        ))
        .expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());
        assert_eq!(set.lists()[0].last_result, LastResult::NotAttempted);
    }

    #[test]
    fn not_modified_keeps_the_rules_and_counts_as_a_success() {
        let mut set = ListSet::from_config(&config(
            "[[filter.lists]]\nrule = \"block list domain https://lists.example/a.txt\"\n",
        ))
        .expect("lists");
        let list = &mut set.lists_mut()[0];
        let validators = Validators {
            etag: Some("\"v1\"".to_owned()),
            last_modified: None,
        };
        list.apply_load("ads.example\n", validators.clone(), now());
        list.record_not_modified(now() + Duration::from_secs(60));

        assert_eq!(list.last_result, LastResult::NotModified);
        assert_eq!(list.last_success, Some(now() + Duration::from_secs(60)));
        assert_eq!(list.validators, validators);
        assert_eq!(list.rules.len(), 1);
    }

    #[test]
    fn inline_rules_and_every_list_feed_one_rule_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let block = dir.path().join("block.txt");
        let allow = dir.path().join("allow.txt");
        fs::write(&block, "ads.example\nok.example\n").expect("write");
        fs::write(&allow, "ok.example\n").expect("write");
        let config = config(&format!(
            "[filter]\nrules = [\"block wildcard *.tracker.example\"]\n[[filter.lists]]\nrule = \"block list domain {}\"\n[[filter.lists]]\nrule = \"allow list domain {}\"\n",
            file_url(&block),
            file_url(&allow)
        ));
        let mut set = ListSet::from_config(&config).expect("lists");
        set.load_files(DEFAULT_MAX_LIST_BYTES, now());

        let inline = config.inline_rules();
        let rules = RuleSet::from_rules(inline.iter().chain(set.rules())).expect("build");
        assert!(rules.is_blocked("ads.example"));
        assert!(rules.is_blocked("x.tracker.example"));
        assert!(
            !rules.is_blocked("ok.example"),
            "allow from one list beats block from another"
        );
    }
}
