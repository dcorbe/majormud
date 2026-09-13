use mud_client::combat::CombatState;

#[test]
fn a_lone_combat_off_clears_and_cools() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), Some("thief"));
}

#[test]
fn a_switch_pair_restores_the_target_and_cancels_cooling() {
    let mut c = CombatState::new();
    c.engage("dark goblin archer");
    c.on_combat_off();
    c.on_combat_engaged();
    assert_eq!(c.engaged(), Some("dark goblin archer"));
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn a_non_engaged_event_closes_the_switch_window() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    c.close_switch_window();
    c.on_combat_engaged(); // too late: window closed
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), Some("thief"));
}

#[test]
fn a_kill_clears_with_no_cooldown() {
    let mut c = CombatState::new();
    c.engage("big skeleton");
    c.on_kill();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn default_state_is_idle() {
    let c = CombatState::default();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn engage_sets_the_target() {
    let mut c = CombatState::new();
    c.engage("fierce filthbug");
    assert_eq!(c.engaged(), Some("fierce filthbug"));
}

#[test]
fn a_switch_window_with_nothing_to_restore_stays_idle() {
    // A kill's Combat Off (nothing was engaged) opens a window with
    // nothing to restore; a following Combat Engaged has nothing to do.
    let mut c = CombatState::new();
    c.on_combat_off();
    c.on_combat_engaged();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn the_quiet_prompt_backstop_clears_at_the_threshold() {
    let mut c = CombatState::new();
    c.engage("cave bear");
    assert!(!c.note_quiet_prompt(3));
    assert!(!c.note_quiet_prompt(3));
    assert_eq!(c.engaged(), Some("cave bear"));
    assert!(c.note_quiet_prompt(3));
    assert_eq!(c.engaged(), None);
}

#[test]
fn a_blow_resets_the_quiet_prompt_count() {
    let mut c = CombatState::new();
    c.engage("cave bear");
    assert!(!c.note_quiet_prompt(2));
    c.note_blow();
    assert!(!c.note_quiet_prompt(2));
    assert_eq!(c.engaged(), Some("cave bear"));
}

#[test]
fn note_quiet_prompt_does_nothing_while_idle() {
    let mut c = CombatState::new();
    assert!(!c.note_quiet_prompt(1));
    assert_eq!(c.engaged(), None);
}

#[test]
fn cooling_settles_after_two_present_blocks() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    assert_eq!(c.cooling_noun(), Some("thief"));
    c.settle_cooling(true);
    assert_eq!(c.cooling_noun(), Some("thief"));
    c.settle_cooling(true);
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn cooling_settles_on_one_absent_block() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    assert_eq!(c.cooling_noun(), Some("thief"));
    c.settle_cooling(false);
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn clear_forces_idle_with_no_cooldown() {
    let mut c = CombatState::new();
    c.engage("giant rat");
    c.clear();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), None);
}
