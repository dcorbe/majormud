//! M7 slice 6: the `ask` verb + `ask_monster_a_question` keyword
//! dialogue (quests.md §3; cmd_ask 0x458306 decompile 53682-53727,
//! handler 0x20834 at 18070-18200). Fixture blocks mirror the shipped
//! kobold-thief conversation (31 -> 32 default, `shit:80`).

use mud_core::content::{
    Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock, TextBlock,
    TextBlockId,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Alley".into(),
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
    // The talker: keyword table 31 (default long text = its next, 32).
    content.add_monster(Monster {
        id: MonsterId(7),
        name: "kobold thief".into(),
        hitpoints: 10,
        greet_block: Some(TextBlockId(31)),
        ..Monster::default()
    });
    // The mute: no conversation block.
    content.add_monster(Monster {
        id: MonsterId(8),
        name: "giant rat".into(),
        hitpoints: 10,
        ..Monster::default()
    });
    // The quest-giver: its answer block carries a follow-up script.
    content.add_monster(Monster {
        id: MonsterId(9),
        name: "old sage".into(),
        hitpoints: 10,
        greet_block: Some(TextBlockId(33)),
        ..Monster::default()
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(31),
        next: Some(TextBlockId(32)),
        body: "shit:80".into(),
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(32),
        next: None,
        body: "The %s grins evilly at you, and eyes your purse!".into(),
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(80),
        next: None,
        body: "What a foul mouth!".into(),
    });
    // Sage: keyword `gem` speaks 85, whose NEXT (86) is the script.
    content.add_text_block(TextBlock {
        id: TextBlockId(33),
        next: None, // bare ask -> "doesn't understand you."
        body: "gem:85".into(),
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(85),
        next: Some(TextBlockId(86)),
        body: "The sage takes the gem and nods.".into(),
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(86),
        next: None,
        body: "flag 3 set".into(),
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
}

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: A,
        ..Default::default()
    }
}

fn boot_with_monster(template: u16) -> (Core, SessionId) {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Seeker"));
    core.spawn_monster(MonsterId(template), A).unwrap();
    core.drain_events();
    (core, s)
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
fn bare_ask_and_unknown_monster_fall_to_say() {
    // cmd_ask 53682: argc < 2 returns 0 -> the funnel says the line;
    // an unresolved monster word likewise (53697-53700).
    let (mut core, s) = boot_with_monster(7);
    core.input(s, "ask");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You say \"ask\""), "got: {shown:?}");
    core.input(s, "ask dragon");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You say \"ask dragon\""), "got: {shown:?}");
}

#[test]
fn mute_monster_has_nothing_to_tell() {
    // Handler step 1 (18110-18116): conversation block id 0 ->
    // "%s has nothing to tell you!" (strings dump 640).
    let (mut core, s) = boot_with_monster(8);
    core.input(s, "ask rat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("giant rat has nothing to tell you!"),
        "got: {shown:?}"
    );
}

#[test]
fn bare_question_shows_the_default_long_text() {
    // No question: the block's NEXT long text speaks, with the monster
    // name substituted (18131-18142; correction #4 — printf `%s`).
    let (mut core, s) = boot_with_monster(7);
    core.input(s, "ask kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("The kobold thief grins evilly at you, and eyes your purse!"),
        "got: {shown:?}"
    );
}

#[test]
fn bare_question_without_default_doesnt_understand() {
    // next == 0 -> "%s doesn't understand you." (strings dump 641).
    let (mut core, s) = boot_with_monster(9);
    core.input(s, "ask sage");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("old sage doesn't understand you."),
        "got: {shown:?}"
    );
}

#[test]
fn keyword_substring_match_speaks_the_answer() {
    // 18144-18160: keyword-in-answer substring, case-insensitive; the
    // shipped kobold pair (`shit:80` -> "What a foul mouth!").
    let (mut core, s) = boot_with_monster(7);
    core.input(s, "ask kobold WHAT IS THIS SHIT");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("What a foul mouth!"), "got: {shown:?}");
}

#[test]
fn unmatched_question_has_nothing_to_tell() {
    let (mut core, s) = boot_with_monster(7);
    core.input(s, "ask kobold about the weather");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("kobold thief has nothing to tell you!"),
        "got: {shown:?}"
    );
}

#[test]
fn matched_answer_runs_the_spoken_blocks_next_as_script() {
    // Correction #3 (18185-18187): display_LONG_text returns the SPOKEN
    // block's next, which feeds the unconditional runner — the quest
    // advance rides the answer.
    let (mut core, s) = boot_with_monster(9);
    core.input(s, "ask sage about the gem");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("The sage takes the gem and nods."), "got: {shown:?}");
    assert_eq!(
        core.player_snapshot(s).quest_flags,
        1 << 2,
        "block 86 (`flag 3 set`) ran"
    );
}
