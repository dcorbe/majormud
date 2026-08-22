//! The backstab opener's weapon decision.
//!
//! A pure function of (wielded weapon, carried items, stealth) -> what
//! to do, evaluated BEFORE arming sneak -- equipping breaks it, so any
//! swap has to happen first
//! (`2026-08-22-inventory-and-backstab-design.md` "The backstab
//! decision"). Nothing here sends anything; it only decides.

use mud_core::content::Content;

use crate::items;

/// What the opener should do about the wielded weapon before attempting
/// a backstab this room entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackstabPlan {
    /// Already able to backstab as wielded: unarmed (`mud-core`'s
    /// `backstab_command` backstabs successfully with `None` weapon), or
    /// the wielded weapon itself carries BSAccu (0x74). No swap needed.
    UseWielded,
    /// Swap to `to` before sneaking and moving, then swap back to
    /// `restore` (whatever was wielded before) after the opening round.
    Swap { to: String, restore: String },
    /// Nothing to do: stealth is not believed intact, no BSAccu-capable
    /// weapon is available anywhere, or the wielded item's identity did
    /// not resolve. Fall through to a normal opening attack -- the spec
    /// deliberately does NOT disarm to enable an unarmed backstab, and
    /// never guesses at an unresolved identity.
    Nothing,
}

/// Decide the plan.
///
/// `wielded` is the display name of the currently wielded weapon as the
/// board printed it (`None` = unarmed); `carried` is the pack, as listed
/// by the board (e.g. `Session::contents().items`). `stealthy` is
/// whatever the caller currently believes about hidden/sneak-armed
/// state -- this function trusts it and does not re-derive it.
pub fn decide(
    content: &Content,
    wielded: Option<&str>,
    carried: &[String],
    stealthy: bool,
) -> BackstabPlan {
    if !stealthy {
        return BackstabPlan::Nothing;
    }
    let Some(weapon_name) = wielded else {
        return BackstabPlan::UseWielded;
    };
    let Some(weapon) = items::resolve(content, weapon_name) else {
        // Identity unresolved: never guess, never act.
        return BackstabPlan::Nothing;
    };
    if items::bs_capable(weapon) {
        return BackstabPlan::UseWielded;
    }
    let bs_in_pack = carried.iter().find(|name| {
        items::resolve(content, name).is_some_and(items::bs_capable)
    });
    match bs_in_pack {
        Some(bs_name) => BackstabPlan::Swap {
            to: bs_name.clone(),
            restore: weapon_name.to_string(),
        },
        // Deliberately not a disarm-to-backstab-unarmed plan: see
        // `BackstabPlan::Nothing`'s doc.
        None => BackstabPlan::Nothing,
    }
}
