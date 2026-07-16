//! The shared ability enum — generated at build time from
//! `re/docs/ability_ids.tsv` by `build.rs`.

include!(concat!(env!("OUT_DIR"), "/ability_generated.rs"));
