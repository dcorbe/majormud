//! The live board's ANSI color scheme (cataloged from the oracle raws):
//! room name 1;36, description 0;37, notice 0;36, "Also here" 0;35 with
//! 1;35 names, exits 0;32, prompt 0;37 with the HP number 1;31 when low,
//! movement lines 1;33 name + 0;32 sentence, your hits 1;31, your
//! misses/glances 0;31, incoming monster lines 0;36, combat markers
//! 0;33, rumbles 0;35, says 0;32. `CoreConfig::ansi` gates it: false
//! (the default — and the MBBS non-graphics user setting) strips every
//! escape at the output funnel.

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Exit, Monster, MonsterId, Race, RaceId, Room,
    RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_core::text;

const HALL: RoomId = RoomId { map: 1, room: 1 };
const YARD: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut hall = Room {
        id: HALL,
        name: "Great Hall".into(),
        description: vec!["A vaulted chamber.".into()],
        ..Default::default()
    };
    hall.exits[Direction::North as usize] = Some(Exit {
        dest: YARD,
        exit_type: 0,
        ..Default::default()
    });
    let mut yard = Room {
        id: YARD,
        name: "Yard".into(),
        ..Default::default()
    };
    yard.exits[Direction::South as usize] = Some(Exit {
        dest: HALL,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(hall);
    content.add_room(yard);
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "giant rat".into(),
        hitpoints: 500,
        energy: 1000,
        behaviour: 2,
        aggression: 50,
        attacks: [
            AttackForm {
                kind: 1,
                accuracy: 100,
                weight: 100,
                min_damage: 1,
                max_damage: 2,
                energy: 200,
                ..Default::default()
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
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

fn player(name: &str, hp: i32) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: hp,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ansi: true, // the per-user flag (M7) — this suite tests colours

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

fn ansi_config() -> CoreConfig {
    CoreConfig {
        ansi: true,
        ..CoreConfig::default()
    }
}

#[test]
fn room_render_matches_the_board_palette() {
    let mut core = Core::new(world(), ansi_config());
    let s = core.attach_player(player("Alice", 10));
    core.spawn_monster(MonsterId(1), HALL).unwrap();
    core.drain_events();
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("\x1b[1;36mGreat Hall"), "room name bright cyan: {shown:?}");
    assert!(shown.contains("\x1b[0;37m    A vaulted chamber."), "desc white: {shown:?}");
    assert!(
        shown.contains("\x1b[0;35mAlso here: \x1b[1;35mgiant rat\x1b[0m\x1b[0;35m."),
        "also-here magenta with bright names: {shown:?}"
    );
    assert!(shown.contains("\x1b[0;32mObvious exits: north"), "exits green: {shown:?}");
    assert!(shown.contains("\x1b[0;37m[HP=10\x1b[0;37m]:"), "prompt white: {shown:?}");
}

#[test]
fn low_hp_paints_the_prompt_number_red() {
    // Oracle: "[HP=" + 1;31 "3" + 0;37 "]:" at 3 of ~35 max.
    let mut core = Core::new(world(), ansi_config());
    let mut weak = player("Weak", 2);
    weak.level = 5; // max HP well above 8 so 2 sits under the quarter line
    let s = core.attach_player(weak);
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("[HP=\x1b[1;31m2\x1b[0;37m]:"),
        "low HP number bright red: {shown:?}"
    );
}

#[test]
fn movement_lines_are_yellow_name_green_text() {
    let mut core = Core::new(world(), ansi_config());
    let alice = core.attach_player(player("Alice", 10));
    let bob = core.attach_player(player("Bob", 10));
    core.drain_events();
    core.input(bob, "n");
    let to_alice = text_to(&core.drain_events(), alice);
    assert!(
        to_alice.contains("\x1b[1;33mBob\x1b[0;32m just left to the north."),
        "departure: {to_alice:?}"
    );
}

#[test]
fn combat_lines_use_the_red_and_cyan_families() {
    let mut core = Core::new(world(), ansi_config());
    let s = core.attach_player(player("Fighter", 400));
    core.spawn_monster(MonsterId(1), HALL).unwrap();
    core.drain_events();
    core.input(s, "attack rat");
    let mut shown = String::new();
    for _ in 0..15 {
        core.tick();
        shown.push_str(&text_to(&core.drain_events(), s));
    }
    assert!(
        shown.contains("\x1b[0;33m*Combat Engaged*"),
        "engage marker yellow: {shown:?}"
    );
    assert!(
        shown.contains("\x1b[1;31mGiant rat"),
        "incoming hit bright red: {shown:?}"
    );
    assert!(
        shown.contains("\x1b[0;36mYou swing at giant rat!"),
        "your plain miss is CYAN, not the red the glance family uses: {shown:?}"
    );
}

#[test]
fn the_players_miss_and_parry_lines_are_cyan_but_the_glance_is_red() {
    // MEASURED (`re/oracle/oracle_dodge_parry_{control,acc-mid,acc-high}.raw`,
    // 2026-07-26). The raws are NOT ANSI-stripped -- `charm.md` §8.3 said they
    // were, which is why the parry line's colour was filed as unmeasurable --
    // so the attribute byte ahead of each line is readable directly. Counting
    // only segments whose ENTIRE content is the line in question, so the code
    // belongs to that line and not to a neighbour sharing the physical line:
    //
    //     plain miss  "You swing at giant bat!"                    0;36  x24
    //     parry       "You swing at ... who dodges your attack!"   0;36  x46
    //     glance      "Your ... glances off ..."                   0;31  x36
    //
    // Zero exceptions in any of the three captures. So §8.3's guess that the
    // parry line INHERITS the plain miss's colour was right, but the plain
    // miss itself was painted with the glance's red. The two are different
    // families on the board: a swing that never connected is cyan, like the
    // incoming monster lines, and only the connected-but-soaked glance is red.
    assert!(
        text::player_miss("swing at", "giant bat").starts_with("\x1b[0;36m"),
        "the plain miss is cyan"
    );
    assert!(
        text::player_dodge("swing at", "giant bat").starts_with("\x1b[0;36m"),
        "the parry line inherits the plain miss's cyan"
    );
    assert!(
        text::player_glance("swing at", "giant bat").starts_with("\x1b[0;31m"),
        "the glance stays red"
    );
}

#[test]
fn plain_mode_strips_every_escape() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut plain = player("Plain", 10);
    plain.ansi = false; // the per-user flag decides (M7)
    let s = core.attach_player(plain);
    core.spawn_monster(MonsterId(1), HALL).unwrap();
    core.drain_events();
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains('\x1b'), "no escapes in plain mode: {shown:?}");
    assert!(shown.contains("Great Hall"), "{shown:?}");
    assert!(shown.contains("[HP=10]:"), "{shown:?}");
}
