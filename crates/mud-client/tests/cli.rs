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
fn path_subcommand_takes_rooms_and_db() {
    use clap::Parser;
    let cli = Cli::parse_from(["mmc", "path", "1/1", "15/982"]);
    match cli.command {
        mud_client::cli::Command::Path { from, to, content } => {
            assert_eq!(from, "1/1");
            assert_eq!(to, "15/982");
            assert_eq!(content.to_str(), Some("re/mmud_wgnt.sqlite"));
        }
        _ => panic!("expected path subcommand"),
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

#[test]
fn farm_subcommand_takes_profile_capture_and_content() {
    use clap::Parser;
    let cli = Cli::parse_from([
        "mmc",
        "farm",
        "--profile",
        "chars/nav.toml",
        "--capture",
        "out/farm1",
        "--content",
        "re/other.sqlite",
    ]);
    match cli.command {
        mud_client::cli::Command::Farm {
            profile,
            capture,
            content,
        } => {
            assert_eq!(profile.to_str(), Some("chars/nav.toml"));
            assert_eq!(capture.as_deref().and_then(|p| p.to_str()), Some("out/farm1"));
            assert_eq!(content.as_deref().and_then(|p| p.to_str()), Some("re/other.sqlite"));
        }
        _ => panic!("expected farm subcommand"),
    }
}

/// Without --content the room database comes from [farm].content in the
/// profile, so the flag has no default of its own to disagree with it.
#[test]
fn farm_content_defaults_to_the_profile() {
    use clap::Parser;
    let cli = Cli::parse_from(["mmc", "farm", "--profile", "chars/nav.toml"]);
    match cli.command {
        mud_client::cli::Command::Farm { content, .. } => assert_eq!(content, None),
        _ => panic!("expected farm subcommand"),
    }
}
