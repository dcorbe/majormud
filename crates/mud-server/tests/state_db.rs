//! Tests for the runtime state database: accounts and player persistence.

use mud_core::content::{ClassId, ItemId, RaceId, RoomId, SpellId, StatBlock};
use mud_core::game::{ActiveSpell, Gender, Player};
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
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 100,
        cp_lifetime: 100,
        lives: 9,
        experience: 0,
        location: RoomId { map: 1, room: 1 },
        spellbook: Default::default(),
        active_spells: Default::default(),
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

#[test]
fn spellbook_roundtrips() {
    let db = db();
    let mut p = player("Vexil");
    p.spellbook.insert(SpellId(1), false); // learned
    p.spellbook.insert(SpellId(129), true); // temporary (GiveTempSpell)
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded.spellbook, p.spellbook);

    // Mutate and resave: delete-before-insert means the stored book
    // matches exactly — no stale rows survive a save.
    p.spellbook.remove(&SpellId(1));
    p.spellbook.insert(SpellId(129), false); // temp grant became learned
    db.save_player(&p).expect("resave");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded.spellbook, p.spellbook);
}

#[test]
fn active_spells_roundtrip() {
    let db = db();
    let mut p = player("Vexil");
    // Blur in slot 0, a second buff in slot 3; empty slots in between must
    // survive the trip (slot index is part of the state, not an artifact).
    p.active_spells[0] = ActiveSpell {
        spell: Some(SpellId(129)),
        value: 5,
        remaining: 70,
    };
    p.active_spells[3] = ActiveSpell {
        spell: Some(SpellId(157)),
        value: -2,
        remaining: 1,
    };
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded, p);

    // Mutate and resave: the upkeep tick decays a slot and one expires.
    // Delete-before-insert means no stale rows survive.
    p.active_spells[0].remaining = 42;
    p.active_spells[3] = ActiveSpell::default();
    db.save_player(&p).expect("resave");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded, p);
    assert_eq!(loaded.active_spells[0].remaining, 42);
    assert_eq!(loaded.active_spells[3], ActiveSpell::default());
}

#[test]
fn delete_player_purges_everything() {
    let db = db();
    let mut p = player("Vexil");
    p.inventory.push((ItemId(3), 0));
    p.bankbooks.push((45, 1000));
    p.spellbook.insert(SpellId(1), false);
    p.active_spells[0] = ActiveSpell {
        spell: Some(SpellId(129)),
        value: 5,
        remaining: 70,
    };
    db.save_player(&p).expect("save");

    db.delete_player("Vexil").expect("delete");
    assert!(db.load_player("Vexil").expect("query").is_none());
    assert_eq!(db.side_table_rows("Vexil"), 0, "no orphan rows survive");

    // A fresh same-name character (names collate NOCASE) must not
    // inherit the dead one's belongings.
    let fresh = player("vexil");
    db.save_player(&fresh).expect("save fresh");
    let loaded = db.load_player("vexil").expect("query").expect("found");
    assert_eq!(loaded, fresh);
    assert!(loaded.inventory.is_empty());
    assert!(loaded.bankbooks.is_empty());
    assert!(loaded.spellbook.is_empty());
    assert_eq!(loaded.active_spells, [ActiveSpell::default(); 10]);
}

/// The schema as first shipped (commit 02f029a): the shape of a live
/// `state.sqlite` from before base stats, HP, hunger/thirst, coins, and
/// lawful were added to the player table.
const OLD_SCHEMA: &str = "
CREATE TABLE account (
    name          TEXT PRIMARY KEY COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    gender        TEXT NOT NULL CHECK (gender IN ('M', 'F'))
) STRICT;
CREATE TABLE player (
    name        TEXT PRIMARY KEY COLLATE NOCASE,
    gender      TEXT NOT NULL CHECK (gender IN ('M', 'F')),
    race        INTEGER NOT NULL,
    class       INTEGER NOT NULL,
    level       INTEGER NOT NULL,
    intellect   INTEGER NOT NULL,
    wisdom      INTEGER NOT NULL,
    strength    INTEGER NOT NULL,
    health      INTEGER NOT NULL,
    agility     INTEGER NOT NULL,
    charm       INTEGER NOT NULL,
    cp_unspent  INTEGER NOT NULL,
    cp_lifetime INTEGER NOT NULL,
    lives       INTEGER NOT NULL,
    experience  INTEGER NOT NULL,
    map         INTEGER NOT NULL,
    room        INTEGER NOT NULL
) STRICT;
";

#[test]
fn old_database_is_migrated_on_open() {
    let path = std::env::temp_dir().join(format!(
        "mud_state_migrate_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    // A database written by the original server: old player table, one
    // character saved by the old 17-column save_player.
    {
        let conn = rusqlite::Connection::open(&path).expect("create old db");
        conn.execute_batch(OLD_SCHEMA).expect("old schema");
        conn.execute(
            "INSERT INTO player (name, gender, race, class, level,
                 intellect, wisdom, strength, health, agility, charm,
                 cp_unspent, cp_lifetime, lives, experience, map, room)
             VALUES ('Daniela', 'F', 2, 1, 3,
                 30, 50, 50, 50, 30, 30,
                 100, 100, 9, 500, 1, 1)",
            [],
        )
        .expect("old-shaped player row");
    }

    let db = StateDb::open(&path).expect("open migrates old schema");

    // (a) Saving a modern player must succeed against the migrated table —
    // including an active spell, which needs the player_effect table the
    // old database never had (CREATE TABLE IF NOT EXISTS adds it on open).
    let mut modern = player("Newbie");
    modern.active_spells[0] = ActiveSpell {
        spell: Some(SpellId(129)),
        value: 5,
        remaining: 70,
    };
    db.save_player(&modern).expect("modern save succeeds");
    let loaded = db.load_player("Newbie").expect("query").expect("found");
    assert_eq!(loaded, modern);

    // (b) The old row loads, with base stats backfilled from the effective
    // stats and current_hp backfilled to conscious.
    let old = db.load_player("Daniela").expect("query").expect("found");
    assert_eq!(old.base_stats, old.stats, "b_* backfilled from effective");
    assert_eq!(old.stats.wisdom, 50);
    assert_eq!(old.stats.intellect, 30);
    assert_eq!(old.current_hp, 1, "loads conscious, not downed");
    assert_eq!(old.level, 3);
    assert_eq!(old.experience, 500);
    assert!(!old.lawful);
    assert_eq!(old.coins, Default::default());
    assert_eq!(old.active_spells, [ActiveSpell::default(); 10]);
    drop(db);

    // (c) Reopening is idempotent: same data, still writable.
    let db = StateDb::open(&path).expect("reopen migrated db");
    let again = db.load_player("Daniela").expect("query").expect("found");
    assert_eq!(again, old);
    db.save_player(&player("Second")).expect("save after reopen");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn shop_stock_save_and_load_roundtrip() {
    let db = db();
    assert!(db.load_shop_stock().expect("load").is_empty(), "fresh db");

    let mut counts = [0i16; 20];
    counts[0] = 4;
    counts[3] = 31;
    db.save_shop_stock(45, &counts).expect("save");
    counts[0] = 5;
    db.save_shop_stock(45, &counts).expect("upsert");
    db.save_shop_stock(8, &[1i16; 20]).expect("second shop");

    let rows = db.load_shop_stock().expect("load");
    assert!(rows.contains(&(45, 0, 5)), "upserted: {rows:?}");
    assert!(rows.contains(&(45, 3, 31)), "got: {rows:?}");
    assert!(rows.contains(&(8, 19, 1)), "got: {rows:?}");
    assert_eq!(rows.len(), 40, "20 slots per saved shop");
}
