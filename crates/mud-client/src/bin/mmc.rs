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
        Command::Farm {
            profile,
            capture,
            content,
            watch,
            quiet,
        } => farm_command(&profile, capture.as_deref(), content.as_deref(), watch, quiet),
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
    use mud_client::farm::parse_room_id;
    let (Some(from), Some(to)) = (parse_room_id(from), parse_room_id(to)) else {
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

fn farm_command(
    profile_path: &std::path::Path,
    capture: Option<&std::path::Path>,
    content: Option<&std::path::Path>,
    watch: bool,
    quiet: bool,
) -> ExitCode {
    use mud_client::farm::{FarmEnd, FarmPlan, go_to_finish, run_farm};

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
    let Some(farm_config) = profile.farm.clone() else {
        eprintln!(
            "profile {} has no [farm] table: nothing to patrol",
            profile_path.display()
        );
        return ExitCode::FAILURE;
    };
    let bot_config = profile.bot.clone().unwrap_or_default();

    // Load the graph and validate the whole circuit before connecting:
    // a typo should cost nothing more than an error message.
    let db = content.unwrap_or(&farm_config.content);
    let graph = match mud_client::graph::RoomGraph::load(db) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let plan = match FarmPlan::build(&farm_config, &graph) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[farm]: {e}");
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
        // Started before the login dance, so a run that stalls on the
        // way in is visible too rather than looking like a silent hang.
        if !quiet {
            let mut events = session.events();
            let mut view = mud_client::progress::ProgressView::new(watch);
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(ev) => {
                            if let Some(line) = view.on_event(&ev) {
                                println!("{line}");
                            }
                        }
                        // A lagged feed has missed lines, but the run is
                        // fine; say so and carry on rather than going
                        // quiet for the rest of the session.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            println!("!! progress feed dropped {n} events");
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        match mud_client::dialect::login(&session, &profile).await {
            Ok(mud_client::dialect::LoginOutcome::InGame) => {}
            Ok(mud_client::dialect::LoginOutcome::CharacterCreation) => {
                if let Err(e) = mud_client::dialect::finish_creation(&session).await {
                    eprintln!("character creation: {e}");
                    return ExitCode::FAILURE;
                }
            }
            Err(e) => {
                eprintln!("login: {e}");
                return ExitCode::FAILURE;
            }
        }

        // Ctrl-C stops the patrol. There is no session close API, and
        // sending "x" would mean waiting out exit meditation while the
        // operator has already asked to stop — but the character is not
        // simply abandoned where it stands any more; see below.
        let outcome = tokio::select! {
            r = run_farm(&session, graph.clone(), &plan, &bot_config, &farm_config) => Some(r),
            _ = tokio::signal::ctrl_c() => None,
        };

        // Say why the run stopped BEFORE walking home, so the two read
        // in the order they happened. Printing the walk first made
        // "interrupted" look like it was the walk that got interrupted.
        match &outcome {
            None => eprintln!("interrupted"),
            Some(Ok((end, stats))) => println!(
                "{end:?}: {} kills, {} laps, {} flees, {} slowdowns, {} interrupts",
                stats.kills, stats.loops, stats.flees, stats.slowdowns, stats.interrupts
            ),
            Some(Err(e)) => eprintln!("farm: {e}"),
        }

        // Walk home on every route out of the run, interrupted included.
        // Ctrl-C is the commonest way a farm ends, so a finish walk that
        // only ran on a clean finish would miss the case that matters.
        // Skipped only for a death, which cannot walk anywhere.
        let died = matches!(outcome, Some(Ok((FarmEnd::Died, _))));
        if !died && plan.finish.is_some() {
            match go_to_finish(&session, graph.clone(), &plan, &farm_config).await {
                Ok(()) => println!("walked to the finish room"),
                // Worth saying loudly: the character is still out there.
                Err(e) => eprintln!("could not walk to the finish room: {e}"),
            }
        }

        match outcome {
            None | Some(Err(_)) => ExitCode::FAILURE,
            // Both mean the patrol stopped short because the character
            // could not go on.
            Some(Ok((FarmEnd::Died | FarmEnd::TooHurt, _))) => ExitCode::FAILURE,
            Some(Ok(_)) => ExitCode::SUCCESS,
        }
    })
}

/// "out/run1" + "_timing.log" -> "out/run1_timing.log"
fn append_to_stem(base: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut s = base.as_os_str().to_os_string();
    s.push(suffix);
    std::path::PathBuf::from(s)
}
