//! Investigation tool: replay a capture pair (`<name>.raw` +
//! `<name>_timing.log`) through the wire pipeline, the correlator and
//! the play-mode assist bot, and print what the assist would have sent
//! at every event. `cargo run -p mud-client --example replay_assist --
//! <name> <profile.toml>`
use mud_client::bot::{Bot, BotConfig};
use mud_client::correlate::{CmdId, Correlator};
use mud_client::events::Event;
use mud_client::parse::Parser;
use mud_client::tui::assist_actions;
use mud_client::wire::{cp437_to_string, TelnetFilter};
use mud_client::world::Here;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().expect("usage: replay_assist <capture-basename> <profile.toml>");
    let profile = args.next().expect("usage: replay_assist <capture-basename> <profile.toml>");
    let raw = std::fs::read(format!("{name}.raw")).expect("read raw");
    let timing = std::fs::read_to_string(format!("{name}_timing.log")).expect("read timing");
    let profile = mud_client::profile::Profile::load(std::path::Path::new(&profile)).expect("profile");
    let mut cfg = profile.bot.clone().unwrap_or(BotConfig { auto_combat: true, auto_get: true, ..Default::default() });
    cfg.auto_heal = false;
    cfg.auto_flee = false;
    // One loot owner, `here`, exactly as `new_assist` builds the real
    // play-mode bot: `auto_get` off so this tool's sweep line matches
    // what the window actually sends.
    cfg.auto_get = false;
    let mut bot = Bot::new(cfg);
    // Folded exactly as the window loop folds it, so this tool's own
    // "-> [...]" line shows the loot sweep the assist now takes from
    // the model rather than from `auto_get`.
    let mut here = Here::default();

    // Raw lines in wire order, each with its terminator kept.
    let mut raw_lines: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    for (i, b) in raw.iter().enumerate() {
        if *b == b'\n' {
            raw_lines.push(&raw[start..=i]);
            start = i + 1;
        }
    }
    if start < raw.len() {
        raw_lines.push(&raw[start..]);
    }
    let mut next_raw = 0usize;

    let mut filter = TelnetFilter::new();
    let mut parser = Parser::new();
    let mut cor = Correlator::new(Duration::from_secs(20));
    let base = Instant::now();
    let mut t0: Option<f64> = None;
    let mut id = 0u64;

    for entry in timing.lines() {
        let Some((ts, rest)) = entry.split_once(' ') else { continue };
        let ts: f64 = ts.parse().expect("timestamp");
        let t0v = *t0.get_or_insert(ts);
        let now = base + Duration::from_secs_f64(ts - t0v);
        if let Some(cmd) = rest.strip_prefix("TX ") {
            id += 1;
            cor.sent(CmdId(id), cmd.trim(), now);
            println!("{:>9.3} TX#{id} {cmd:?}", ts - t0v);
            continue;
        }
        if rest.strip_prefix("RX ").is_none() {
            continue;
        }
        let Some(bytes) = raw_lines.get(next_raw) else { println!("!! raw exhausted"); break };
        next_raw += 1;
        let out = filter.push(bytes);
        let decoded = cp437_to_string(&out.data);
        for ev in parser.push(&decoded) {
            let c = cor.on_event(ev, now);
            here.on_event(&c, now);
            let actions = assist_actions(&mut bot, &mut here, &c, false);
            match &c.event {
                Event::RoomSeen(r) => println!(
                    "{:>9.3} ROOM {:?} here={:?} items={:?} sgr={:?} answers={:?} elsewhere={} engaged={:?} -> {:?}",
                    ts - t0v, r.name, r.also_here, r.items, r.also_here_sgr, c.answers, c.elsewhere, bot.engaged(), actions
                ),
                Event::ActorEntered { .. } => println!("{:>9.3} {:?} -> {:?}", ts - t0v, c.event, actions),
                _ if !actions.is_empty() => println!("{:>9.3} {:?} -> {:?}", ts - t0v, c.event, actions),
                _ => {}
            }
        }
    }
    println!("raw lines consumed {next_raw} of {}", raw_lines.len());
}
