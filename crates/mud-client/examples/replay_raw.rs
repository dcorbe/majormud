//! Investigation tool: replay a .raw capture through the wire pipeline
//! and dump every parsed event. `cargo run -p mud-client --example
//! replay_raw -- <file.raw>`
use mud_client::parse::Parser;
use mud_client::wire::{cp437_to_string, TelnetFilter};

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay_raw <file.raw>");
    let bytes = std::fs::read(&path).expect("read capture");
    let mut filter = TelnetFilter::new();
    let mut parser = Parser::new();
    let mut n = 0usize;
    let mut emit = |ev: mud_client::events::Event| {
        n += 1;
        println!("{n:4} {ev:?}");
    };
    for chunk in bytes.chunks(512) {
        let out = filter.push(chunk);
        if out.data.is_empty() {
            continue;
        }
        let decoded = cp437_to_string(&out.data);
        for ev in parser.push(&decoded) {
            emit(ev);
        }
    }
    for ev in parser.finish() {
        emit(ev);
    }
}
