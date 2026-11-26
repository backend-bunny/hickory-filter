//! The refresher with its real loader: a file list and an HTTP list.

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_filter::{
    config::Config,
    fetch::Fetcher,
    lists::LastResult,
    refresh::{DefaultLoader, Refresher},
    ruleset::{LiveRules, RuleSet},
};

mod common;
use common::{Reply, ok, serve};

fn blocked(live: &LiveRules, name: &str) -> bool {
    live.load().is_blocked(name)
}

fn refresher(config: &Config, live: &Arc<LiveRules>) -> Refresher<DefaultLoader> {
    let fetcher = Fetcher::new(Duration::from_secs(5), 1 << 20).expect("fetcher");
    Refresher::new(
        Arc::new(DefaultLoader::new(fetcher)),
        config,
        Arc::clone(live),
    )
    .expect("refresher")
}

#[tokio::test]
async fn file_and_http_lists_load_refresh_and_survive_a_failing_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("block.txt");
    fs::write(&file, "from-file.example\n").expect("write");

    // 0 = serving, 1 = serving the changed list, 2 = down.
    let mode = Arc::new(Mutex::new(0));
    let current = Arc::clone(&mode);
    let server = serve(move |_| match *current.lock().unwrap() {
        0 => ok("from-http.example\n"),
        1 => ok("from-http.example\nnew-http.example\n"),
        _ => Reply::Full {
            status: 503,
            headers: Vec::new(),
            body: Vec::new(),
        },
    })
    .await;

    let config = Config::parse(
        Path::new("test.toml"),
        &format!(
            "upstreams = [\"192.0.2.53\"]\n[filter]\nrules = [\"block domain inline.example\"]\n\
             [[filter.lists]]\nrule = \"block list domain file://{}\"\n\
             [[filter.lists]]\nrule = \"block list domain {}\"\n",
            file.display(),
            server.url("/list.txt")
        ),
    )
    .expect("config");
    let live = Arc::new(LiveRules::new(RuleSet::default()));
    let mut refresher = refresher(&config, &live);

    refresher.initial_load().await.expect("startup");
    assert!(blocked(&live, "inline.example"));
    assert!(blocked(&live, "from-file.example"));
    assert!(blocked(&live, "from-http.example"));
    assert!(!blocked(&live, "new-http.example"));
    assert_eq!(live.generation(), 1);

    // Both sources change; one refresh picks both up with one swap.
    fs::write(&file, "from-file.example\nnew-file.example\n").expect("write");
    *mode.lock().unwrap() = 1;
    refresher.refresh_all().await;
    assert!(blocked(&live, "new-file.example"));
    assert!(blocked(&live, "new-http.example"));
    assert_eq!(live.generation(), 2, "one swap for both lists");

    // The server goes down and the file disappears: nothing is lost.
    *mode.lock().unwrap() = 2;
    fs::remove_file(&file).expect("remove");
    refresher.refresh_all().await;
    assert!(
        blocked(&live, "new-http.example"),
        "the last good HTTP rules stay"
    );
    assert!(
        blocked(&live, "new-file.example"),
        "the last good file rules stay"
    );
    assert!(blocked(&live, "inline.example"));
    assert_eq!(live.generation(), 2, "failures swap nothing");
    for list in refresher.lists().lists() {
        assert!(
            matches!(list.last_result, LastResult::Failed(_)),
            "{list:?}"
        );
        assert!(list.has_loaded());
    }
}

#[tokio::test]
async fn startup_fails_for_a_required_file_list_that_is_missing() {
    let config = Config::parse(
        Path::new("test.toml"),
        "upstreams = [\"192.0.2.53\"]\n[[filter.lists]]\n\
         rule = \"block list domain file:///nonexistent/hickory-filter/block.txt\"\nrequired = true\n",
    )
    .expect("config");
    let live = Arc::new(LiveRules::new(RuleSet::default()));
    let mut refresher = refresher(&config, &live);

    let error = refresher
        .initial_load()
        .await
        .expect_err("startup must fail");
    let text = error.to_string();
    assert!(
        text.contains("/nonexistent/hickory-filter/block.txt"),
        "{text}"
    );
    assert!(text.contains("required list"), "{text}");
}
