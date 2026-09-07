//! mud-client: automated MajorMUD client.
//!
//! One engine, three uses: interactive terminal play with bot toggles,
//! headless Lua-scripted oracle runs, and DB-driven navigation. Targets
//! both the live MBBSEmu board (WCCMMUD 1.11p, CP437 + ANSI + anti-bot
//! backspace obfuscation) and the in-repo `mud-server` reimplementation.

pub mod backstab;
pub mod bank;
pub mod bot;
pub mod cli;
pub mod correlate;
pub mod deathlog;
pub mod deaths;
pub mod dialect;
pub mod equipment;
pub mod events;
pub mod farm;
pub mod go;
pub mod graph;
pub mod items;
pub mod loops;
pub mod lost;
pub mod map;
pub mod mega;
pub mod mapview;
pub mod nav;
pub mod pack;
pub mod parse;
pub mod profile;
pub mod progress;
pub mod purse;
pub mod puzzle;
pub mod roam;
pub mod screen;
pub mod script;
pub mod session;
pub mod settings;
pub mod sheet;
pub mod spawn;
pub mod stats;
pub mod tui;
pub mod views;
pub mod window;
pub mod wire;
pub mod world;

