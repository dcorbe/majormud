//! Command-line surface for the `mmc` binary.
//!
//! Subcommand arguments grow per milestone; each stays minimal until the
//! slice that implements it lands.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mmc", version, about = "MajorMUD client: play, bot, oracle")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Interactive session against a target board
    Play {
        /// Character profile (TOML)
        #[arg(long)]
        profile: PathBuf,
        /// Capture basename: writes `<capture>.raw` and `<capture>_timing.log`
        #[arg(long)]
        capture: Option<PathBuf>,
    },
    /// Run a Lua oracle/bot script headless
    Run {
        /// Lua script to execute
        script: PathBuf,
        /// Character profile (TOML)
        #[arg(long)]
        profile: PathBuf,
        /// Capture basename: writes `<capture>.raw` and
        /// `<capture>_timing.log`; sections land in `<capture>_sections.json`
        #[arg(long)]
        capture: Option<PathBuf>,
    },
    /// Compute a route between two rooms (map/room, e.g. 1/1)
    Path {
        /// Start room as map/room
        from: String,
        /// Target room as map/room
        to: String,
        /// Room database (decoded WG3-NT sqlite)
        #[arg(long, default_value = "re/mmud_wgnt.sqlite")]
        content: PathBuf,
    },
    /// Walk the profile's patrol circuit, farming each stop
    Farm {
        /// Character profile (TOML); needs a `[farm]` table
        #[arg(long)]
        profile: PathBuf,
        /// Capture basename: writes `<capture>.raw` and `<capture>_timing.log`
        #[arg(long)]
        capture: Option<PathBuf>,
        /// Room database, overriding the profile's `[farm].content`
        #[arg(long)]
        content: Option<PathBuf>,
        /// Echo every line the board sends, not just notable ones
        #[arg(long)]
        watch: bool,
        /// Print nothing until the run ends
        #[arg(long, conflicts_with = "watch")]
        quiet: bool,
    },
}
