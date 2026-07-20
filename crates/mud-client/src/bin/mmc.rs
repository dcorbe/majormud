use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use mud_client::cli::{Cli, Command};
use mud_client::profile::Profile;
use mud_client::script::run_script;
use mud_client::session::{Capture, Session};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Play { profile, capture } => play_command(&profile, capture.as_deref()),
        Command::Run {
            script,
            profile,
            capture,
        } => run_command(&script, &profile, capture.as_deref()),
        Command::Path { from, to, content } => path_command(&from, &to, &content),
        Command::Farm => {
            eprintln!("mmc farm: not implemented yet (C8)");
            ExitCode::FAILURE
        }
    }
}

fn play_command(profile_path: &std::path::Path, capture: Option<&std::path::Path>) -> ExitCode {
    let profile: Profile = match std::fs::read_to_string(profile_path)
        .map_err(|e| e.to_string())
        .and_then(|s| toml::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("profile {}: {e}", profile_path.display());
            return ExitCode::FAILURE;
        }
    };
    let capture = capture.map(|base| Capture {
        raw: base.with_extension("raw"),
        timing: Some(append_to_stem(base, "_timing.log")),
    });
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async {
        let session = match Session::connect(&profile, capture).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                eprintln!("connect {}:{}: {e}", profile.host, profile.port);
                return ExitCode::FAILURE;
            }
        };
        match mud_client::tui::play(session).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("terminal error: {e}");
                ExitCode::FAILURE
            }
        }
    })
}

fn run_command(
    script: &std::path::Path,
    profile_path: &std::path::Path,
    capture: Option<&std::path::Path>,
) -> ExitCode {
    let profile: Profile = match std::fs::read_to_string(profile_path)
        .map_err(|e| e.to_string())
        .and_then(|s| toml::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("profile {}: {e}", profile_path.display());
            return ExitCode::FAILURE;
        }
    };
    let capture = capture.map(|base| Capture {
        raw: base.with_extension("raw"),
        timing: Some(append_to_stem(base, "_timing.log")),
    });
    let sections_path = capture
        .as_ref()
        .map(|c| append_to_stem(&c.raw.with_extension(""), "_sections.json"));

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async {
        let session = match Session::connect(&profile, capture).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                eprintln!("connect {}:{}: {e}", profile.host, profile.port);
                return ExitCode::FAILURE;
            }
        };
        match run_script(script, session).await {
            Ok(outcome) => {
                if let Some(path) = sections_path {
                    match serde_json::to_string_pretty(&outcome.sections) {
                        Ok(json) => {
                            if let Err(e) = std::fs::write(&path, json + "\n") {
                                eprintln!("write {}: {e}", path.display());
                                return ExitCode::FAILURE;
                            }
                            eprintln!("sections: {}", path.display());
                        }
                        Err(e) => {
                            eprintln!("serialize sections: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    for name in outcome.sections.keys() {
                        eprintln!("section captured: {name}");
                    }
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("script failed: {e}");
                ExitCode::FAILURE
            }
        }
    })
}

fn path_command(from: &str, to: &str, content: &std::path::Path) -> ExitCode {
    fn parse_room(s: &str) -> Option<mud_core::content::RoomId> {
        let (map, room) = s.split_once('/')?;
        Some(mud_core::content::RoomId {
            map: map.parse().ok()?,
            room: room.parse().ok()?,
        })
    }
    let (Some(from), Some(to)) = (parse_room(from), parse_room(to)) else {
        eprintln!("rooms must be map/room, e.g. 1/1");
        return ExitCode::FAILURE;
    };
    let graph = match mud_client::graph::RoomGraph::load(content) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let name = |id| {
        graph
            .room(id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "?".into())
    };
    match graph.route(from, to) {
        None => {
            eprintln!("no route from {}/{} to {}/{}", from.map, from.room, to.map, to.room);
            ExitCode::FAILURE
        }
        Some(steps) => {
            println!(
                "{} steps: {} -> {}",
                steps.len(),
                name(from),
                name(to)
            );
            let words: Vec<&str> = steps.iter().map(|&d| mud_client::nav::dir_word(d)).collect();
            println!("{}", words.join(" "));
            ExitCode::SUCCESS
        }
    }
}

/// "out/run1" + "_timing.log" -> "out/run1_timing.log"
fn append_to_stem(base: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut s = base.as_os_str().to_os_string();
    s.push(suffix);
    std::path::PathBuf::from(s)
}
