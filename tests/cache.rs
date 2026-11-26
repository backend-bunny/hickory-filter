//! The on-disk list cache across simulated restarts: each "process" is a new
//! loader and refresher, and the cache directory is what survives.

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_filter::{
    cache::{self, CachedLoader},
    config::Config,
    fetch::Fetcher,
    lists::LastResult,
    refresh::{DefaultLoader, Refresher},
    ruleset::{LiveRules, RuleSet},
};

mod common;
use common::{Reply, TestServer, serve};

/// What the list server currently serves.
struct State {
    body: String,
    etag: String,
    down: bool,
}

async fn list_server(body: &str, etag: &str) -> (TestServer, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        body: body.to_owned(),
        etag: etag.to_owned(),
        down: false,
    }));
    let shared = Arc::clone(&state);
    let server = serve(move |req| {
        let state = shared.lock().unwrap();
        if state.down {
            return Reply::Full {
                status: 503,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        if req.headers.get("if-none-match") == Some(&state.etag) {
            return Reply::Full {
                status: 304,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        Reply::Full {
            status: 200,
            headers: vec![("ETag", state.etag.clone())],
            body: state.body.clone().into_bytes(),
        }
    })
    .await;
    (server, state)
}

fn config(list_rule: &str, extra: &str) -> Config {
    Config::parse(
        Path::new("test.toml"),
        &format!(
            "upstreams = [\"192.0.2.53\"]\n[[filter.lists]]\nrule = \"{list_rule}\"\n{extra}\n"
        ),
    )
    .expect("config")
}

fn http_list(server: &TestServer, extra: &str) -> Config {
    config(
        &format!("block list domain {}", server.url("/list.txt")),
        extra,
    )
}

/// A new "process": fresh loader state over the same cache directory.
fn process(
    config: &Config,
    dir: &Path,
) -> (
    Refresher<CachedLoader<DefaultLoader>>,
    Arc<LiveRules>,
    Arc<CachedLoader<DefaultLoader>>,
) {
    let fetcher = Fetcher::new(Duration::from_secs(5), 1 << 20).expect("fetcher");
    let loader = Arc::new(CachedLoader::new(
        DefaultLoader::new(fetcher),
        dir.to_owned(),
    ));
    let live = Arc::new(LiveRules::new(RuleSet::default()));
    let refresher =
        Refresher::new(Arc::clone(&loader), config, Arc::clone(&live)).expect("refresher");
    (refresher, live, loader)
}

fn blocked(live: &LiveRules, name: &str) -> bool {
    live.load().is_blocked(name)
}

fn cache_files(dir: &Path) -> Vec<String> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn a_fetched_list_is_cached_with_its_validators() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _) = list_server("ads.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut refresher, live, _) = process(&config, dir.path());

    refresher.initial_load().await.expect("startup");

    assert!(blocked(&live, "ads.example"));
    let cached = cache::read(dir.path(), &server.url("/list.txt")).expect("cached");
    assert_eq!(cached.body, "ads.example\n");
    assert_eq!(cached.validators.etag.as_deref(), Some("\"v1\""));
    assert_eq!(cache_files(dir.path()).len(), 1, "no temporary files left");
}

#[tokio::test]
async fn a_restart_with_the_list_server_down_serves_the_cached_rules_and_becomes_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("ads.example\ntrack.example\n", "\"v1\"").await;
    // The list is `required`: without a copy, startup would fail.
    let config = http_list(&server, "required = true");

    let (mut first, _, _) = process(&config, dir.path());
    first.initial_load().await.expect("first start");
    drop(first);

    state.lock().unwrap().down = true;
    let (mut second, live, _) = process(&config, dir.path());
    second
        .initial_load()
        .await
        .expect("the cached copy satisfies the required list");

    assert!(blocked(&live, "ads.example"));
    assert!(blocked(&live, "track.example"));
    let list = &second.lists().lists()[0];
    assert!(list.has_loaded());
    assert_eq!(list.last_result, LastResult::Loaded);
}

#[tokio::test]
async fn an_unchanged_list_is_confirmed_with_the_cached_validators() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _) = list_server("ads.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut first, _, _) = process(&config, dir.path());
    first.initial_load().await.expect("first start");
    drop(first);

    // The second start sends the cached ETag; the server answers 304.
    let (mut second, live, _) = process(&config, dir.path());
    second.initial_load().await.expect("second start");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].headers.contains_key("if-none-match"));
    assert_eq!(requests[1].headers["if-none-match"], "\"v1\"");
    assert!(blocked(&live, "ads.example"), "the cached rules are used");
    assert!(second.lists().lists()[0].has_loaded());
}

#[tokio::test]
async fn a_changed_list_replaces_the_cached_copy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("old.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut first, _, _) = process(&config, dir.path());
    first.initial_load().await.expect("first start");
    drop(first);

    {
        let mut state = state.lock().unwrap();
        state.body = "new.example\n".to_owned();
        state.etag = "\"v2\"".to_owned();
    }
    let (mut second, live, _) = process(&config, dir.path());
    second.initial_load().await.expect("second start");

    assert!(blocked(&live, "new.example"));
    assert!(!blocked(&live, "old.example"));
    let cached = cache::read(dir.path(), &server.url("/list.txt")).expect("cached");
    assert_eq!(cached.body, "new.example\n");
    assert_eq!(cached.validators.etag.as_deref(), Some("\"v2\""));
}

#[tokio::test]
async fn a_list_started_from_the_cache_refreshes_when_the_server_returns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("old.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut first, _, _) = process(&config, dir.path());
    first.initial_load().await.expect("first start");
    drop(first);

    state.lock().unwrap().down = true;
    let (mut second, live, _) = process(&config, dir.path());
    second.initial_load().await.expect("started from the cache");
    assert!(blocked(&live, "old.example"));

    {
        let mut state = state.lock().unwrap();
        state.down = false;
        state.body = "new.example\n".to_owned();
        state.etag = "\"v2\"".to_owned();
    }
    second.refresh_all().await;
    assert!(blocked(&live, "new.example"));
    assert!(!blocked(&live, "old.example"));
}

#[tokio::test]
async fn later_failures_keep_the_in_memory_rules_and_leave_the_cache_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("ads.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut refresher, live, _) = process(&config, dir.path());
    refresher.initial_load().await.expect("start");
    let before = fs::read(dir.path().join(cache::file_name(&server.url("/list.txt")))).unwrap();

    state.lock().unwrap().down = true;
    refresher.refresh_all().await;

    assert!(blocked(&live, "ads.example"));
    assert!(matches!(
        refresher.lists().lists()[0].last_result,
        LastResult::Failed(_)
    ));
    let after = fs::read(dir.path().join(cache::file_name(&server.url("/list.txt")))).unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn without_a_cached_copy_a_required_list_still_fails_when_the_server_is_down() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("ads.example\n", "\"v1\"").await;
    state.lock().unwrap().down = true;

    let required = http_list(&server, "required = true");
    let (mut refresher, _, _) = process(&required, dir.path());
    let error = refresher.initial_load().await.expect_err("startup fails");
    assert!(
        error.to_string().contains(&server.url("/list.txt")),
        "{error}"
    );

    let optional = http_list(&server, "");
    let (mut refresher, live, _) = process(&optional, dir.path());
    refresher
        .initial_load()
        .await
        .expect("an optional list may fail");
    assert!(!blocked(&live, "ads.example"));
    assert!(matches!(
        refresher.lists().lists()[0].last_result,
        LastResult::Failed(_)
    ));
}

#[tokio::test]
async fn a_corrupt_cache_file_is_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, state) = list_server("ads.example\n", "\"v1\"").await;
    state.lock().unwrap().down = true;
    let url = server.url("/list.txt");
    fs::write(dir.path().join(cache::file_name(&url)), "not a cache file").unwrap();

    let config = http_list(&server, "required = true");
    let (mut refresher, _, _) = process(&config, dir.path());
    assert!(refresher.initial_load().await.is_err());
}

#[tokio::test]
async fn local_file_lists_are_never_cached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = dir.path().join("cache");
    let list = dir.path().join("block.txt");
    fs::write(&list, "ads.example\n").unwrap();
    let config = config(&format!("block list domain file://{}", list.display()), "");
    let (mut refresher, live, _) = process(&config, &cache_dir);

    refresher.initial_load().await.expect("start");

    assert!(blocked(&live, "ads.example"));
    assert!(
        !cache_dir.exists(),
        "nothing was written, not even the directory"
    );
}

#[tokio::test]
async fn a_cache_that_cannot_be_written_does_not_fail_the_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The "directory" is a file, so it can't be created or written into.
    let not_a_dir = dir.path().join("occupied");
    fs::write(&not_a_dir, "a file").unwrap();
    let (server, _) = list_server("ads.example\n", "\"v1\"").await;
    let config = http_list(&server, "");
    let (mut refresher, live, loader) = process(&config, &not_a_dir);

    refresher
        .initial_load()
        .await
        .expect("the load itself succeeds");

    assert!(blocked(&live, "ads.example"));
    let error = loader.last_write_error().expect("the failure is recorded");
    assert!(error.contains("cannot write the list cache"), "{error}");
}
