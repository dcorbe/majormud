//! `get_random_name` (0x424172, monsters.md §2 step 6): the spawn-time
//! adjective walk over the template's name block (sqlite `desctxt` =
//! `knmsr+0x124`). Per line a candidate composes — `A:` base sep suffix,
//! `B:` prefix sep base, `F:` full replace, `N:` base, anything else the
//! line verbatim — then `genrdn(0,100)` accepts on <= 9; running off the
//! block keeps the LAST candidate. Separator = " " (`DAT_00480efa`),
//! strncpy/strncat caps 28/29, 1000-line ceiling. Death lines always use
//! the base template name (spellcasting.md §8.10).

use mud_core::content::{
    Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock, TextBlock,
    TextBlockId,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player};
use mud_core::text::generate_name;

fn fixed(rolls: &[i32]) -> impl FnMut(i32, i32) -> i32 + '_ {
    let mut i = 0;
    move |_, _| {
        let r = rolls.get(i).copied().unwrap_or(100);
        i += 1;
        r
    }
}

#[test]
fn low_roll_accepts_the_composed_line() {
    let mut roll = fixed(&[50, 3]);
    let name = generate_name("giant rat", "B:nasty\nB:angry\n", &mut roll);
    assert_eq!(name, "angry giant rat");
}

#[test]
fn running_off_the_block_keeps_the_last_candidate() {
    let mut roll = fixed(&[99, 99, 99]);
    let name = generate_name("giant rat", "B:nasty\nB:angry\nB:thin\n", &mut roll);
    assert_eq!(name, "thin giant rat");
}

#[test]
fn line_kinds_compose_correctly() {
    assert_eq!(
        generate_name("giant rat", "A:the Large\n", &mut fixed(&[0])),
        "giant rat the Large"
    );
    assert_eq!(
        generate_name("giant rat", "F:dire vermin\n", &mut fixed(&[0])),
        "dire vermin"
    );
    assert_eq!(generate_name("giant rat", "N:\n", &mut fixed(&[0])), "giant rat");
    // An unrecognized line (no marker) is taken verbatim.
    assert_eq!(
        generate_name("giant rat", "plague carrier\n", &mut fixed(&[0])),
        "plague carrier"
    );
}

#[test]
fn empty_block_yields_the_base_name() {
    assert_eq!(generate_name("giant rat", "", &mut fixed(&[])), "giant rat");
}

#[test]
fn b_prefix_caps_at_the_dll_bounds() {
    // strncpy cap 28 for the B: prefix; the appended base then fills to
    // 29 total (strncat n = 29 - len). A 30-char prefix truncates to 28,
    // the separator lands at 29, and nothing of the base fits.
    let long = "x".repeat(30);
    let name = generate_name("giant rat", &format!("B:{long}\n"), &mut fixed(&[0]));
    assert_eq!(name, format!("{} ", "x".repeat(28)));
    assert_eq!(name.len(), 29);
}

#[test]
fn walk_stops_at_the_thousandth_line() {
    let block = "F:x\n".repeat(1500);
    let mut count = 0usize;
    let mut roll = |_: i32, _: i32| {
        count += 1;
        100 // never accept
    };
    let _ = generate_name("giant rat", &block, &mut roll);
    assert_eq!(count, 1000, "one roll per composed line, 1000-line ceiling");
}

// --- spawn integration: composed name on the instance, base on death ---

const YARD: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: YARD,
        name: "Yard".into(),
        ..Default::default()
    });
    // A one-line B: block composes deterministically: whatever the roll,
    // end-of-block keeps the single candidate.
    content.add_text_block(TextBlock {
        id: TextBlockId(2000),
        next: None,
        body: "B:nasty\n".into(),
    });
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "practice dummy".into(),
        hitpoints: 1,
        energy: 0,
        name_block: Some(TextBlockId(2000)),
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

fn resident() -> Player {
    let stats = StatBlock {
        intellect: 40,
        wisdom: 30,
        strength: 60,
        health: 30,
        agility: 60,
        charm: 50,
    };
    Player {
        name: "Watcher".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 20,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: YARD,
        ..Default::default()
    }
}

#[test]
fn spawned_monster_carries_the_composed_name() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(resident());
    core.spawn_monster(MonsterId(1), YARD).unwrap();
    core.drain_events();
    core.input(s, "look");
    let events = core.drain_events();
    let listed = events.iter().any(|e| {
        matches!(e, Event::Output { session, text }
            if *session == s && text.contains("nasty practice dummy"))
    });
    assert!(listed, "also-here shows the composed name: {events:?}");
}

#[test]
fn death_line_keeps_the_base_name() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(resident());
    core.spawn_monster(MonsterId(1), YARD).unwrap();
    core.drain_events();
    core.input(s, "attack dummy");
    for _ in 0..40 {
        core.tick();
    }
    let events = core.drain_events();
    let died_plain = events.iter().any(|e| {
        matches!(e, Event::Output { text, .. } if text.contains("practice dummy is dead.")
            && !text.contains("nasty practice dummy is dead."))
    });
    assert!(died_plain, "death announcement uses the base name: {events:?}");
}
