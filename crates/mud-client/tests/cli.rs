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
    for expected in ["play", "run", "path", "replay"] {
        assert!(
            subs.contains(&expected.to_string()),
            "missing subcommand {expected}"
        );
    }
}

#[test]
fn replay_subcommand_takes_a_capture_and_an_optional_profile() {
    use clap::Parser;
    let cli = Cli::parse_from(["mmc", "replay", "out/cw-beef.raw", "--profile", "chars/beef.toml"]);
    match cli.command {
        mud_client::cli::Command::Replay { capture, profile } => {
            assert_eq!(capture.to_str(), Some("out/cw-beef.raw"));
            assert_eq!(profile.as_deref().and_then(|p| p.to_str()), Some("chars/beef.toml"));
        }
        _ => panic!("expected replay subcommand"),
    }

    let cli = Cli::parse_from(["mmc", "replay", "out/cw-beef.raw"]);
    match cli.command {
        mud_client::cli::Command::Replay { profile, .. } => assert_eq!(profile, None),
        _ => panic!("expected replay subcommand"),
    }
}

#[test]
fn play_without_a_profile_opens_the_lobby() {
    use clap::Parser;
    use mud_client::cli::Command;
    let cli = Cli::parse_from(["mmc", "play"]);
    match cli.command {
        Command::Play { profile, capture } => {
            assert!(profile.is_none());
            assert!(capture.is_none());
        }
        _ => panic!("expected play"),
    }
    let cli = Cli::parse_from(["mmc", "play", "--profile", "chars/dan.toml"]);
    match cli.command {
        Command::Play { profile, .. } => assert_eq!(profile.unwrap().to_str(), Some("chars/dan.toml")),
        _ => panic!("expected play"),
    }
}

