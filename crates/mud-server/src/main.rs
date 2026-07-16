//! Standalone telnet server for the MajorMUD reimplementation.
//!
//! M0 behavior: load the content database, validate it, report, exit.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path: PathBuf = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "re/mmud_wgnt.sqlite".into())
        .into();

    let content = match mud_server::content_db::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    let errors = content.validate();
    println!(
        "loaded {}: {} rooms, {} monsters, {} items, {} spells, {} messages, {} shops, {} races, {} classes",
        path.display(),
        content.rooms.len(),
        content.monsters.len(),
        content.items.len(),
        content.spells.len(),
        content.messages.len(),
        content.shops.len(),
        content.races.len(),
        content.classes.len(),
    );
    if errors.is_empty() {
        println!("validation: OK");
        ExitCode::SUCCESS
    } else {
        for e in &errors {
            eprintln!("validation: {e:?}");
        }
        eprintln!("validation: {} error(s)", errors.len());
        ExitCode::FAILURE
    }
}
