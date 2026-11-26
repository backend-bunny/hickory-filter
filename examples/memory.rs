//! Resident memory of a 1M-name rule set, at rest and during a swap:
//! `cargo run --release --example memory`. Linux only (reads /proc).

use std::{fs, time::Instant};

use hickory_filter::ruleset::{LiveRules, RuleSet};

// Shared with the benchmark; it uses functions this example does not.
#[allow(dead_code)]
#[path = "../benches/workload.rs"]
mod workload;

/// A field of /proc/self/status in KiB, such as `VmRSS` or `VmHWM`.
fn status_kib(field: &str) -> u64 {
    let status = fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    status
        .lines()
        .find_map(|line| line.strip_prefix(field)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no {field} in /proc/self/status"))
}

fn mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

fn main() {
    let names = workload::EXACT_NAMES;
    let baseline = status_kib("VmRSS");

    let started = Instant::now();
    let live = LiveRules::new(RuleSet::from_rules(workload::rules(names)).expect("build"));
    let build_time = started.elapsed();
    let steady = status_kib("VmRSS");
    let build_peak = status_kib("VmHWM");
    let index_bytes = live.load().name_index_bytes();

    // A refresh: build the replacement while the live set is still serving.
    let replacement = RuleSet::from_rules(workload::rules(names)).expect("build");
    let during_swap = status_kib("VmRSS");
    let peak = status_kib("VmHWM");

    live.swap(replacement);
    // The old set is freed when the last reader drops it; here, right away.
    let after_swap = status_kib("VmRSS");

    let steady_delta = steady - baseline;
    println!("exact names:        {names}");
    println!("build time:         {:.2} s", build_time.as_secs_f64());
    println!("baseline RSS:       {:.1} MiB", mib(baseline));
    println!(
        "steady RSS:         {:.1} MiB (+{:.1} MiB)",
        mib(steady),
        mib(steady_delta)
    );
    println!(
        "name index (FST):   {:.1} MiB",
        index_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "per name:           {:.0} bytes",
        (steady_delta * 1024) as f64 / names as f64
    );
    println!(
        "during swap RSS:    {:.1} MiB (both sets live)",
        mib(during_swap)
    );
    println!(
        "peak after 1st build: {:.1} MiB (VmHWM before the replacement)",
        mib(build_peak)
    );
    println!("peak RSS (VmHWM):   {:.1} MiB", mib(peak));
    println!("after swap RSS:     {:.1} MiB", mib(after_swap));
}
