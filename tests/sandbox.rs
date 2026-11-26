//! The binary in a sandbox: an unprivileged user, no capabilities, a read-only
//! root filesystem, and only the configured cache directory writable.
//!
//! Needs `bwrap` (bubblewrap) and unprivileged user namespaces. Where those are
//! not available the tests say so and return without checking anything, unless
//! `HICKORY_REQUIRE_SANDBOX` is set, in which case they fail.

use std::{
    ffi::OsString,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use hickory_filter::cache;
use hickory_server::proto::{
    op::ResponseCode,
    rr::{RData, RecordType, rdata::A},
};

mod common;
use common::{
    dns::{StubDns, ask_udp},
    ok,
    process::{http_status_line, spawn_command},
    serve,
};

/// The unprivileged user the image runs as.
const UID: &str = "65532";

/// The `bwrap` arguments up to the command: user namespace as uid and gid
/// 65532, every capability dropped, the host filesystem read-only, and `cache`
/// the only writable path.
fn sandbox_args(cache: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--unshare-user",
        "--uid",
        UID,
        "--gid",
        UID,
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--chdir",
        "/",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.extend([
        OsString::from("--bind"),
        cache.as_os_str().to_owned(),
        cache.as_os_str().to_owned(),
    ]);
    args
}

/// Whether a sandbox can be made here. When it can't, says why and returns
/// false (or panics when `HICKORY_REQUIRE_SANDBOX` is set).
fn sandbox_available() -> bool {
    let probe = Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .output();
    let problem = match probe {
        Ok(output) if output.status.success() => return true,
        Ok(output) => format!(
            "bwrap failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => format!("cannot run bwrap: {e}"),
    };
    assert!(
        std::env::var_os("HICKORY_REQUIRE_SANDBOX").is_none(),
        "HICKORY_REQUIRE_SANDBOX is set but there is no usable sandbox: {problem}"
    );
    eprintln!("SKIPPED (no usable sandbox, nothing was checked): {problem}");
    false
}

/// Run a shell snippet in the sandbox; returns (success, combined output).
fn sandboxed_shell(cache: &Path, snippet: &str) -> (bool, String) {
    let output = Command::new("bwrap")
        .args(sandbox_args(cache))
        .args(["sh", "-c", snippet])
        .output()
        .expect("run bwrap");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn the_sandbox_is_what_it_claims_to_be() {
    if !sandbox_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = dir.path().join("cache");
    std::fs::create_dir(&cache).expect("cache dir");

    // The user and capabilities.
    let (_, facts) = sandboxed_shell(&cache, "id -u; grep -E '^Cap(Eff|Prm)' /proc/self/status");
    assert!(facts.starts_with(UID), "runs as uid {UID}: {facts}");
    assert!(
        facts.contains("CapEff:\t0000000000000000") && facts.contains("CapPrm:\t0000000000000000"),
        "no capabilities: {facts}"
    );

    // Nothing is writable except the cache directory.
    let (wrote, text) = sandboxed_shell(&cache, "touch /outside");
    assert!(
        !wrote && text.contains("Read-only"),
        "the root is read-only: {text}"
    );
    let (wrote, text) = sandboxed_shell(
        &cache,
        &format!("touch {}", dir.path().join("beside").display()),
    );
    assert!(
        !wrote,
        "beside the cache directory is read-only too: {text}"
    );
    let (wrote, text) = sandboxed_shell(&cache, &format!("touch {}/inside", cache.display()));
    assert!(wrote, "the cache directory is writable: {text}");
}

/// Everything the sandboxed binary printed to stderr.
fn drain(lines: &std::sync::mpsc::Receiver<String>) -> Vec<String> {
    std::iter::from_fn(|| lines.try_recv().ok()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_binary_serves_and_writes_only_to_its_cache_directory() {
    if !sandbox_available() {
        return;
    }
    let upstream = StubDns::start(Ipv4Addr::new(192, 0, 2, 1)).await;
    let list_server = serve(|_| ok("listed.example\n")).await;
    let list_url = list_server.url("/list.txt");

    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir: PathBuf = dir.path().join("cache");
    std::fs::create_dir(&cache_dir).expect("cache dir");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "listen = [\"127.0.0.1:0\"]\nupstreams = [\"{}\"]\nhttp = \"127.0.0.1:0\"\n\
             [filter]\ncache_dir = \"{}\"\nrules = [\"block domain ads.example\"]\n\
             [[filter.lists]]\nrule = \"block list domain {list_url}\"\n",
            upstream.addr,
            cache_dir.display()
        ),
    )
    .expect("write config");

    let mut command = Command::new("bwrap");
    command
        .args(sandbox_args(&cache_dir))
        .arg(env!("CARGO_BIN_EXE_hickory-filter"))
        .arg("--config")
        .arg(&config);
    let mut process = spawn_command(command);
    let (dns, http) =
        tokio::task::block_in_place(|| (process.listening_addr(), process.http_addr()));

    // It serves: inline and list rules block, other names are forwarded.
    for name in ["ads.example.", "listed.example."] {
        let answer = ask_udp(dns, name, RecordType::A).await;
        assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
        assert_eq!(
            answer.answers[0].data,
            RData::A(A(Ipv4Addr::UNSPECIFIED)),
            "{name}"
        );
    }
    let forwarded = ask_udp(dns, "news.example.", RecordType::A).await;
    assert_eq!(
        forwarded.answers[0].data,
        RData::A(A(Ipv4Addr::new(192, 0, 2, 1)))
    );
    assert!(http_status_line(http, "/ready").contains("200"));

    // The one thing it writes is the list cache, in the cache directory.
    let cached = cache::read(&cache_dir, &list_url).expect("the list is cached");
    assert_eq!(cached.body, "listed.example\n");
    let entries: Vec<String> = std::fs::read_dir(&cache_dir)
        .expect("cache dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec![cache::file_name(&list_url)], "{entries:?}");

    // It shuts down cleanly on SIGTERM, still as an unprivileged user.
    // bwrap stays the parent, so the signal goes to the binary inside it, and
    // bwrap passes its exit status through.
    process.signal_inner("TERM");
    let (status, took) = tokio::task::block_in_place(|| process.wait_exit(Duration::from_secs(15)));
    assert_eq!(status.code(), Some(0), "{status:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");

    // Nothing complained about a read-only filesystem or missing permission,
    // and nothing appeared beside the cache directory.
    let stderr = drain(&process.lines).join("\n");
    for complaint in ["Read-only", "Permission denied", "cannot write"] {
        assert!(!stderr.contains(complaint), "{complaint}: {stderr}");
    }
    let mut top: Vec<String> = std::fs::read_dir(dir.path())
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    assert_eq!(top, vec!["cache", "config.toml"], "{top:?}");
}
