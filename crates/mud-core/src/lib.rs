//! MajorMUD engine core.
//!
//! Pure game logic: deterministic given (state, input sequence, RNG seed).
//! No I/O — loading, networking, and persistence live in `mud-server`.
//! Behavior follows the spec in `re/docs/`.

pub mod ability;
pub mod combat;
pub mod command;
pub mod content;
pub mod crime;
pub mod game;
pub mod questvm;
pub mod stats;
pub mod text;
pub mod tick;
