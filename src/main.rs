//! hickory-filter: a filtering DNS forwarder.

use std::{path::PathBuf, process::ExitCode};

use hickory_filter::{
    config::{Config, DEFAULT_CONFIG_PATH},
    server,
};
use tokio::signal::unix::{SignalKind, signal};

const USAGE: &str = "\
usage: hickory-filter [--config <file>] [--check]
       hickory-filter --version

  --config <file>  configuration file (default: /etc/hickory-filter/config.toml)
  --check          validate the configuration and exit without serving
  --version, -V    print the version and exit
  --help, -h       print this help and exit";

/// The line printed for `--version`: `hickory-filter X.Y.Z`.
fn version_line() -> String {
    format!("hickory-filter {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Version,
    Help,
    Run { config: PathBuf, check: bool },
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut config = None;
    let mut check = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--help" | "-h" => return Ok(Command::Help),
            "--check" => check = true,
            "--config" => {
                let value = args.next().ok_or("--config needs a file name")?;
                config = Some(PathBuf::from(value));
            }
            other => match other.strip_prefix("--config=") {
                Some(value) => config = Some(PathBuf::from(value)),
                None => return Err(format!("unknown argument: {other}")),
            },
        }
    }
    Ok(Command::Run {
        config: config.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH)),
        check,
    })
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Command::Version) => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Run { config, check }) => run(&config, check),
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(path: &std::path::Path, check: bool) -> ExitCode {
    // A configuration error stops startup (fail closed).
    let config = match Config::load(path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if check {
        println!("{}: configuration ok", path.display());
        return ExitCode::SUCCESS;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(serve(&config))
}

/// Serve until SIGTERM or SIGINT, then shut down within the grace period.
async fn serve(config: &Config) -> ExitCode {
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(e) => {
            eprintln!("error: cannot listen for SIGTERM: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(e) => {
            eprintln!("error: cannot listen for SIGINT: {e}");
            return ExitCode::FAILURE;
        }
    };

    let running = match server::start(config).await {
        Ok(running) => running,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let addrs: Vec<String> = running.addrs().iter().map(ToString::to_string).collect();
    eprintln!("{} listening on {}", version_line(), addrs.join(", "));
    for (address, reason) in running.skipped() {
        eprintln!("note: not listening on {address}: {reason}");
    }

    tokio::select! {
        _ = terminate.recv() => eprintln!("received SIGTERM, shutting down"),
        _ = interrupt.recv() => eprintln!("received SIGINT, shutting down"),
    }
    if !running.shutdown(server::SHUTDOWN_GRACE).await {
        eprintln!("note: some queries were still in flight after the grace period");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn version_line_names_the_binary_and_the_crate_version() {
        assert_eq!(
            version_line(),
            format!("hickory-filter {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn defaults_to_the_documented_config_path() {
        assert_eq!(
            parse_args(args(&[])),
            Ok(Command::Run {
                config: PathBuf::from(DEFAULT_CONFIG_PATH),
                check: false
            })
        );
    }

    #[test]
    fn parses_config_check_and_version() {
        assert_eq!(
            parse_args(args(&["--config", "a.toml", "--check"])),
            Ok(Command::Run {
                config: PathBuf::from("a.toml"),
                check: true
            })
        );
        assert_eq!(
            parse_args(args(&["--config=b.toml"])),
            Ok(Command::Run {
                config: PathBuf::from("b.toml"),
                check: false
            })
        );
        assert_eq!(parse_args(args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse_args(args(&["-V"])), Ok(Command::Version));
    }

    #[test]
    fn rejects_unknown_arguments_and_a_missing_config_value() {
        assert!(parse_args(args(&["--nope"])).is_err());
        assert!(parse_args(args(&["--config"])).is_err());
    }
}
