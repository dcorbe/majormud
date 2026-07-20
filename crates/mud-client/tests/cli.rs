//! CLI surface tests: the `mmc` command definition is valid and exposes
//! the four entry points (play, run, path, farm).

use clap::CommandFactory;
use mud_client::cli::Cli;

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
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
