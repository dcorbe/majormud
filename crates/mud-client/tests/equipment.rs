//! `mud_client::equipment` -- the wielded-weapon model.
//!
//! The load-bearing case is the swap: the board's own confirmation line
//! never names what it displaced, so the model has to remember rather
//! than parse. See `equipment.rs`'s module doc.

use mud_client::equipment::{Equipment, wielded_from_listing};

#[test]
fn confirming_a_first_weapon_displaces_nothing() {
    let mut eq = Equipment::new();
    let displaced = eq.confirm_weapon("a dagger");
    assert_eq!(displaced, None);
    assert_eq!(eq.weapon(), Some("a dagger"));
}

/// Step 2's test: the board's swap line, `You are now holding a
/// longsword.`, says nothing whatsoever about the dagger it displaced.
/// The client must know it anyway.
#[test]
fn a_weapon_swap_remembers_what_it_took_off_though_the_wire_never_says() {
    let mut eq = Equipment::new();
    eq.confirm_weapon("a dagger");

    let displaced = eq.observe("You are now holding a longsword.");

    assert_eq!(
        displaced,
        Some("a dagger".to_string()),
        "the client must know what it took off without ever having been told"
    );
    assert_eq!(eq.weapon(), Some("a longsword"));
}

/// Step 3: a refused equip (class/level/slot gate) must not update the
/// model -- the client may not assume its own command succeeded.
#[test]
fn a_refused_equip_does_not_update_the_model() {
    let mut eq = Equipment::new();
    eq.confirm_weapon("a dagger");

    let displaced = eq.observe("You may not use that weapon!");
    assert_eq!(displaced, None);
    assert_eq!(eq.weapon(), Some("a dagger"), "a refusal must not be believed");

    let displaced2 = eq.observe("You do not have a longsword left unequipped.");
    assert_eq!(displaced2, None);
    assert_eq!(eq.weapon(), Some("a dagger"));
}

#[test]
fn unrelated_lines_do_not_touch_the_model() {
    let mut eq = Equipment::new();
    eq.confirm_weapon("a dagger");

    eq.observe("A giant rat enters the room.");

    assert_eq!(eq.weapon(), Some("a dagger"));
}

#[test]
fn a_fresh_equipment_model_is_unarmed() {
    let eq = Equipment::new();
    assert_eq!(eq.weapon(), None);
}

/// Empty-input hygiene: a malformed or truncated line must not be read
/// as an equip of nothing.
#[test]
fn an_empty_holding_line_is_not_a_confirmation() {
    let mut eq = Equipment::new();
    let out = eq.observe("You are now holding .");
    assert_eq!(out, None);
    assert_eq!(eq.weapon(), None);
}

// ---------------------------------------------------------------------
// `wielded_from_listing` / `Equipment::seed` -- the marker rule
// established against MudPlay's `EquippedSlotRegex` and
// `GAME_MECHANICS.md`'s "Equipment & gear": a trailing `"(<Slot>)"`
// means equipped (worn OR wielded); no marker means carried only.
// ---------------------------------------------------------------------

/// The real fixture wording (`crates/mud-client/tests/contents.rs`'s
/// `REAL_INVENTORY`-shaped reply, `sheet.rs`'s wrap-rejoin already
/// folds `"quarterstaff (Two\r\nhanded)"` into one entry) -- a
/// two-handed weapon, mixed in among items with no marker at all.
#[test]
fn a_two_handed_marker_identifies_the_wielded_weapon() {
    let items = vec![
        "21 silver nobles".to_string(),
        "quarterstaff (Two handed)".to_string(),
        "a torch".to_string(),
    ];
    assert_eq!(wielded_from_listing(&items), Some("quarterstaff".to_string()));
}

/// A one-handed weapon marks the OTHER wording -- both mean wielded.
#[test]
fn a_weapon_hand_marker_identifies_the_wielded_weapon() {
    let items = vec!["gilded robes (Torso)".to_string(), "a dagger (Weapon Hand)".to_string()];
    assert_eq!(wielded_from_listing(&items), Some("a dagger".to_string()));
}

/// This is the whole marker rule under test: a carried-but-unequipped
/// weapon prints NO parenthetical at all, so it must never be read as
/// wielded. Mutation target -- weaken the match (e.g. treat ANY
/// carried weapon-shaped entry as wielded, or match on presence of "("
/// rather than the closed two-word set) and this must fail.
#[test]
fn a_carried_but_unequipped_weapon_has_no_marker_and_is_not_wielded() {
    let items = vec!["a dagger".to_string(), "gilded robes (Torso)".to_string()];
    assert_eq!(
        wielded_from_listing(&items),
        None,
        "an unmarked carried weapon must never be believed wielded"
    );
}

/// An armour marker alone (no weapon-hand marker anywhere) must not be
/// picked up as a weapon -- the two weapon spellings are the only ones
/// this function recognises.
#[test]
fn a_worn_armour_marker_is_not_mistaken_for_a_wielded_weapon() {
    let items = vec!["gilded robes (Torso)".to_string(), "a ring (Finger)".to_string()];
    assert_eq!(wielded_from_listing(&items), None);
}

#[test]
fn an_empty_carried_list_has_no_wielded_weapon() {
    assert_eq!(wielded_from_listing(&[]), None);
}

/// `Equipment::seed` only takes when nothing has been confirmed yet --
/// past the first listing, the client's own `eq`/`wield` history is
/// authoritative and a later, possibly-stale listing must not overwrite
/// it.
#[test]
fn seeding_a_fresh_model_adopts_the_listings_wielded_weapon() {
    let mut eq = Equipment::new();
    eq.seed(&["quarterstaff (Two handed)".to_string()]);
    assert_eq!(eq.weapon(), Some("quarterstaff"));
}

#[test]
fn seeding_never_overwrites_an_already_confirmed_weapon() {
    let mut eq = Equipment::new();
    eq.confirm_weapon("a dagger");
    eq.seed(&["quarterstaff (Two handed)".to_string()]);
    assert_eq!(
        eq.weapon(),
        Some("a dagger"),
        "our own confirmed equip history outranks a listing re-read"
    );
}

/// A listing with no equipped weapon at all (genuinely unarmed) seeds
/// `None` -- distinct from "never seeded" only in that a later `observe`
/// can still confirm one, exactly as it always could.
#[test]
fn seeding_from_an_unarmed_listing_stays_unarmed() {
    let mut eq = Equipment::new();
    eq.seed(&["a torch".to_string()]);
    assert_eq!(eq.weapon(), None);
}
