//! MajorMUD engine core.
//!
//! Pure game logic: deterministic given (state, input sequence, RNG seed).
//! No I/O — loading, networking, and persistence live in `mud-server`.
//! Behavior follows the spec in `re/docs/`.
