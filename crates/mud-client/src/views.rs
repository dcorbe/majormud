//! By-name lookups over `Content`, standing in for `graph.rs`'s
//! hand-written `monster`/`spell` SQL (`2026-08-22-one-path-to-content`
//! Task 3). `Content` keys both tables by numeric id; these build the
//! lowercased-name indices the client actually needs, in one pass, at
//! whatever moment the caller has a loaded `Content` in hand.
//!
//! Row filtering that used to live in the SQL `WHERE` clause moves here
//! verbatim -- see each function's doc for the exact rule it reproduces.

use std::collections::BTreeMap;

use mud_core::content::Content;

use crate::bot::ThreatTable;

/// How dangerous each monster template is, keyed by lowercase name.
///
/// Reproduces `graph.rs`'s old `select lower(name), experience,
/// hitpoints from monster where name != ''`: blank names are skipped (1
/// such row shipped), the score is `experience*1000 + hitpoints`, and
/// the HIGHEST score wins a name collision (7 rows share "giant rat"),
/// so a shared name is never ranked below its most dangerous variant.
/// Built once per world ([`crate::tui::World::new`]) and shared.
pub fn threat_table(content: &Content) -> ThreatTable {
    let mut table = ThreatTable::new();
    for monster in content.monsters.values() {
        if monster.name.is_empty() {
            continue;
        }
        let name = monster.name.to_lowercase();
        let score = i64::from(monster.experience) * 1000 + i64::from(monster.hitpoints);
        let slot = table.entry(name).or_insert(score);
        if score > *slot {
            *slot = score;
        }
    }
    table
}

/// How long each spell lasts, in combat rounds, by lowercased name.
///
/// Reproduces `graph.rs`'s old `select lower(name), duration from spell
/// where name != '' and duration > 0`: blank names and non-positive
/// durations are both skipped (453 of 1379 shipped spells survive --
/// the design doc's "542" does not match the shipped database), and
/// the SHORTEST duration wins a name collision (`rapid healing` is both
/// 138 and 831), because the figure is a floor for the upkeep timer and
/// early is safe. Built once per world ([`crate::tui::World::new`]).
pub fn spell_durations(content: &Content) -> BTreeMap<String, u32> {
    let mut table: BTreeMap<String, u32> = BTreeMap::new();
    for spell in content.spells.values() {
        if spell.name.is_empty() || spell.duration <= 0 {
            continue;
        }
        let name = spell.name.to_lowercase();
        let rounds = u32::try_from(spell.duration).unwrap_or(0);
        let slot = table.entry(name).or_insert(rounds);
        if rounds < *slot {
            *slot = rounds;
        }
    }
    table
}
