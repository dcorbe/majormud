//! `mud_client::equipment` -- the wielded-weapon model.
//!
//! The load-bearing case is the swap: the board's own confirmation line
//! never names what it displaced, so the model has to remember rather
//! than parse. See `equipment.rs`'s module doc.

use mud_client::equipment::Equipment;

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
