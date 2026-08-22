//! The backstab weapon decision, one case per spec row
//! (`2026-08-22-inventory-and-backstab-design.md` "The backstab
//! decision"), against the real WG3-NT database -- same rows
//! `tests/items.rs` already established: dagger (68) and sickle (74)
//! carry BSAccu, quarterstaff (100) does not.

use mud_client::backstab::{decide, BackstabPlan};
use mud_core::content::Content;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn content() -> &'static Content {
    use std::sync::OnceLock;
    static C: OnceLock<Content> = OnceLock::new();
    C.get_or_init(|| mud_core::content_db::load(&db_path()).expect("load content"))
}

/// Dual-purpose: the wielded weapon already carries BSAccu. No swap.
#[test]
fn a_dual_purpose_wielded_weapon_needs_no_swap() {
    let plan = decide(content(), Some("dagger"), &[], true);
    assert_eq!(plan, BackstabPlan::UseWielded);
}

/// Unarmed already backstabs successfully (`mud-core`'s
/// `backstab_command`, `None => AttackType::Backstab`). No swap.
#[test]
fn unarmed_needs_no_swap() {
    let plan = decide(content(), None, &[], true);
    assert_eq!(plan, BackstabPlan::UseWielded);
}

/// Swap-needed: the wielded weapon lacks BSAccu, but a capable one sits
/// in the pack.
#[test]
fn a_non_capable_wielded_weapon_swaps_to_a_carried_one() {
    let carried = vec!["dagger".to_string()];
    let plan = decide(content(), Some("quarterstaff"), &carried, true);
    assert_eq!(
        plan,
        BackstabPlan::Swap {
            to: "dagger".to_string(),
            restore: "quarterstaff".to_string(),
        }
    );
}

/// None carried: do nothing. The spec deliberately does not disarm to
/// enable an unarmed backstab -- that trades the whole fight's damage
/// for one opener.
#[test]
fn nothing_carried_capable_does_nothing() {
    let plan = decide(content(), Some("quarterstaff"), &[], true);
    assert_eq!(plan, BackstabPlan::Nothing);
}

/// Same row, but the pack is non-empty -- proving the miss is about
/// capability, not merely an empty pack.
#[test]
fn a_pack_with_no_capable_weapon_does_nothing() {
    let carried = vec!["quarterstaff".to_string(), "torch".to_string()];
    let plan = decide(content(), Some("quarterstaff"), &carried, true);
    assert_eq!(plan, BackstabPlan::Nothing);
}

/// Identity unresolved: never guess, never act.
#[test]
fn an_unresolved_wielded_identity_does_nothing() {
    let carried = vec!["dagger".to_string()];
    let plan = decide(content(), Some("a completely invented weapon"), &carried, true);
    assert_eq!(
        plan,
        BackstabPlan::Nothing,
        "an unresolved identity must never act, even with a real backstab weapon in the pack"
    );
}

/// Stealth not believed intact: nothing to do regardless of weapons.
#[test]
fn without_stealth_the_decision_does_nothing() {
    let carried = vec!["dagger".to_string()];
    let plan = decide(content(), Some("quarterstaff"), &carried, false);
    assert_eq!(plan, BackstabPlan::Nothing);
}
