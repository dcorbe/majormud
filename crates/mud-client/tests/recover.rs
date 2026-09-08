//! The recover job's pure parts: what the sweep asks for, what a reply
//! meant, and how an ending reads.

use mud_client::farm::{DIED, Phase};
use mud_client::recover::{
    GetReply, Haul, HomeWhy, RecoverEnd, Take, TakeKind, read_get_reply, sweep_list,
};
use mud_core::content::RoomId;

fn items(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn cmds(takes: &[Take]) -> Vec<&str> {
    takes.iter().map(|t| t.cmd.as_str()).collect()
}

/// Gear first, coins last, so a forced exit leaves coins behind rather
/// than equipment.
#[test]
fn the_sweep_takes_gear_before_coins_in_listed_order() {
    let list = sweep_list(&items(&[
        "5 copper farthings",
        "a rusty dagger",
        "an iron helm",
        "11 silver nobles",
        "the Book of Eldritch Lore",
    ]));
    assert_eq!(
        cmds(&list),
        vec![
            "get rusty dagger",
            "get iron helm",
            "get Book of Eldritch Lore",
            "get copper",
            "get silver"
        ]
    );
    assert_eq!(list[0].kind, TakeKind::Gear);
    assert_eq!(list[0].label, "a rusty dagger");
    assert_eq!(list[3].kind, TakeKind::Coins);
    assert_eq!(list[3].label, "5 copper farthings");
}

/// An item that wears a denomination word is still an item: only a
/// leading count makes a coin pile.
#[test]
fn a_silver_amulet_is_gear_not_silver() {
    let list = sweep_list(&items(&["a silver holy amulet"]));
    assert_eq!(cmds(&list), vec!["get silver holy amulet"]);
    assert_eq!(list[0].kind, TakeKind::Gear);
}

#[test]
fn an_empty_entry_is_skipped() {
    assert!(sweep_list(&items(&["", "  "])).is_empty());
}

#[test]
fn a_get_reply_is_read_by_its_wording() {
    assert_eq!(read_get_reply("You took a rusty dagger."), GetReply::Taken);
    assert_eq!(read_get_reply("You picked up 5 copper farthings"), GetReply::Taken);
    assert_eq!(read_get_reply("You picked up a rusty dagger"), GetReply::Taken);
    assert_eq!(read_get_reply("You don't see a rusty dagger here."), GetReply::Gone);
    assert_eq!(read_get_reply("You don't see any copper farthings"), GetReply::Gone);
    assert_eq!(read_get_reply("You took 12 damage."), GetReply::Other, "a blow is not a pickup");
    assert_eq!(read_get_reply("The giant rat swings at you but misses!"), GetReply::Other);
    assert_eq!(
        read_get_reply("You don't see that anywhere!"),
        GetReply::Other,
        "rob's missing-target refusal is not a get reply"
    );
}

fn haul(taken_items: usize, items: usize, taken_coins: usize, coins: usize) -> Haul {
    Haul {
        taken: Vec::new(),
        items_wanted: items,
        items_taken: taken_items,
        coins_wanted: coins,
        coins_taken: taken_coins,
    }
}

#[test]
fn the_haul_counts_what_it_wanted_and_what_it_took() {
    let list = sweep_list(&items(&["a rusty dagger", "an iron helm", "5 copper farthings"]));
    let mut h = Haul::wanted(&list);
    assert_eq!(h.items_wanted, 2);
    assert_eq!(h.coins_wanted, 1);
    h.took(&list[0]);
    h.took(&list[2]);
    assert_eq!(h.items_taken, 1);
    assert_eq!(h.coins_taken, 1);
    assert_eq!(h.taken, vec!["a rusty dagger", "5 copper farthings"]);
    assert_eq!(h.summary(), "1 of 2 items and 1 coin piles");
}

#[test]
fn the_summary_leaves_coins_out_when_none_were_listed() {
    assert_eq!(haul(3, 7, 0, 0).summary(), "3 of 7 items");
    assert_eq!(haul(5, 7, 2, 2).summary(), "5 of 7 items and 2 coin piles");
}

const DEATH_ROOM: RoomId = RoomId { map: 1, room: 2810 };
const HOME: RoomId = RoomId { map: 1, room: 2400 };

fn done(end: &RecoverEnd) -> (String, Option<RoomId>) {
    match end.phase() {
        Phase::Done { why, at } => (why, at),
        other => panic!("not a done phase: {other:?}"),
    }
}

#[test]
fn every_ending_reads_as_the_table_says() {
    let swept = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Swept,
        haul: haul(5, 7, 2, 2),
    };
    assert_eq!(
        done(&swept),
        ("recovered 5 of 7 items and 2 coin piles".to_string(), Some(HOME))
    );

    let broke = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Broke {
            at: DEATH_ROOM,
            name: "Darkwood Forest".into(),
        },
        haul: Haul::default(),
    };
    assert_eq!(
        done(&broke).0,
        "sneak broke at 1/2810 Darkwood Forest, nothing taken"
    );

    let attacked = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Attacked {
            at: DEATH_ROOM,
            name: "Darkwood Forest".into(),
        },
        haul: Haul::default(),
    };
    assert_eq!(done(&attacked).0, "attacked at 1/2810 Darkwood Forest, nothing taken");

    let hurt = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Hurt { mark: 70 },
        haul: haul(3, 7, 0, 0),
    };
    assert_eq!(done(&hurt).0, "hurt under 70%, back with 3 of 7 items");

    let nothing = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Nothing,
        haul: Haul::default(),
    };
    assert_eq!(done(&nothing).0, "nothing there");

    let stopped = RecoverEnd::Stopped {
        at: RoomId { map: 1, room: 2812 },
        name: "Darkwood Forest".into(),
        haul: haul(3, 7, 0, 0),
    };
    assert_eq!(
        done(&stopped),
        (
            "stopped at 1/2812 Darkwood Forest with 3 of 7 items".to_string(),
            Some(RoomId { map: 1, room: 2812 })
        )
    );

    let died = RecoverEnd::Died { haul: haul(1, 7, 0, 0) };
    assert_eq!(done(&died), (DIED.to_string(), None));
    assert_eq!(died.haul().items_taken, 1);
}

#[test]
fn the_jobs_phases_have_labels_the_bar_can_show() {
    assert_eq!(Phase::Preparing.label(), "preparing");
    assert_eq!(
        Phase::SneakingIn { to: DEATH_ROOM }.label(),
        "sneaking in to 1/2810"
    );
    assert_eq!(Phase::Sweeping { at: DEATH_ROOM }.label(), "sweeping 1/2810");
    assert_eq!(Phase::GoingHome { to: HOME }.label(), "going home to 1/2400");
    assert_eq!(
        Phase::Sweeping { at: DEATH_ROOM }.room(),
        Some(DEATH_ROOM),
        "standing still in a known room is a position"
    );
    assert_eq!(Phase::SneakingIn { to: DEATH_ROOM }.room(), None);
}

/// What a recovery runs under, from a profile that has every switch on.
/// A recovery that fights, flees or loots on its own is a different job
/// with the same name, and the operator finds out by watching the
/// character swing at what killed it.
#[test]
fn the_recovery_config_takes_the_fighting_out_of_a_walk() {
    use mud_client::bot::BotConfig;
    use mud_client::farm::FarmConfig;
    use mud_client::profile::Profile;
    use mud_client::recover::recover_config;

    let profile = Profile {
        bot: Some(BotConfig {
            auto_combat: true,
            auto_flee: true,
            auto_get: true,
            auto_sneak: true,
            ..Default::default()
        }),
        farm: Some(FarmConfig {
            fight_while_travelling: true,
            interrupt_at_percent: 60,
            depart_at_percent: Some(80),
            max_seconds: 900,
            nav: mud_client::nav::NavConfig {
                bash_doors: true,
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let (bot, farm) = recover_config(&profile);
    assert!(!bot.auto_combat, "a recovery never swings");
    assert!(!bot.auto_flee, "the job turns for home itself");
    assert!(!bot.auto_get, "the sweep does the taking");
    assert!(bot.auto_sneak, "auto_sneak is the operator's, and refusal reads it");
    assert!(!farm.fight_while_travelling, "the walk in never stops to fight");
    assert_eq!(farm.interrupt_at_percent, 0);
    assert!(!farm.nav.bash_doors, "a corpse run is quiet");
    assert_eq!(farm.depart_at_percent, None);
    assert_eq!(farm.max_seconds, 0);

    let (bot, farm) = recover_config(&Profile::default());
    assert!(!bot.auto_combat && !bot.auto_flee && !bot.auto_get);
    assert!(!farm.fight_while_travelling && !farm.nav.bash_doors);
    assert_eq!(farm.interrupt_at_percent, 0);
}


/// Where `/recover` goes when the operator typed a room.
#[test]
fn a_typed_target_resolves_and_a_typed_nonsense_name_refuses() {
    use mud_client::recover::target_of;

    let graph = target_graph();
    let log = target_log("typed");
    assert_eq!(
        target_of(&graph, None, Some("1/2810"), "beef", &log),
        Ok(DEATH_ROOM)
    );
    assert_eq!(
        target_of(&graph, None, Some("Sunken Vault"), "beef", &log),
        Ok(DEATH_ROOM)
    );
    let why = target_of(&graph, None, Some("no such place"), "beef", &log)
        .expect_err("a name that names nothing refuses");
    assert!(
        why.iter().all(|l| l.starts_with("recover:")),
        "the refusal names the verb that asked: {why:?}"
    );
}

/// With no room typed, the target is the newest logged death that
/// carries one, and an empty log is a refusal rather than a guess.
#[test]
fn an_untyped_target_reads_the_log_and_refuses_when_it_holds_nothing() {
    use mud_client::deathlog::{Death, FixWord, record_in};
    use mud_client::recover::target_of;

    let graph = target_graph();
    let log = target_log("untyped");
    assert_eq!(
        target_of(&graph, None, None, "beef", &log),
        Err(vec![
            "-- recover: no logged death with a room. /recover <room> --".to_string()
        ]),
        "no file at all is an empty log"
    );

    record_in(
        &log,
        &Death {
            stamp: "2026-09-07T14:42:07Z".into(),
            character: "beef".into(),
            room: Some(DEATH_ROOM),
            name: "Sunken Vault".into(),
            fix: FixWord::Confirmed,
        },
    )
    .unwrap();
    assert_eq!(target_of(&graph, None, None, "beef", &log), Ok(DEATH_ROOM));
    assert_eq!(
        target_of(&graph, None, None, "porkchop", &log),
        Err(vec![
            "-- recover: no logged death with a room. /recover <room> --".to_string()
        ]),
        "another character's death is not this one's target"
    );
}

/// The one room a typed target can name.
fn target_graph() -> mud_client::graph::RoomGraph {
    mud_client::graph::RoomGraph::from_rooms(vec![(
        DEATH_ROOM,
        mud_client::graph::GraphRoom {
            name: "Sunken Vault".into(),
            ..Default::default()
        },
    )])
}

/// A scratch log under the test target directory, never the real one.
fn target_log(test: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("recover_target")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("deaths.log")
}

