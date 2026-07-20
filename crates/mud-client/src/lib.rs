//! mud-client: automated MajorMUD client.
//!
//! One engine, three uses: interactive terminal play with bot toggles,
//! headless Lua-scripted oracle runs, and DB-driven navigation. Targets
//! both the live MBBSEmu board (WCCMMUD 1.11p, CP437 + ANSI + anti-bot
//! backspace obfuscation) and the in-repo `mud-server` reimplementation.

pub mod cli;
pub mod wire;
