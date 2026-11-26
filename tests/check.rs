//! `--check` against the real binary: it validates the configuration and
//! exits, without opening listeners or fetching lists.

use std::{
    net::{TcpListener, UdpSocket},
    path::Path,
    process::{Command, Output},
};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hickory-filter"))
        .args(args)
        .output()
        .expect("run hickory-filter")
}

fn check(path: &Path) -> Output {
    run(&["--check", "--config", path.to_str().expect("utf-8 path")])
}

#[test]
fn check_on_a_valid_config_exits_zero_and_opens_no_sockets() {
    // Occupy the configured port on UDP and TCP. If --check tried to bind its
    // listeners it would fail; it must not touch them.
    let tcp = TcpListener::bind("127.0.0.1:0").expect("bind tcp");
    let port = tcp.local_addr().expect("addr").port();
    let _udp = UdpSocket::bind(("127.0.0.1", port)).expect("bind udp");

    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("config.toml");
    std::fs::write(
        &file,
        format!(
            "listen = [\"127.0.0.1:{port}\"]\nhttp = \"127.0.0.1:{port}\"\nupstreams = [\"192.0.2.53\"]\n\
             [[filter.lists]]\nrule = \"block list hosts https://lists.example/hosts.txt\"\n"
        ),
    )
    .expect("write config");

    let output = check(&file);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("configuration ok"));
}

#[test]
fn check_on_an_invalid_config_exits_nonzero_and_names_file_and_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("bad.toml");
    std::fs::write(&file, "upstreams = [\"192.0.2.53\"]\nlisten_adress = []\n").expect("write");

    let output = check(&file);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.toml"), "{stderr}");
    assert!(stderr.contains("listen_adress"), "{stderr}");
}

#[test]
fn a_missing_config_file_fails_startup() {
    let output = check(Path::new("/nonexistent/hickory-filter.toml"));
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("/nonexistent/hickory-filter.toml"));
}

#[test]
fn the_example_config_passes_check() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/hickory-filter.toml");
    let output = check(&example);
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn check_rejects_an_invalid_rule_and_names_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("rules.toml");
    std::fs::write(
        &file,
        "upstreams = [\"192.0.2.53\"]\n[filter]\nrules = [\"block glob *.example\"]\n",
    )
    .expect("write");

    let output = check(&file);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rules.toml"), "{stderr}");
    assert!(stderr.contains("block glob *.example"), "{stderr}");
    assert!(stderr.contains("accepted types"), "{stderr}");
}
