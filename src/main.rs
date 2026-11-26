//! hickory-filter: a filtering DNS forwarder.

use std::process::ExitCode;

/// The line printed for `--version`: `hickory-filter X.Y.Z`.
fn version_line() -> String {
    format!("hickory-filter {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: hickory-filter --version");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_the_binary_and_the_crate_version() {
        assert_eq!(
            version_line(),
            format!("hickory-filter {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
