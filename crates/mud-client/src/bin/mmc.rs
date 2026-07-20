use clap::Parser;
use mud_client::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Play => eprintln!("mmc play: not implemented yet (C5)"),
        Command::Run => eprintln!("mmc run: not implemented yet (C4)"),
        Command::Path => eprintln!("mmc path: not implemented yet (C6)"),
        Command::Farm => eprintln!("mmc farm: not implemented yet (C8)"),
    }
}
