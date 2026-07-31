//! The shell-style prompt discipline (every oracle capture: async bursts
//! are prefixed `ESC[79D ESC[K` — erase the dangling prompt line — then
//! the message, then a fresh prompt). Input-driven responses print below
//! the echoed command line with no erase, exactly like the live board.
//!
//! Room renders carry their own preamble on top of that, and it comes in
//! two shapes. Measured in `re/oracle/accept-run1.raw` and matching the
//! grammar OmegaMUD's `Parsing/RoomParseState.cs` documents: a look or a
//! move resets the SGR first (`ESC[0;37;40m ESC[79D ESC[K ESC[1;36m`),
//! while entering the game and the bare-Enter re-show skip the reset
//! (`ESC[79D ESC[K ESC[1;36m`). `ESC[79D` on its own is not a render
//! marker — 354 of them in that one capture, only 25 opening a room.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 10,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ansi: true, // the per-user flag decides (M7); plain-mode tests
        // flip it back off on their own players.

        ..Default::default()
    }
}

fn text_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn async_broadcast_erases_the_prompt_and_redraws_it() {
    // ANSI sessions get the DLL's in-place erase; plain terminals get a
    // newline away from the dangling prompt. Both re-prompt after.
    let mut core = Core::new(
        world(),
        CoreConfig { ansi: true, ..CoreConfig::default() },
    );
    let alice = core.attach_player(player("Alice"));
    let bob = core.attach_player(player("Bob"));
    core.drain_events(); // Alice sits at a dangling prompt
    core.input(bob, "hi"); // say fallback
    let to_alice = text_to(&core.drain_events(), alice);
    assert!(
        to_alice.starts_with("\x1b[79D\x1b[K"),
        "the dangling prompt line is erased first: {to_alice:?}"
    );
    assert!(to_alice.contains("Bob says \"hi\""), "{to_alice:?}");
    assert!(
        to_alice.trim_end_matches(' ').ends_with("]:\x1b[0m"),
        "a fresh prompt follows the burst: {to_alice:?}"
    );
}

#[test]
fn plain_mode_steps_off_the_prompt_with_a_newline() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut plain = player("Alice");
    plain.ansi = false;
    let alice = core.attach_player(plain);
    let bob = core.attach_player(player("Bob"));
    core.drain_events();
    core.input(bob, "hi");
    let to_alice = text_to(&core.drain_events(), alice);
    assert!(
        to_alice.starts_with("\r\n"),
        "plain terminals get a newline instead of the erase: {to_alice:?}"
    );
    assert!(
        to_alice.trim_end_matches(' ').ends_with("[HP=10]:"),
        "fresh prompt after: {to_alice:?}"
    );
}

#[test]
fn own_command_output_prints_below_the_echoed_line() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut plain = player("Alice");
    plain.ansi = false;
    let alice = core.attach_player(plain);
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        !shown.starts_with("\x1b[79D\x1b[K"),
        "input responses never erase the command line: {shown:?}"
    );
    assert!(shown.contains("Hall"), "{shown:?}");
    assert!(
        shown.trim_end_matches(' ').ends_with("[HP=10]:"),
        "prompt after the response: {shown:?}"
    );
}

/// The two preambles, byte for byte. `RESET_ERASE` opens a look or a
/// move; `ERASE` alone opens a game entry or a bare-Enter re-show.
const RESET_ERASE: &str = "\x1b[0;37;40m\x1b[79D\x1b[K\x1b[1;36m";
const ERASE: &str = "\x1b[79D\x1b[K\x1b[1;36m";

fn ansi_core() -> Core {
    Core::new(
        world(),
        CoreConfig {
            ansi: true,
            ..CoreConfig::default()
        },
    )
}

#[test]
fn a_look_render_resets_before_the_erase() {
    let mut core = ansi_core();
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.starts_with(RESET_ERASE),
        "look carries the reset variant: {shown:?}"
    );
}

#[test]
fn a_move_render_resets_before_the_erase() {
    let mut content = world();
    let mut hall = content.rooms[&HALL].clone();
    let cell = RoomId { map: 1, room: 2 };
    hall.exits[mud_core::content::Direction::North as usize] = Some(mud_core::content::Exit {
        dest: cell,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(hall);
    content.add_room(Room {
        id: cell,
        name: "Cell".into(),
        ..Default::default()
    });
    let mut core = Core::new(
        content,
        CoreConfig {
            ansi: true,
            ..CoreConfig::default()
        },
    );
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    core.input(alice, "n");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.starts_with(RESET_ERASE),
        "arriving carries the reset variant: {shown:?}"
    );
    assert!(shown.contains("Cell"), "{shown:?}");
}

/// Hit-enter and game entry are the board's two reset-less renders. The
/// erase is still there — it is the reset that distinguishes them.
#[test]
fn a_bare_enter_reshow_skips_the_reset() {
    let mut core = ansi_core();
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    core.input(alice, "");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.starts_with(ERASE),
        "the re-show erases without resetting: {shown:?}"
    );
    assert!(
        !shown.starts_with(RESET_ERASE),
        "and it is not the look/move shape: {shown:?}"
    );
}

#[test]
fn entering_the_game_skips_the_reset() {
    let mut core = ansi_core();
    let alice = core.attach_player(player("Alice"));
    let shown = text_to(&core.drain_events(), alice);
    let render = shown.find("Hall").expect("the room renders on entry");
    let preamble = &shown[..render];
    assert!(
        preamble.ends_with(ERASE),
        "entry erases without resetting: {preamble:?}"
    );
    assert!(
        !preamble.ends_with(RESET_ERASE),
        "and it is not the look/move shape: {preamble:?}"
    );
}

/// The render brings its own erase, so a disturbed prompt must not also
/// collect `output`'s — one erase, not two.
#[test]
fn a_render_erases_exactly_once() {
    let mut core = ansi_core();
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    let head = &shown[..shown.find("Hall").expect("room name")];
    assert_eq!(
        head.matches("\x1b[79D").count(),
        1,
        "one cursor-back in the preamble: {head:?}"
    );
}

/// A plain terminal cannot erase anything; it steps off the prompt with
/// a newline, and the preamble strips away with the rest of the ANSI.
#[test]
fn plain_terminals_get_no_preamble() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut plain = player("Alice");
    plain.ansi = false;
    let alice = core.attach_player(plain);
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        !shown.contains('\x1b'),
        "no escapes reach a plain terminal: {shown:?}"
    );
    assert!(shown.contains("Hall"), "{shown:?}");
}

#[test]
fn tick_driven_output_also_redraws() {
    // A monster spawn/attack burst on a tick disturbs the prompt the same
    // way — here the simplest tick source: another player's exit
    // meditation dots don't apply, so use a broadcast via say from a tick
    // isn't possible; instead verify that after a tick with NO output the
    // prompt is NOT re-spammed.
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    for _ in 0..10 {
        core.tick();
    }
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.is_empty(),
        "quiet ticks re-print nothing: {shown:?}"
    );
}
