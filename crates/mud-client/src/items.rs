//! Item identity: the join from a display string the board printed to
//! the `Content.items` row it names.
//!
//! Mirrors `sheet::class_caster_group`'s contract: case-insensitive,
//! trimmed, `Option`-returning, and a miss means *"I do not know"*,
//! never *"no such item"*. The one thing [`resolve`] must NEVER do is
//! resolve to a WRONG row: `Content.items` has 1950 rows and the board
//! prints display strings, not ids, so a name resolution that guesses at
//! a near-miss is worse than giving up -- confidently wielding a weapon
//! that turns out not to carry BSAccu costs the whole opening round and
//! says nothing until the refusal line arrives
//! (`2026-08-22-inventory-and-backstab-design.md` "Item identity
//! resolution").

use mud_core::ability::Ability;
use mud_core::content::{Content, Item};

/// Resolve a display string -- an inventory entry, an equip
/// confirmation's `<name>` -- to the `Content.items` row it names.
///
/// Handles the three things the wire actually does to a stored name
/// before it reaches here:
/// - a leading count (`"3 sickle"` -- `show_inventory` groups loose
///   items this way, `crates/mud-core/src/game.rs`),
/// - a leading article (`"a dagger"`),
/// - a trailing worn/wielded-hand suffix (`"quarterstaff (Two
///   handed)"`, `"padded pants (Legs)"`).
///
/// Matching is exact once normalised -- never prefix, never substring,
/// never edit-distance. Ambiguity is a miss: if two different rows
/// normalise to the same key, this cannot tell which the board meant
/// and returns `None` rather than picking either.
pub fn resolve<'c>(content: &'c Content, display_name: &str) -> Option<&'c Item> {
    let key = normalize(display_name)?;
    let mut hits = content
        .items
        .values()
        .filter(|item| normalize(&item.name).as_deref() == Some(key.as_str()));
    let first = hits.next()?;
    if hits.next().is_some() {
        // Two or more rows share this normalised name: ambiguous, not
        // resolved.
        return None;
    }
    Some(first)
}

/// Whether `item` carries ability `0x74` (BSAccu) -- the backstab gate
/// on the wielded weapon (`mud-core/src/game.rs`'s `backstab_command`:
/// `None` weapon backstabs unarmed, `Some(id)` requires this ability or
/// the swing degrades to a normal attack).
pub fn bs_capable(item: &Item) -> bool {
    let bsaccu = Ability::from_id(0x74).expect("BSAccu is in the enum");
    item.abilities.iter().any(|(a, _)| *a == bsaccu)
}

/// Strip a trailing worn/wielded-hand suffix, a leading numeric count,
/// and a leading article, then lowercase and trim. `None` only for an
/// empty result -- a key that could only ever match a blank `Content`
/// name, and none carry one.
fn normalize(s: &str) -> Option<String> {
    let s = s.trim();
    let s = strip_paren_suffix(s);
    let s = strip_leading_count(s);
    let s = strip_article(s);
    let s = s.trim().to_lowercase();
    if s.is_empty() { None } else { Some(s) }
}

/// `"quarterstaff (Two handed)"` -> `"quarterstaff"`. Only a trailing
/// `" (...)"` counts -- an item whose own name legitimately ends in a
/// parenthesis would be a different bug to chase, and none of the
/// shipped rows do.
fn strip_paren_suffix(s: &str) -> &str {
    match (s.rfind(" ("), s.ends_with(')')) {
        (Some(i), true) => &s[..i],
        _ => s,
    }
}

/// `"3 sickle"` -> `"sickle"`. Only a run of digits followed by a space
/// counts as a count -- an item name that happens to start with a digit
/// is not a pattern the shipped data has.
fn strip_leading_count(s: &str) -> &str {
    match s.split_once(' ') {
        Some((count, rest)) if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) => {
            rest
        }
        _ => s,
    }
}

/// `"a dagger"` -> `"dagger"`. Checked longest-first so `"an"` is never
/// swallowed by a hypothetical `"a "` collision -- there isn't one, but
/// the order costs nothing and documents the intent.
fn strip_article(s: &str) -> &str {
    for article in ["an ", "the ", "a "] {
        if let Some(rest) = s.strip_prefix(article) {
            return rest;
        }
    }
    s
}
