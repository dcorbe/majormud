//! Command-line surface for the `mmc` binary.
//!
//! Subcommand arguments grow per milestone; each stays minimal until the
//! slice that implements it lands.

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
    Run,
    /// Compute a route between two rooms
    Path,
    /// Run a farming loop over a set of spawn rooms
    Farm,
}
