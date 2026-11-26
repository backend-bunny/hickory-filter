//! Keeping the rule set current: the initial load and the scheduled refresh.
//!
//! One task owns every list's state. Each list has its own interval (`0`
//! disables its refresh). When lists come due it loads them with bounded
//! concurrency, applies the results, and, only if some list's rules changed,
//! builds one new rule set off the query path and swaps it in. Cycles run one
//! after another, so they never overlap, and a slow cycle is not followed by a
//! burst of catch-up cycles: the next one is due a full interval after the
//! previous one finished. Parsing and the rule-set build run on the blocking
//! pool; the query path only ever does an atomic load.

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime},
};

use tokio::{
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, sleep_until},
};

use crate::{
    config::Config,
    fetch::{FetchOutcome, Fetcher},
    lists::{
        DEFAULT_MAX_LIST_BYTES, ListSet, LoadResult, RequiredListError, Source, Validators,
        read_list_file,
    },
    rules::{Rule, RuleError, parse_list},
    ruleset::{LiveRules, RuleSet},
};

/// How many lists are loaded at the same time by default.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Where a list's content comes from. The server uses `DefaultLoader`; tests
/// substitute their own.
pub trait ListLoader: Send + Sync + 'static {
    /// Load `source`, sending `validators` if it is remote. An error is the
    /// reason the load failed.
    fn load(
        &self,
        source: &Source,
        validators: &Validators,
    ) -> impl Future<Output = Result<FetchOutcome, String>> + Send;
}

/// Reads `file://` lists from disk and fetches remote lists over HTTP(S).
pub struct DefaultLoader {
    fetcher: Fetcher,
    max_bytes: u64,
}

impl DefaultLoader {
    pub fn new(fetcher: Fetcher) -> Self {
        Self {
            fetcher,
            max_bytes: DEFAULT_MAX_LIST_BYTES,
        }
    }
}

impl ListLoader for DefaultLoader {
    async fn load(&self, source: &Source, validators: &Validators) -> Result<FetchOutcome, String> {
        match source {
            Source::File(path) => {
                let path = path.clone();
                let max_bytes = self.max_bytes;
                let body = tokio::task::spawn_blocking(move || read_list_file(&path, max_bytes))
                    .await
                    .map_err(|e| e.to_string())??;
                Ok(FetchOutcome::Fetched {
                    body,
                    validators: Validators::default(),
                })
            }
            Source::Remote(url) => self
                .fetcher
                .fetch(url, validators)
                .await
                .map_err(|e| format!("GET {url}: {e}")),
        }
    }
}

/// Loads every list, builds the rule set, and keeps it current.
pub struct Refresher<L: ListLoader> {
    loader: Arc<L>,
    lists: ListSet,
    inline: Arc<Vec<Rule>>,
    live: Arc<LiveRules>,
    concurrency: usize,
    /// When each list is next due; `None` when its refresh is disabled.
    due: Vec<Option<Instant>>,
    /// Why the last rule-set build failed, if it did. The previous set stays.
    pub last_build_error: Option<String>,
}

impl<L: ListLoader> Refresher<L> {
    /// A refresher for the lists and inline rules in `config`, swapping into
    /// `live`.
    pub fn new(loader: Arc<L>, config: &Config, live: Arc<LiveRules>) -> Result<Self, RuleError> {
        let lists = ListSet::from_config(config)?;
        let due = vec![None; lists.lists().len()];
        Ok(Self {
            loader,
            lists,
            inline: Arc::new(config.inline_rules()),
            live,
            concurrency: DEFAULT_CONCURRENCY,
            due,
            last_build_error: None,
        })
    }

    /// Load at most this many lists at the same time.
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    pub fn lists(&self) -> &ListSet {
        &self.lists
    }

    /// Load every list, build the first rule set and swap it in, then
    /// schedule the refreshes. Fails for a `required` list that could not be
    /// loaded; any other failed list is recorded and the server starts without
    /// its rules.
    pub async fn initial_load(&mut self) -> Result<(), RequiredListError> {
        let all: Vec<usize> = (0..self.lists.lists().len()).collect();
        self.load_lists(&all).await;
        // Always build here, and exactly once: even with no list content there
        // are inline rules.
        self.rebuild_and_swap().await;
        self.schedule(&all, Instant::now());
        self.lists.check_required()
    }

    /// Load every list now and swap once if any changed.
    pub async fn refresh_all(&mut self) {
        let all: Vec<usize> = (0..self.lists.lists().len()).collect();
        if self.load_lists(&all).await {
            self.rebuild_and_swap().await;
        }
    }

    /// Run until `shutdown` completes: sleep until a list is due, refresh the
    /// lists that are, and reschedule them.
    pub async fn run(mut self, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        loop {
            let Some(next) = self.due.iter().flatten().min().copied() else {
                // Every refresh is disabled: nothing to do but wait.
                shutdown.await;
                return;
            };
            tokio::select! {
                () = sleep_until(next) => {}
                () = &mut shutdown => return,
            }
            let now = Instant::now();
            let due: Vec<usize> = (0..self.due.len())
                .filter(|&i| self.due[i].is_some_and(|at| at <= now))
                .collect();
            tokio::select! {
                () = self.cycle(&due) => {}
                () = &mut shutdown => return,
            }
            // The next cycle is a full interval after this one finished.
            self.schedule(&due, Instant::now());
        }
    }

    /// One refresh cycle for the lists at `indices`: load them, apply the
    /// results, and swap once if any list's rules changed.
    async fn cycle(&mut self, indices: &[usize]) {
        if self.load_lists(indices).await {
            self.rebuild_and_swap().await;
        }
    }

    fn schedule(&mut self, indices: &[usize], from: Instant) {
        for &i in indices {
            let interval: Duration = self.lists.lists()[i].refresh.0;
            self.due[i] = (!interval.is_zero()).then(|| from + interval);
        }
    }

    /// Load the lists at `indices` with bounded concurrency and apply the
    /// results. Returns whether any list's rules changed.
    async fn load_lists(&mut self, indices: &[usize]) -> bool {
        let permits = Arc::new(Semaphore::new(self.concurrency));
        let mut loads = JoinSet::new();
        for &index in indices {
            let list = &self.lists.lists()[index];
            let (source, validators) = (list.source.clone(), list.validators.clone());
            let (action, list_type) = (list.decl.action, list.decl.list_type);
            let loader = Arc::clone(&self.loader);
            let permits = Arc::clone(&permits);
            loads.spawn(async move {
                let _permit = permits.acquire_owned().await;
                let result = match loader.load(&source, &validators).await {
                    Ok(FetchOutcome::Fetched { body, validators }) => {
                        // Parsing a big list is CPU work: keep it off the
                        // async workers.
                        match tokio::task::spawn_blocking(move || {
                            parse_list(list_type, action, &body)
                        })
                        .await
                        {
                            Ok(parsed) => LoadResult::Loaded { parsed, validators },
                            Err(e) => LoadResult::Failed(format!("parsing failed: {e}")),
                        }
                    }
                    Ok(FetchOutcome::NotModified) => LoadResult::NotModified,
                    Err(reason) => LoadResult::Failed(reason),
                };
                (index, result)
            });
        }

        let mut changed = false;
        while let Some(joined) = loads.join_next().await {
            // A task that panicked has no result to apply; its list keeps its
            // last good rules.
            if let Ok((index, result)) = joined {
                let list = &mut self.lists.lists_mut()[index];
                changed |= list.apply(result, SystemTime::now());
                crate::metrics::record_list(list);
            }
        }
        changed
    }

    /// Build a rule set from the inline rules and every list's last good
    /// rules, off the query path, and swap it in. If the build fails the
    /// previous set stays.
    async fn rebuild_and_swap(&mut self) {
        let inline = Arc::clone(&self.inline);
        let snapshot = self.lists.snapshot();
        let built = tokio::task::spawn_blocking(move || {
            RuleSet::from_rules(
                inline
                    .iter()
                    .chain(snapshot.iter().flat_map(|rules| rules.iter())),
            )
        })
        .await;
        match built {
            Ok(Ok(set)) => {
                self.live.swap(set);
                self.last_build_error = None;
            }
            Ok(Err(e)) => self.last_build_error = Some(e.to_string()),
            Err(e) => self.last_build_error = Some(format!("rule set build failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::Path,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;
    use crate::lists::LastResult;

    /// What the fake loader answers for one URL.
    #[derive(Clone)]
    enum Script {
        Content(String),
        NotModified,
        Error(String),
    }

    /// An in-memory loader: scripted answers, a recorded call log, optional
    /// latency, and a high-water mark of simultaneous loads.
    struct FakeLoader {
        scripts: Mutex<HashMap<String, Script>>,
        calls: Mutex<Vec<(String, Instant)>>,
        delay: Duration,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    impl FakeLoader {
        fn new(delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                scripts: Mutex::new(HashMap::new()),
                calls: Mutex::new(Vec::new()),
                delay,
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
            })
        }

        fn set(&self, url: &str, script: Script) {
            self.scripts.lock().unwrap().insert(url.to_owned(), script);
        }

        fn content(&self, url: &str, body: &str) {
            self.set(url, Script::Content(body.to_owned()));
        }

        fn calls_for(&self, url: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| called == url)
                .count()
        }

        fn max_in_flight(&self) -> usize {
            self.max_in_flight.load(Ordering::SeqCst)
        }
    }

    impl ListLoader for FakeLoader {
        async fn load(
            &self,
            source: &Source,
            _validators: &Validators,
        ) -> Result<FetchOutcome, String> {
            let url = match source {
                Source::Remote(url) => url.clone(),
                Source::File(path) => format!("file://{}", path.display()),
            };
            self.calls
                .lock()
                .unwrap()
                .push((url.clone(), Instant::now()));
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            let script = self.scripts.lock().unwrap().get(&url).cloned();
            match script {
                Some(Script::Content(body)) => Ok(FetchOutcome::Fetched {
                    body,
                    validators: Validators::default(),
                }),
                Some(Script::NotModified) => Ok(FetchOutcome::NotModified),
                Some(Script::Error(reason)) => Err(reason),
                None => Err(format!("no script for {url}")),
            }
        }
    }

    const A: &str = "https://lists.example/a.txt";
    const B: &str = "https://lists.example/b.txt";

    fn config(global_refresh: &str, lists: &[(&str, &str)], inline: &[&str]) -> Config {
        let mut toml = format!(
            "upstreams = [\"192.0.2.53\"]\n[filter]\nrefresh = \"{global_refresh}\"\nrules = [{}]\n",
            inline
                .iter()
                .map(|rule| format!("\"{rule}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (url, extra) in lists {
            toml.push_str(&format!(
                "[[filter.lists]]\nrule = \"block list domain {url}\"\n{extra}\n"
            ));
        }
        Config::parse(Path::new("test.toml"), &toml).expect("config")
    }

    fn empty_live() -> Arc<LiveRules> {
        Arc::new(LiveRules::new(RuleSet::default()))
    }

    fn blocked(live: &LiveRules, name: &str) -> bool {
        live.load().is_blocked(name)
    }

    const HOUR: Duration = Duration::from_secs(3600);

    // --- the initial load ---

    #[tokio::test(start_paused = true)]
    async fn the_initial_load_builds_one_rule_set_from_inline_rules_and_every_list() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        loader.content(B, "track.example\nok.example\n");
        let live = empty_live();
        let config = config(
            "24h",
            &[(A, ""), (B, "")],
            &[
                "block wildcard *.tracker.example",
                "allow domain ok.example",
            ],
        );
        let mut refresher = Refresher::new(loader, &config, Arc::clone(&live)).expect("refresher");

        refresher.initial_load().await.expect("startup");

        assert_eq!(live.generation(), 1, "one swap for the whole initial load");
        assert!(blocked(&live, "ads.example"));
        assert!(blocked(&live, "track.example"));
        assert!(blocked(&live, "x.tracker.example"));
        assert!(
            !blocked(&live, "ok.example"),
            "allow beats block across sources"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_required_list_that_fails_stops_startup_an_optional_one_does_not() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        loader.set(B, Script::Error("connection refused".to_owned()));

        let optional = config("24h", &[(A, ""), (B, "")], &[]);
        let live = empty_live();
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &optional, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("starts without B");
        assert!(
            blocked(&live, "ads.example"),
            "the other list's rules are served"
        );
        assert!(matches!(
            refresher.lists().lists()[1].last_result,
            LastResult::Failed(_)
        ));

        let required = config("24h", &[(A, ""), (B, "required = true")], &[]);
        let mut refresher = Refresher::new(loader, &required, empty_live()).expect("refresher");
        let error = refresher
            .initial_load()
            .await
            .expect_err("startup must fail");
        assert!(error.to_string().contains(B), "{error}");
        assert!(error.to_string().contains("connection refused"), "{error}");
    }

    // --- scheduled refresh ---

    #[tokio::test(start_paused = true)]
    async fn a_list_that_gains_a_line_is_blocked_after_the_next_refresh() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        let live = empty_live();
        let config = config("1h", &[(A, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("startup");
        assert!(!blocked(&live, "new.example"));

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        loader.content(A, "ads.example\nnew.example\n");

        // Not yet: the interval is an hour.
        tokio::time::sleep(HOUR / 2).await;
        assert!(!blocked(&live, "new.example"));
        tokio::time::sleep(HOUR / 2 + Duration::from_secs(1)).await;
        assert!(blocked(&live, "new.example"), "no restart needed");
        assert!(blocked(&live, "ads.example"));

        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn with_the_global_refresh_at_zero_each_list_is_fetched_exactly_once() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        loader.content(B, "track.example\n");
        let config = config("0", &[(A, ""), (B, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, empty_live()).expect("refresher");
        refresher.initial_load().await.expect("startup");

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        tokio::time::sleep(Duration::from_secs(48 * 3600)).await;

        assert_eq!(loader.calls_for(A), 1);
        assert_eq!(loader.calls_for(B), 1);
        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn each_list_refreshes_on_its_own_interval() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "a.example\n");
        loader.content(B, "b.example\n");
        let config = config(
            "24h",
            &[(A, "refresh = \"1h\""), (B, "refresh = \"6h\"")],
            &[],
        );
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, empty_live()).expect("refresher");
        refresher.initial_load().await.expect("startup");

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        tokio::time::sleep(7 * HOUR + HOUR / 2).await;

        // The initial load, then hours 1..=7 for A and hour 6 for B.
        assert_eq!(loader.calls_for(A), 1 + 7);
        assert_eq!(loader.calls_for(B), 1 + 1);
        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn a_list_with_refresh_zero_is_left_alone_while_others_refresh() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "a.example\n");
        loader.content(B, "b.example\n");
        let config = config("1h", &[(A, "refresh = 0"), (B, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, empty_live()).expect("refresher");
        refresher.initial_load().await.expect("startup");

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        tokio::time::sleep(3 * HOUR + HOUR / 2).await;

        assert_eq!(loader.calls_for(A), 1);
        assert_eq!(loader.calls_for(B), 1 + 3);
        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    // --- last good copy on failure ---

    #[tokio::test(start_paused = true)]
    async fn a_failed_refresh_keeps_the_last_good_rules_and_records_the_failure() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        let live = empty_live();
        let config = config("1h", &[(A, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("startup");
        let generation = live.generation();

        loader.set(A, Script::Error("HTTP status 503".to_owned()));
        refresher.refresh_all().await;

        assert!(blocked(&live, "ads.example"), "ads.example stays blocked");
        assert_eq!(live.generation(), generation, "a failure swaps nothing");
        let list = &refresher.lists().lists()[0];
        assert!(matches!(&list.last_result, LastResult::Failed(r) if r.contains("503")));
        assert!(list.has_loaded(), "the last success is kept");
    }

    // --- one swap per cycle ---

    #[tokio::test(start_paused = true)]
    async fn a_cycle_that_changes_several_lists_swaps_once() {
        let loader = FakeLoader::new(Duration::ZERO);
        let urls = [A, B, "https://lists.example/c.txt"];
        for url in urls {
            loader.content(url, "old.example\n");
        }
        let live = empty_live();
        let lists: Vec<(&str, &str)> = urls.iter().map(|url| (*url, "")).collect();
        let config = config("1h", &lists, &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("startup");
        let generation = live.generation();

        for (i, url) in urls.iter().enumerate() {
            loader.content(url, &format!("new{i}.example\n"));
        }
        refresher.refresh_all().await;

        assert_eq!(
            live.generation(),
            generation + 1,
            "one swap for three changes"
        );
        for i in 0..3 {
            assert!(blocked(&live, &format!("new{i}.example")));
        }
        assert!(!blocked(&live, "old.example"));
    }

    #[tokio::test(start_paused = true)]
    async fn unchanged_content_and_not_modified_swap_nothing() {
        let loader = FakeLoader::new(Duration::ZERO);
        loader.content(A, "ads.example\n");
        loader.content(B, "track.example\n");
        let live = empty_live();
        let config = config("1h", &[(A, ""), (B, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("startup");
        let generation = live.generation();

        // A returns the same content again, B says 304.
        loader.set(B, Script::NotModified);
        refresher.refresh_all().await;

        assert_eq!(live.generation(), generation);
        assert_eq!(
            refresher.lists().lists()[1].last_result,
            LastResult::NotModified
        );
        assert!(blocked(&live, "track.example"));
    }

    // --- no overlap, bounded concurrency, queries never wait ---

    #[tokio::test(start_paused = true)]
    async fn a_slow_list_never_overlaps_its_own_next_refresh() {
        // Loading takes longer than the interval.
        let loader = FakeLoader::new(2 * HOUR);
        loader.content(A, "ads.example\n");
        let config = config("1h", &[(A, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, empty_live()).expect("refresher");
        refresher.initial_load().await.expect("startup");

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        tokio::time::sleep(12 * HOUR).await;

        assert_eq!(
            loader.max_in_flight(),
            1,
            "never two loads of the list at once"
        );
        // Each load takes 2h and the next is due 1h after it ends: a load
        // every 3h, not a burst of catch-up loads.
        let calls = loader.calls_for(A);
        assert!((4..=6).contains(&calls), "{calls} loads in 12h");
        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn loads_run_concurrently_but_never_beyond_the_bound() {
        let loader = FakeLoader::new(Duration::from_secs(10));
        let urls: Vec<String> = (0..10)
            .map(|i| format!("https://lists.example/{i}.txt"))
            .collect();
        for url in &urls {
            loader.content(url, "x.example\n");
        }
        let lists: Vec<(&str, &str)> = urls.iter().map(|url| (url.as_str(), "")).collect();
        let config = config("24h", &lists, &[]);
        let mut refresher = Refresher::new(Arc::clone(&loader), &config, empty_live())
            .expect("refresher")
            .with_concurrency(4);

        refresher.initial_load().await.expect("startup");

        assert_eq!(loader.max_in_flight(), 4, "bounded at the limit, and used");
        for url in &urls {
            assert_eq!(loader.calls_for(url), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn queries_are_answered_from_the_old_set_while_a_refresh_is_in_flight() {
        let loader = FakeLoader::new(HOUR);
        loader.content(A, "ads.example\n");
        let live = empty_live();
        let config = config("1h", &[(A, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, Arc::clone(&live)).expect("refresher");
        refresher.initial_load().await.expect("startup");

        loader.content(A, "ads.example\nnew.example\n");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        // Into the middle of the second load (it started at 1h, takes 1h).
        tokio::time::sleep(HOUR + HOUR / 2).await;

        assert!(blocked(&live, "ads.example"), "the old set answers");
        assert!(
            !blocked(&live, "new.example"),
            "the new set isn't half in place"
        );

        tokio::time::sleep(HOUR).await;
        assert!(blocked(&live, "new.example"));
        stop.send(()).expect("stop");
        task.await.expect("run ends on shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn run_stops_on_shutdown_even_in_the_middle_of_a_load() {
        let loader = FakeLoader::new(10 * HOUR);
        loader.content(A, "ads.example\n");
        let config = config("1h", &[(A, "")], &[]);
        let mut refresher =
            Refresher::new(Arc::clone(&loader), &config, empty_live()).expect("refresher");
        // Skip the slow initial wait: load once with no delay budget concerns.
        refresher.initial_load().await.expect("startup");

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(refresher.run(async move {
            let _ = stopped.await;
        }));
        tokio::time::sleep(HOUR + Duration::from_secs(1)).await; // a load is now in flight
        stop.send(()).expect("stop");
        task.await.expect("run ends promptly on shutdown");
    }
}
