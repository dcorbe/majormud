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
    /// Interactive session against a target board, or the lobby when no
    /// profile is given
    Play {
        /// Character profile: a name under ~/.config/mmc, or a path.
        /// Without one the client starts in the lobby: `/connect
        /// host[:port]`, then `/save <name>`.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Capture basename: writes `<capture>.raw` and `<capture>_timing.log`
        #[arg(long)]
        capture: Option<PathBuf>,
    },
    /// Run a Lua oracle/bot script headless
    Run {
        /// Lua script to execute
        script: PathBuf,
        /// Character profile: a name under ~/.config/mmc, or a path
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
    /// Browse the world map without connecting to a board
    Map {
        /// Room to open on: `map/room` (e.g. 1/1076) or part of a name
        at: String,
        /// Room database (decoded WG3-NT sqlite)
        #[arg(long, default_value = "re/mmud_wgnt.sqlite")]
        content: PathBuf,
    },
    /// Import a MegaMud `.mp` path into the loop library
    Import {
        /// The `.mp` file to read
        file: PathBuf,
        /// Room database (decoded WG3-NT sqlite)
        #[arg(long, default_value = "re/mmud_wgnt.sqlite")]
        content: PathBuf,
        /// Start room as `map/room`, when the file's own start is
        /// ambiguous
        #[arg(long)]
        start: Option<String>,
    },
    /// Walk the profile's patrol circuit, farming each stop
    Farm {
        /// Character profile: a name under ~/.config/mmc, or a path.
        /// Needs a `[farm]` table
        #[arg(long)]
        profile: PathBuf,
        /// Capture basename: writes `<capture>.raw` and `<capture>_timing.log`
        #[arg(long)]
        capture: Option<PathBuf>,
        /// Room database, overriding the profile's `[farm].content`
        #[arg(long)]
        content: Option<PathBuf>,
        /// Show only notable lines instead of everything the board sends
        #[arg(long)]
        brief: bool,
        /// Print nothing until the run ends
        #[arg(long, conflicts_with = "brief")]
        quiet: bool,
    },
}

