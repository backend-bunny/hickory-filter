//! Running a test inside a fresh user and network namespace.
//!
//! Recursion tests need fake root, TLD and authoritative servers on port 53 at
//! addresses the recursor accepts. In a namespace created with
//! `unshare --user --map-root-user --net` the test is root, so it can bring up
//! the loopback interface, give it those addresses and bind port 53, without
//! touching the host. The test re-runs itself inside the namespace:
//!
//! ```ignore
//! #[tokio::test]
//! async fn something() {
//!     let Some(_ns) = netns::enter("something", &authority::addresses()) else { return };
//!     // ... runs inside the namespace ...
//! }
//! ```
//!
//! Where namespaces are not available the test says so and returns, unless
//! `HICKORY_REQUIRE_SANDBOX` is set, in which case it fails.

use std::{
    process::{Command, Output},
    sync::OnceLock,
};

const INSIDE: &str = "HICKORY_NETNS";

/// Proof that the test is running inside the namespace.
pub struct Namespace(());

fn unshare() -> Command {
    let mut command = Command::new("unshare");
    command.args(["--user", "--map-root-user", "--net", "--"]);
    command
}

fn ip(args: &[&str]) {
    let output = Command::new("ip").args(args).output().expect("run ip");
    assert!(
        output.status.success(),
        "ip {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Why a namespace can't be made here, or `None` when it can.
fn problem() -> Option<&'static str> {
    static PROBLEM: OnceLock<Option<String>> = OnceLock::new();
    PROBLEM
        .get_or_init(|| {
            let probe: Result<Output, _> = unshare()
                .args([
                    "sh",
                    "-c",
                    "ip link set lo up && ip addr add 198.18.255.1/32 dev lo",
                ])
                .output();
            match probe {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
                Err(e) => Some(format!("cannot run unshare: {e}")),
            }
        })
        .as_deref()
}

/// Run test `name` of this test binary inside a namespace and fail unless it
/// passes there; inside the namespace, set it up and return the proof.
///
/// Returns `None` in the outer process (the work is done, or skipped), and
/// `Some` only inside the namespace, where the test body must continue.
pub fn enter(name: &str, addresses: &[String]) -> Option<Namespace> {
    if std::env::var_os(INSIDE).is_some() {
        ip(&["link", "set", "lo", "up"]);
        for address in addresses {
            ip(&["addr", "add", &format!("{address}/32"), "dev", "lo"]);
        }
        return Some(Namespace(()));
    }
    if let Some(why) = problem() {
        assert!(
            std::env::var_os("HICKORY_REQUIRE_SANDBOX").is_none(),
            "HICKORY_REQUIRE_SANDBOX is set but there is no usable namespace: {why}"
        );
        eprintln!("skipping {name}: no usable user and network namespace here ({why})");
        return None;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let output = unshare()
        .arg(exe)
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(INSIDE, "1")
        .output()
        .expect("run the test in a namespace");
    assert!(
        output.status.success(),
        "{name} failed inside the namespace:\n--- stdout\n{}\n--- stderr\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    None
}
