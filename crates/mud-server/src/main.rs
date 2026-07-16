//! Standalone telnet server for the MajorMUD reimplementation.
//!
//! Usage: mud-server [--content <path>] [--state <path>] [--listen <addr>]

use std::path::PathBuf;
use std::process::ExitCode;

use mud_core::game::CoreConfig;
use mud_server::{content_db, server::Server, state_db::StateDb};

struct Args {
    content: PathBuf,
    state: PathBuf,
    listen: String,
    /// Dev fixture spawns until the M6 spawner: "id@map,room", repeatable.
    spawns: Vec<(u16, u16, u16)>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        content: "re/mmud_wgnt.sqlite".into(),
        state: "state.sqlite".into(),
        listen: "0.0.0.0:2325".into(),
        spawns: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--content" => args.content = value("--content")?.into(),
            "--state" => args.state = value("--state")?.into(),
            "--listen" => args.listen = value("--listen")?,
            "--spawn" => {
                let v = value("--spawn")?;
                let (id, loc) = v
                    .split_once('@')
                    .ok_or_else(|| format!("--spawn wants id@map,room, got {v}"))?;
                let (map, room) = loc
                    .split_once(',')
                    .ok_or_else(|| format!("--spawn wants id@map,room, got {v}"))?;
                args.spawns.push((
                    id.parse().map_err(|_| format!("bad monster id {id}"))?,
                    map.parse().map_err(|_| format!("bad map {map}"))?,
                    room.parse().map_err(|_| format!("bad room {room}"))?,
                ));
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let content = match content_db::load(&args.content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load {}: {e}", args.content.display());
            return ExitCode::FAILURE;
        }
    };
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

    let state = match StateDb::open(&args.state) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open {}: {e}", args.state.display());
            return ExitCode::FAILURE;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let server = match Server::start_with_spawns(
            content,
            CoreConfig::default(),
            state,
            &args.listen,
            args.spawns.clone(),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("failed to listen on {}: {e}", args.listen);
                return ExitCode::FAILURE;
            }
        };
        println!("listening on {}", server.local_addr());
        // Serve until killed.
        std::future::pending::<()>().await;
        ExitCode::SUCCESS
    })
}
