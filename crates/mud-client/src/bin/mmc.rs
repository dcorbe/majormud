use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use mud_client::cli::{Cli, Command};
use mud_client::profile::Profile;
use mud_client::script::run_script;
use mud_client::session::{append_to_stem, Capture, Session};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Play { profile, capture } => {
            let profile = profile.map(|p| mud_client::profile::resolve(&p.to_string_lossy()));
            play_command(profile.as_deref(), capture.as_deref())
        }
        Command::Run {
            script,
            profile,
            capture,
        } => run_command(
            &script,
            &mud_client::profile::resolve(&profile.to_string_lossy()),
            capture.as_deref(),
        ),
        Command::Replay { capture, profile } => replay_command(
            &capture,
            profile
                .map(|p| mud_client::profile::resolve(&p.to_string_lossy()))
                .as_deref(),
        ),
        Command::Path { from, to, content } => path_command(&from, &to, &content),
        Command::Map { at, content } => map_command(&at, &content),
        Command::Import {
            file,
            content,
            start,
        } => import_command(&file, &content, start.as_deref()),
    }
}

fn play_command(
    profile_path: Option<&std::path::Path>,
    capture: Option<&std::path::Path>,
) -> ExitCode {
    let settings = match profile_path {
        Some(path) => match mud_client::settings::Settings::load(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("profile {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => mud_client::settings::Settings::default(),
    };
    let capture = capture.map(std::path::Path::to_path_buf);
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async {
        match mud_client::tui::run(settings, capture).await {
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
    let profile = match Profile::load(profile_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("profile {}: {e}", profile_path.display());
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = profile.require_host() {
        eprintln!("profile {}: {e}", profile_path.display());
        return ExitCode::FAILURE;
    }
    let capture = capture.map(Capture::at);
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

fn map_command(at: &str, content: &std::path::Path) -> ExitCode {
    use mud_client::map::PaintCtx;
    use mud_client::mapview::{MapView, ViewAction, run_offline};

    let graph = match mud_client::graph::RoomGraph::load(content) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let spawns = match mud_client::spawn::SpawnTable::load(content) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // Same resolver `/go` and `/map` use, so a room means one thing
    // whichever way you ask for it.
    let anchor = match mud_client::go::resolve(&graph, None, at) {
        Ok(id) => id,
        Err(refusal) => {
            for line in refusal.lines("go") {
                eprintln!("{line}");
            }
            return ExitCode::FAILURE;
        }
    };
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    // Nothing is known about a character here — there is no character —
    // so nothing is warned about. See `map::PaintCtx`.
    let mut view = MapView::new(
        graph.clone(),
        spawns,
        anchor,
        mud_client::lost::Fix::Unknown,
        PaintCtx::default(),
        (cols as usize, rows as usize),
    );
    match run_offline(&mut view) {
        Err(e) => {
            eprintln!("terminal error: {e}");
            ExitCode::FAILURE
        }
        // The library is shared by every character, so a loop built
        // offline is one `mmc play` can walk straight away.
        Ok(ViewAction::Save(l)) => {
            let dir = mud_client::loops::dir();
            match l.save(&dir) {
                Ok(path) => {
                    println!("saved {} stops as {:?}: {}", l.stops.len(), l.name, path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("loop: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        // `mmc map` holds no connection, so there is nothing to roam
        // with. Report the fence rather than discarding it silently —
        // the marking was real work, and saying so is what tells the
        // operator to do it from inside `mmc play` instead.
        Ok(ViewAction::Roam(walls)) => {
            eprintln!(
                "roam needs a connection; mark those {} walls again inside `mmc play` (r)",
                walls.len()
            );
            ExitCode::FAILURE
        }
        // Nothing to walk with, so the answer is the room itself: enough
        // to paste into a profile, a loop file or a `/go`.
        Ok(ViewAction::Go(way)) => {
            for id in way {
                let name = graph
                    .room(id)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "?".into());
                println!("{}/{}  {name}", id.map, id.room);
            }
            ExitCode::SUCCESS
        }
        // Recovering means sneaking there and walking back, which needs
        // a live connection `mmc map` does not hold.
        Ok(ViewAction::Recover(_)) => {
            eprintln!("recover needs a connection, run it from inside `mmc play` instead");
            ExitCode::FAILURE
        }
        Ok(_) => ExitCode::SUCCESS,
    }
}

fn import_command(
    file: &std::path::Path,
    content: &std::path::Path,
    start: Option<&str>,
) -> ExitCode {
    use mud_client::mega::{Index, import, parse_mp};

    let graph = match mud_client::graph::RoomGraph::load(content) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let mp = match parse_mp(&text) {
        Ok(mp) => mp,
        Err(e) => {
            eprintln!("{}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let start = match start.map(|s| {
        mud_client::farm::parse_room_id(s)
            .ok_or_else(|| format!("--start wants map/room, got {s:?}"))
    }) {
        Some(Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
        Some(Ok(id)) => Some(id),
        None => None,
    };
    let (l, report) = match import(&graph, &Index::build(&graph), &mp, start) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e}");
            // An ambiguous start is answerable; say how.
            if matches!(e, mud_client::mega::ImportError::AmbiguousStart { .. }) {
                eprintln!("re-run with --start map/room to pick one");
            }
            return ExitCode::FAILURE;
        }
    };
    for line in report.lines() {
        println!("{line}");
    }
    if l.stops.len() < 2 {
        eprintln!("nothing walkable here; not saved");
        return ExitCode::FAILURE;
    }
    match l.save(&mud_client::loops::dir()) {
        Ok(path) => {
            println!("saved {} stops as {:?}: {}", l.stops.len(), l.name, path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("loop: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Replay a capture's board output through the bot and report command
/// loops. Reads no network: it feeds the recorded board stream to a
/// fresh bot and prints where today's code would spin (see
/// `crate::replay`). A `.raw` keeps the board's colour, which the bot
/// needs to read a room block, so it is the better input; a
/// `_timing.log` works for the coarser loops but has its colour stripped.
fn replay_command(capture: &std::path::Path, profile: Option<&std::path::Path>) -> ExitCode {
    use mud_client::replay::{board_output, find_loops, replay, DEFAULT_MAX_DISTINCT, DEFAULT_MIN_RUN};

    let bytes = match std::fs::read(capture) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", capture.display());
            return ExitCode::FAILURE;
        }
    };
    let text = String::from_utf8_lossy(&bytes);

    let config = match profile {
        Some(path) => match Profile::load(path) {
            Ok(p) => p.bot.unwrap_or_default(),
            Err(e) => {
                eprintln!("profile {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => mud_client::bot::BotConfig {
            auto_combat: true,
            ..Default::default()
        },
    };

    let trace = replay(&board_output(&text), config);
    let loops = find_loops(&trace, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    println!(
        "{}: {} commands emitted",
        capture.display(),
        trace.commands.len()
    );
    if loops.is_empty() {
        println!("no command loops");
        return ExitCode::SUCCESS;
    }
    for finding in &loops {
        println!(
            "loop at event {}: {} commands from {:?}",
            finding.at_event, finding.count, finding.commands
        );
    }
    // A found loop is the failure the tool exists to surface, so it is
    // reported in the exit code too — a scripted sweep over the captures
    // can fail on it without parsing the text.
    ExitCode::FAILURE
}

