//! Tests for the runtime state database: accounts and player persistence.

use mud_core::content::{ClassId, RaceId, RoomId, StatBlock};
use mud_core::game::{Gender, Player};
use mud_server::state_db::{CreateAccountError, StateDb};

fn db() -> StateDb {
    StateDb::open_in_memory().expect("open state db")
}

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Female,
        race: RaceId(2),
        class: ClassId(1),
        level: 1,
        stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        hp_base: 4,
        current_hp: 35,
        current_mana: 0,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        cp_unspent: 100,
        cp_lifetime: 100,
        lives: 9,
        experience: 0,
        location: RoomId { map: 1, room: 1 },
    }
}

#[test]
fn account_roundtrip_and_login() {
    let db = db();
    db.create_account("Alice", "hunter2", Gender::Female)
        .expect("create");

    let profile = db.verify_login("Alice", "hunter2").expect("query");
    let profile = profile.expect("valid credentials accepted");
    assert_eq!(profile.name, "Alice");
    assert_eq!(profile.gender, Gender::Female);

    assert!(db.verify_login("Alice", "wrong").unwrap().is_none());
    assert!(db.verify_login("Nobody", "hunter2").unwrap().is_none());
}

#[test]
fn login_name_is_case_insensitive_but_canonical_name_returned() {
    let db = db();
    db.create_account("Alice", "pw", Gender::Female).unwrap();
    let profile = db.verify_login("aLiCe", "pw").unwrap().expect("login ok");
    assert_eq!(profile.name, "Alice");
}

#[test]
fn duplicate_account_names_are_rejected_case_insensitively() {
    let db = db();
    db.create_account("Alice", "pw", Gender::Female).unwrap();
    assert!(matches!(
        db.create_account("ALICE", "other", Gender::Male),
        Err(CreateAccountError::NameTaken)
    ));
}

#[test]
fn passwords_are_not_stored_in_plaintext() {
    let db = db();
    db.create_account("Alice", "hunter2", Gender::Female).unwrap();
    assert!(!db.raw_password_field("Alice").contains("hunter2"));
}

#[test]
fn player_save_and_load_roundtrip() {
    let db = db();
    let p = player("Alice");
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Alice").expect("query").expect("found");
    assert_eq!(loaded, p);
}

#[test]
fn missing_player_loads_as_none() {
    let db = db();
    assert!(db.load_player("Nobody").expect("query").is_none());
}

#[test]
fn saving_again_updates_the_record() {
    let db = db();
    let mut p = player("Alice");
    db.save_player(&p).unwrap();
    p.location = RoomId { map: 3, room: 77 };
    p.experience = 1234;
    db.save_player(&p).unwrap();
    let loaded = db.load_player("Alice").unwrap().unwrap();
    assert_eq!(loaded.location, RoomId { map: 3, room: 77 });
    assert_eq!(loaded.experience, 1234);
}
