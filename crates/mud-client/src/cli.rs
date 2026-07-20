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
    Play,
    /// Run a Lua oracle/bot script headless
    Run {
        /// Lua script to execute
        script: PathBuf,
        /// Character profile (TOML)
        #[arg(long)]
        profile: PathBuf,
        /// Capture basename: writes <capture>.raw and
        /// <capture>_timing.log; sections land in <capture>_sections.json
        #[arg(long)]
        capture: Option<PathBuf>,
    },
    /// Compute a route between two rooms
    Path,
    /// Run a farming loop over a set of spawn rooms
    Farm,
}
