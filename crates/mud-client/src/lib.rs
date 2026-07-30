//! mud-client: automated MajorMUD client.
//!
//! One engine, three uses: interactive terminal play with bot toggles,
//! headless Lua-scripted oracle runs, and DB-driven navigation. Targets
//! both the live MBBSEmu board (WCCMMUD 1.11p, CP437 + ANSI + anti-bot
//! backspace obfuscation) and the in-repo `mud-server` reimplementation.

pub mod bot;
pub mod cli;
pub mod dialect;
pub mod events;
pub mod farm;
pub mod graph;
pub mod nav;
pub mod parse;
pub mod profile;
pub mod progress;
pub mod script;
pub mod session;
pub mod tui;
pub mod wire;
