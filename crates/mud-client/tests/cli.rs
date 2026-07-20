//! CLI surface tests: the `mmc` command definition is valid and exposes
//! the four entry points (play, run, path, farm).

use clap::CommandFactory;
use mud_client::cli::Cli;

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn run_subcommand_takes_script_profile_and_capture() {
    use clap::Parser;
    let cli = Cli::parse_from([
        "mmc",
        "run",
        "scripts/oracle_directions.lua",
        "--profile",
        "chars/oracle.toml",
        "--capture",
        "out/run1",
    ]);
    match cli.command {
        mud_client::cli::Command::Run {
            script,
            profile,
            capture,
        } => {
            assert_eq!(script.to_str(), Some("scripts/oracle_directions.lua"));
            assert_eq!(profile.to_str(), Some("chars/oracle.toml"));
            assert_eq!(capture.as_deref().and_then(|p| p.to_str()), Some("out/run1"));
        }
        _ => panic!("expected run subcommand"),
    }
}

#[test]
fn cli_has_expected_subcommands() {
    let cmd = Cli::command();
    let subs: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for expected in ["play", "run", "path", "farm"] {
        assert!(
            subs.contains(&expected.to_string()),
            "missing subcommand {expected}"
        );
    }
}
