//! Gang record math (`gangs.md` §0, §2.1) — the pure pieces.

use mud_core::gang::{Gang, GANG_DISBANDED, GANG_HIDDEN, GANG_SATURATED};

fn gang(name: &str, leader: &str) -> Gang {
    Gang::new(name, leader, 0)
}

/// gangs.md §0: the record key is the uppercased name; the display name
/// keeps the caller's case (≤19 chars, enforced by cmd_create before the
/// record is built — `new` just stores).
#[test]
fn key_is_uppercased_display_keeps_case() {
    let g = gang("Iron Fist", "Salad");
    assert_eq!(g.name_key, "IRON FIST");
    assert_eq!(g.display, "Iron Fist");
}

/// gangs.md §0/§1.1: a fresh gang has member count 1, zero pools, zero
/// flags, and the creator as leader.
#[test]
fn new_gang_initial_state() {
    let g = gang("Iron Fist", "Salad");
    assert_eq!(g.member_count, 1);
    assert_eq!(g.exp_pool, 0);
    assert_eq!(g.secondary_pool, 0);
    assert_eq!(g.wrap, 0);
    assert_eq!(g.flags, 0);
    assert_eq!(g.leader, "Salad");
}

/// gangs.md §0: leadership is string equality with `gang+0x2c` — there is
/// no leader flag.
#[test]
fn leadership_is_string_equality() {
    let g = gang("Iron Fist", "Salad");
    assert!(g.is_leader("Salad"));
    assert!(!g.is_leader("Torgo"));
}

/// gangs.md §2.1: while unsaturated the award lands in the primary pool.
#[test]
fn add_exp_accumulates_primary() {
    let mut g = gang("Iron Fist", "Salad");
    g.add_exp(1000);
    g.add_exp(234);
    assert_eq!(g.exp_pool, 1234);
    assert_eq!(g.secondary_pool, 0);
    assert!(!g.is_saturated());
}

/// gangs.md §2.1: on 32-bit overflow the primary saturates to
/// 0xFFFFFFFF, the excess rolls into the secondary pool, and flag 0x8 is
/// set.
#[test]
fn add_exp_saturates_primary_into_secondary() {
    let mut g = gang("Iron Fist", "Salad");
    g.exp_pool = u32::MAX - 10;
    g.add_exp(25);
    assert_eq!(g.exp_pool, u32::MAX);
    assert_eq!(g.secondary_pool, 15);
    assert!(g.is_saturated());
    assert_eq!(g.wrap, 0);
}

/// gangs.md §2.1: an award landing exactly on 0xFFFFFFFF does not
/// overflow — saturation requires crossing the boundary.
#[test]
fn add_exp_exact_max_does_not_saturate() {
    let mut g = gang("Iron Fist", "Salad");
    g.exp_pool = u32::MAX - 25;
    g.add_exp(25);
    assert_eq!(g.exp_pool, u32::MAX);
    assert!(!g.is_saturated());
}

/// gangs.md §2.1: once saturated, accumulation continues in the
/// secondary pool; the primary never moves again.
#[test]
fn add_exp_after_saturation_feeds_secondary() {
    let mut g = gang("Iron Fist", "Salad");
    g.flags |= GANG_SATURATED;
    g.exp_pool = u32::MAX;
    g.add_exp(500);
    assert_eq!(g.exp_pool, u32::MAX);
    assert_eq!(g.secondary_pool, 500);
}

/// gangs.md §2.1: `+0x5c` counts each further wrap of the secondary pool.
#[test]
fn secondary_overflow_bumps_wrap_counter() {
    let mut g = gang("Iron Fist", "Salad");
    g.flags |= GANG_SATURATED;
    g.exp_pool = u32::MAX;
    g.secondary_pool = u32::MAX - 10;
    g.add_exp(25);
    assert_eq!(g.wrap, 1);
    assert_eq!(g.secondary_pool, 14); // wrapping_add semantics
}

/// gangs.md §0 flags: 0x1 disbanded, 0x4 hidden from the top-gangs
/// listing (sysop DISABLE), 0x8 saturated.
#[test]
fn flag_accessors() {
    let mut g = gang("Iron Fist", "Salad");
    assert!(!g.is_disbanded() && !g.is_hidden() && !g.is_saturated());
    g.flags = GANG_DISBANDED;
    assert!(g.is_disbanded());
    g.flags = GANG_HIDDEN;
    assert!(g.is_hidden());
    g.flags = GANG_SATURATED;
    assert!(g.is_saturated());
}
