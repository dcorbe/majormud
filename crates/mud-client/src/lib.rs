//! mud-client: automated MajorMUD client.
//!
//! One engine, three uses: interactive terminal play with bot toggles,
//! headless Lua-scripted oracle runs, and DB-driven navigation. Targets
//! both the live MBBSEmu board (WCCMMUD 1.11p, CP437 + ANSI + anti-bot
//! backspace obfuscation) and the in-repo `mud-server` reimplementation.

pub mod bot;
pub mod cli;
pub mod correlate;
pub mod deaths;
pub mod dialect;
pub mod events;
pub mod farm;
pub mod go;
pub mod graph;
pub mod loops;
pub mod lost;
pub mod map;
pub mod mega;
pub mod mapview;
pub mod nav;
pub mod parse;
pub mod profile;
pub mod progress;
pub mod purse;
pub mod roam;
pub mod script;
pub mod session;
pub mod sheet;
pub mod spawn;
pub mod tui;
pub mod wire;
pub mod world;
