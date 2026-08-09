//! Standalone telnet server for the MajorMUD reimplementation.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use mud_core::game::CoreConfig;
use mud_server::{content_db, server::Server, state_db::StateDb};

#[derive(Parser, Debug)]
#[command(name = "mud-server")]
struct Cli {
    /// Room/monster/item/spell database (decoded WG3-NT sqlite)
    #[arg(long, default_value = "re/mmud_wgnt.sqlite")]
    content: PathBuf,

    /// Player state database
    #[arg(long, default_value = "state.sqlite")]
    state: PathBuf,

    /// Address to bind
    #[arg(long, default_value = "0.0.0.0:2325")]
    listen: String,

    /// Guild-house description files (gangs.md §4) -- every file in the
    /// directory is preloaded, keyed by uppercase filename
    #[arg(long, default_value = "re/hse_files")]
    houses: PathBuf,

    /// Dev fixture spawn as id@map,room, repeatable (the M6 spawner runs
    /// regardless; fixtures are the test/staging placement path)
    #[arg(long = "spawn", value_name = "id@map,room", value_parser = parse_spawn)]
    spawns: Vec<(u16, u16, u16)>,

    /// Send direction words plainly instead of hiding the board's anti-bot
    /// junk character and backspace inside them. Useful when reading a raw
    /// capture by eye.
    #[arg(long)]
    no_wire_noise: bool,
}

/// Parse a `--spawn` value of the form `id@map,room`.
fn parse_spawn(s: &str) -> Result<(u16, u16, u16), String> {
    let (id, loc) = s.split_once('@').ok_or_else(|| format!("wants id@map,room, got {s}"))?;
    let (map, room) = loc.split_once(',').ok_or_else(|| format!("wants id@map,room, got {s}"))?;
    Ok((
        id.parse().map_err(|_| format!("bad monster id {id}"))?,
        map.parse().map_err(|_| format!("bad map {map}"))?,
        room.parse().map_err(|_| format!("bad room {room}"))?,
    ))
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut content = match content_db::load(&cli.content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load {}: {e}", cli.content.display());
            return ExitCode::FAILURE;
        }
    };
    // Guild-house files are optional world data: a missing directory
    // just means FILE DESCRIPTION rooms render without paragraphs (the
    // DLL's sysop-error case).
    match content_db::load_house_dir(&cli.houses) {
        Ok(houses) => {
            let n = houses.len();
            for (name, lines) in houses {
                content.add_house_text(&name, lines);
            }
            if n > 0 {
                println!("loaded {n} guild-house files from {}", cli.houses.display());
            }
        }
        Err(e) => eprintln!("--houses {}: {e} (continuing without)", cli.houses.display()),
    }
    let errors = content.validate();
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("validation: {e:?}");
        }
        eprintln!("content failed validation: {} error(s)", errors.len());
        return ExitCode::FAILURE;
    }
    println!(
        "content OK: {} rooms, {} monsters, {} items, {} spells",
        content.rooms.len(),
        content.monsters.len(),
        content.items.len(),
        content.spells.len(),
    );

    let state = match StateDb::open(&cli.state) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open {}: {e}", cli.state.display());
            return ExitCode::FAILURE;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let server = match Server::start_with_spawns(
            content,
            CoreConfig {
                // The live server speaks ANSI (the MBBS graphics
                // setting); a per-user toggle can arrive later.
                ansi: true,
                // Commands occupy their player for a round, so a second
                // one sent inside it queues and is echoed again when it
                // runs. MEASURED at 1.15s and 1.25s between consecutive
                // queued moves (oracle_blur_duration_timing.log); the
                // client saw up to ~3s on a board busy with combat. One
                // second is the closest our 1 Hz gate can sit to the
                // measurement, and a single uniform round is an
                // approximation of a scheduler that paces each command
                // differently. Fixtures leave this at 0.
                command_round_seconds: 1,
                // The board hides a junk character and a backspace in
                // every direction word; period clients expect it, and a
                // terminal renders it away. --no-wire-noise turns it off
                // for anyone reading the stream by eye.
                wire_noise: !cli.no_wire_noise,
                ..CoreConfig::default()
            },
            state,
            &cli.listen,
            cli.spawns,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("failed to listen on {}: {e}", cli.listen);
                return ExitCode::FAILURE;
            }
        };
        println!("listening on {}", server.local_addr());
        // Serve until killed.
        std::future::pending::<()>().await;
        ExitCode::SUCCESS
    })
}
