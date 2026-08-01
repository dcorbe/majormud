//! Server-side darkness: `_CAN_SEE` (decompile 66340) refuses sight
//! below light level -150 with "The room is %s - you can't see anything"
//! (0xDF37E, NO trailing period), gating room display, look, exits,
//! search and hide — but NOT movement, so dark rooms are walked blind.
//! Light bands per the 0xBDFD6..0xBE01B string table.

use mud_core::ability::Ability;
use mud_core::content::{ClassId, Content, Direction, Exit, RaceId, Room, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

/// Camp (light 0) with exits into every interesting band: north to the
/// very-dark cave (-200), up to the pitch-black abyss (-201), east to
/// the -151 gloom (dark side of the fencepost), west to the -150 fringe
/// (visible side).
fn dark_world() -> Content {
    let mut content = Content::default();
    let rooms: [(u16, &str, i16); 5] = [
        (1, "Camp", 0),
        (2, "Black Cave", -200),
        (3, "Abyss", -201),
        (4, "Gloom", -151),
        (5, "Fringe", -150),
    ];
    for (num, name, light) in rooms {
        content.add_room(Room {
            id: RoomId { map: 1, room: num },
            name: name.into(),
            description: vec![format!("The {name} stretches on.")],
            light,
            ..Default::default()
        });
    }
    let mut link = |from: u16, d: Direction, to: u16| {
        let exit = Exit {
            dest: RoomId { map: 1, room: to },
            exit_type: 0,
            ..Default::default()
        };
        content
            .rooms
            .get_mut(&RoomId { map: 1, room: from })
            .unwrap()
            .exits[d as usize] = Some(exit);
    };
    link(1, Direction::North, 2);
    link(2, Direction::South, 1);
    link(1, Direction::Up, 3);
    link(3, Direction::Down, 1);
    link(1, Direction::East, 4);
    link(4, Direction::West, 1);
    link(1, Direction::West, 5);
    link(5, Direction::East, 1);
    content
}

fn player_at(name: &str, room: u16) -> Player {
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
        location: RoomId { map: 1, room },
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

const VERY_DARK: &str = "The room is very dark - you can't see anything";
const PITCH_BLACK: &str = "The room is pitch black - you can't see anything";

/// The exact refusal, band descriptor included, and nothing of the room.
#[test]
fn look_in_the_dark_prints_the_refusal_and_no_room() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "got: {shown:?}");
    // No trailing period — 0xDF37E ends at "anything".
    assert!(!shown.contains("anything."), "got: {shown:?}");
    assert!(!shown.contains("Black Cave"), "got: {shown:?}");
    assert!(!shown.contains("Obvious exits"), "got: {shown:?}");
    assert!(!shown.contains("stretches on"), "got: {shown:?}");
}

/// Walking in blind: the mover gets the refusal instead of the room,
/// while the broadcasts on both sides still fire — movement itself is
/// not gated, only the display.
#[test]
fn arrival_in_a_dark_room_is_blind_but_broadcasts_fire() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let bob = core.attach_player(player_at("Bob", 1));
    let carol = core.attach_player(player_at("Carol", 2));
    core.drain_events();
    core.input(bob, "n");
    let events = core.drain_events();
    let to_bob = text_to(&events, bob);
    assert!(to_bob.contains(VERY_DARK), "got: {to_bob:?}");
    assert!(!to_bob.contains("Black Cave"), "got: {to_bob:?}");
    let to_alice = text_to(&events, alice);
    assert!(
        to_alice.contains("Bob just left to the north."),
        "got: {to_alice:?}"
    );
    let to_carol = text_to(&events, carol);
    assert!(
        to_carol.contains("Bob walks into the room from the south."),
        "the dark-room occupant still hears the arrival: {to_carol:?}"
    );
}

/// Bare Enter re-shows the room; in the dark that is the refusal.
#[test]
fn hitting_enter_in_the_dark_is_refused_too() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "got: {shown:?}");
    assert!(!shown.contains("Black Cave"), "got: {shown:?}");
}

/// <= -201 crosses into the bottom band.
#[test]
fn pitch_black_gets_its_own_descriptor() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 3));
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(PITCH_BLACK), "got: {shown:?}");
}

/// The _CAN_SEE fencepost is `level < -150`: -150 renders, -151 refuses.
#[test]
fn minus_150_renders_and_minus_151_does_not() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let fringe = core.attach_player(player_at("Alice", 5));
    let gloom = core.attach_player(player_at("Bob", 4));
    core.drain_events();
    core.input(fringe, "look");
    core.input(gloom, "look");
    let events = core.drain_events();
    let seen = text_to(&events, fringe);
    assert!(seen.contains("Fringe"), "-150 is visible: {seen:?}");
    let blind = text_to(&events, gloom);
    assert!(blind.contains(VERY_DARK), "-151 is dark: {blind:?}");
    assert!(!blind.contains("Gloom"), "got: {blind:?}");
}

/// `exits` is a gated caller (_HANDLE_COMMANDS case 0x49).
#[test]
fn exits_command_is_refused_in_the_dark() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "exits");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "got: {shown:?}");
    assert!(!shown.contains("Obvious exits"), "got: {shown:?}");
}

/// _CMD_SEARCH (51201) and _CMD_HIDE (63822) are gated callers.
#[test]
fn search_and_hide_are_refused_in_the_dark() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "search");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "got: {shown:?}");
    assert!(!shown.contains("searching the area"), "got: {shown:?}");

    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "hide");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "got: {shown:?}");
    assert!(!shown.contains("Attempting to hide"), "got: {shown:?}");
}

/// Light 0 — the overwhelming default — is completely unaffected.
#[test]
fn light_zero_rooms_render_as_before() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains("Camp"), "got: {shown:?}");
    assert!(shown.contains("Obvious exits"), "got: {shown:?}");
}

/// The viewer's own Illu (GET_USER_ABILITY_VALUE(0xd)) is a term of
/// _GET_LIGHT_LEVEL: +100 lifts a -200 room to -100 — dimly lit, visible.
#[test]
fn personal_illumination_lets_the_viewer_see() {
    let mut core = Core::new(dark_world(), CoreConfig::default());
    let mut lit = player_at("Alice", 2);
    assert!(lit.give_innate_ability(Ability::Illu, 100));
    let alice = core.attach_player(lit);
    let bob = core.attach_player(player_at("Bob", 2));
    core.drain_events();
    core.input(alice, "look");
    core.input(bob, "look");
    let events = core.drain_events();
    let seen = text_to(&events, alice);
    assert!(seen.contains("Black Cave"), "got: {seen:?}");
    // Personal means PERSONAL: Bob in the same room stays blind.
    let blind = text_to(&events, bob);
    assert!(blind.contains(VERY_DARK), "got: {blind:?}");
}

// ---- Slice 3: the `light` command (_CMD_LIGHT, decompile 63864) ----

use mud_core::content::{Item, ItemId};

/// The shipped torch shape: type 6, IlluTarget 100. Instance charge
/// counts live on the inventory tuple, so `uses` here is the default.
fn torch() -> Item {
    Item {
        id: ItemId(175),
        name: "torch".into(),
        item_type: 6,
        uses: 800,
        abilities: vec![(Ability::IlluTarget, 100)],
        ..Default::default()
    }
}

fn lantern() -> Item {
    Item {
        id: ItemId(176),
        name: "lantern".into(),
        item_type: 6,
        uses: 2400,
        abilities: vec![(Ability::IlluTarget, 175)],
        ..Default::default()
    }
}

fn sword() -> Item {
    Item {
        id: ItemId(300),
        name: "longsword".into(),
        item_type: 1,
        ..Default::default()
    }
}

fn lit_world() -> Content {
    use mud_core::content::Class;
    let mut content = dark_world();
    content.add_item(torch());
    content.add_item(lantern());
    content.add_item(sword());
    // USER_CAN_USE fails closed on an unregistered class; the ladder
    // needs a real one to reach its later rungs.
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

/// Success rung: lit slot set, IlluTarget joins the room sum — for
/// EVERYONE standing there, not just the holder — and the render gate
/// opens. "You lit the %s." (0xDB52D) to the holder, "%s lights %s %s."
/// (0xDB53E) to the room.
#[test]
fn lighting_the_torch_lights_the_dark_room_for_everyone() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 2);
    holder.inventory.push((ItemId(175), 3));
    let alice = core.attach_player(holder);
    let bob = core.attach_player(player_at("Bob", 2));
    core.drain_events();
    core.input(alice, "light torch");
    let events = core.drain_events();
    let to_alice = text_to(&events, alice);
    assert!(to_alice.contains("You lit the torch."), "got: {to_alice:?}");
    let to_bob = text_to(&events, bob);
    assert!(
        to_bob.contains("Alice lights a torch."),
        "got: {to_bob:?}"
    );
    core.input(alice, "look");
    core.input(bob, "look");
    let events = core.drain_events();
    // -200 + 100 = -100: dimly lit, visible — to both of them.
    assert!(text_to(&events, alice).contains("Black Cave"));
    assert!(text_to(&events, bob).contains("Black Cave"));
}

/// No-args rung: "The current light level is %s" (0xDB4B8).
#[test]
fn bare_light_reports_the_current_band() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 2));
    core.drain_events();
    core.input(alice, "light");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("The current light level is very dark"),
        "got: {shown:?}"
    );
}

/// Already-lit rung (char+0x6ab != -1): "You already have something lit!"
#[test]
fn a_second_light_is_refused() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 2);
    holder.inventory.push((ItemId(175), 3));
    holder.inventory.push((ItemId(176), 3));
    let alice = core.attach_player(holder);
    core.drain_events();
    core.input(alice, "light torch");
    core.drain_events();
    core.input(alice, "light lantern");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("You already have something lit!"),
        "got: {shown:?}"
    );
}

/// Type rung: "You cannot light %s!" (0xDB550).
#[test]
fn a_non_light_is_refused() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 1);
    holder.inventory.push((ItemId(300), -1));
    let alice = core.attach_player(holder);
    core.drain_events();
    core.input(alice, "light longsword");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("You cannot light longsword!"),
        "got: {shown:?}"
    );
}

/// Burned-out rung: a type-6 with 0 uses left asks for a recharge
/// (0xDB568) rather than lighting.
#[test]
fn a_burned_out_light_asks_for_a_recharge() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 1);
    holder.inventory.push((ItemId(175), 0));
    let alice = core.attach_player(holder);
    core.drain_events();
    core.input(alice, "light torch");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("You must recharge that before you may light it again."),
        "got: {shown:?}"
    );
}

/// Not-found rung: _CMD_LIGHT falls through to say-aloud (ORACLE-OPEN).
#[test]
fn light_of_nothing_falls_through_to_say() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    core.drain_events();
    core.input(alice, "light banana");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains("You say"), "got: {shown:?}");
}

/// Extinguish: REMOVE's type-6 pre-branch (5835-5854) clears the lit
/// slot, prints "%s is no longer lit!" + room "%s's %s just went out.",
/// and the room goes dark again.
#[test]
fn removing_the_lit_torch_extinguishes_it() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 2);
    holder.inventory.push((ItemId(175), 3));
    let alice = core.attach_player(holder);
    let bob = core.attach_player(player_at("Bob", 2));
    core.drain_events();
    core.input(alice, "light torch");
    core.drain_events();
    core.input(alice, "remove torch");
    let events = core.drain_events();
    let to_alice = text_to(&events, alice);
    assert!(
        to_alice.contains("torch is no longer lit!"),
        "got: {to_alice:?}"
    );
    let to_bob = text_to(&events, bob);
    assert!(
        to_bob.contains("Alice's torch just went out."),
        "got: {to_bob:?}"
    );
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains(VERY_DARK), "dark again: {shown:?}");
}

/// REMOVE of an unlit light hits the 0x19d3 refusal (wording
/// unrecovered — ORACLE-VERIFY placeholder), NOT the not-wearing line.
#[test]
fn removing_an_unlit_light_is_refused() {
    let mut core = Core::new(lit_world(), CoreConfig::default());
    let mut holder = player_at("Alice", 1);
    holder.inventory.push((ItemId(175), 3));
    let alice = core.attach_player(holder);
    core.drain_events();
    core.input(alice, "remove torch");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("You cannot remove that!"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("not wearing"), "got: {shown:?}");
}
