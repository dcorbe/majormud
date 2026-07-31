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
        // Nonzero so the full-struct roundtrip assertion covers the column.
        poison: 7,
        active_spells: Default::default(),
        ..Default::default()
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
fn ansi_flag_roundtrips() {
    let db = db();
    let mut p = player("Colora");
    p.ansi = true;
    db.save_player(&p).unwrap();
    let loaded = db.load_player("Colora").unwrap().unwrap();
    assert!(loaded.ansi, "per-user ANSI persists");
}

#[test]
fn fame_and_warn_flag_roundtrip() {
    let db = db();
    let mut p = player("Rogue");
    p.fame = 85; // Criminal
    p.warn_on_evil = false;
    db.save_player(&p).unwrap();
    let loaded = db.load_player("Rogue").unwrap().unwrap();
    assert_eq!(loaded.fame, 85);
    assert!(!loaded.warn_on_evil);
}

// --- M7 slice 6: the innate/quest ability table (quests.md §1.1,
// `+0x73a`/`+0x776`) persists as player_ability, player_effect-style ---

#[test]
fn innate_abilities_roundtrip() {
    use mud_core::ability::Ability;
    let db = db();
    let mut p = player("Vexil");
    // Accuracy in slot 0, DarkDruidQuest counter in slot 7; the empty
    // slots between must survive — slot index is state (a full table
    // makes addability fail-stop), mirroring player_effect.
    p.innate[0] = (Some(Ability::from_id(22).unwrap()), 3);
    p.innate[7] = (Some(Ability::from_id(129).unwrap()), 2);
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded, p);

    // Mutate and resave: delete-before-insert leaves no stale rows.
    p.innate[0] = (None, 0);
    p.innate[7].1 = 3;
    db.save_player(&p).expect("resave");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded, p);
    assert_eq!(loaded.innate[0], (None, 0));
    assert_eq!(loaded.innate[7].1, 3);
}

#[test]
fn quest_flags_roundtrip() {
    // The `flag`-verb bits (M7 slice 6, `+0x71c`/`+0x460`): one u64
    // column, including a high-word bit (>32).
    let db = db();
    let mut p = player("Vexil");
    p.quest_flags = (1 << 2) | (1 << 39);
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Vexil").expect("query").expect("found");
    assert_eq!(loaded.quest_flags, (1 << 2) | (1 << 39));
}

#[test]
fn delete_player_purges_innate_abilities() {
    use mud_core::ability::Ability;
    let db = db();
    let mut p = player("Vexil");
    p.innate[4] = (Some(Ability::from_id(129).unwrap()), 2);
    db.save_player(&p).expect("save");
    db.delete_player("Vexil").expect("delete");
    assert_eq!(db.side_table_rows("Vexil"), 0, "no orphan rows survive");

    let fresh = player("vexil");
    db.save_player(&fresh).expect("save fresh");
    let loaded = db.load_player("vexil").expect("query").expect("found");
    assert_eq!(loaded.innate, [(None, 0); 30]);
}

// --- M7 slice 7: gangs (gangs.md §0) ---

#[test]
fn gang_roundtrip() {
    use mud_core::gang::{Gang, GANG_DISBANDED, GANG_SATURATED};
    let db = db();
    assert!(db.load_gangs().expect("load").is_empty(), "fresh db");

    let mut g = Gang::new("Iron Fist", "Salad", 1_753_900_000);
    g.exp_pool = 123_456;
    g.secondary_pool = 7;
    g.wrap = 2;
    g.member_count = 3;
    g.flags = GANG_DISBANDED | GANG_SATURATED;
    db.save_gang(&g).expect("save");
    let loaded = db.load_gangs().expect("load");
    assert_eq!(loaded, vec![g.clone()]);

    // Upsert: a resave replaces, never duplicates.
    g.exp_pool = 200_000;
    db.save_gang(&g).expect("resave");
    let loaded = db.load_gangs().expect("load");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].exp_pool, 200_000);
}

#[test]
fn player_gang_fields_roundtrip() {
    use mud_core::gang::{GF_LIEUTENANT, GF_PENDING_DEMOTE};
    let db = db();
    let mut p = player("Torgo");
    p.gang = "Iron Fist".into();
    p.gang_flags = GF_LIEUTENANT | GF_PENDING_DEMOTE;
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Torgo").expect("query").expect("found");
    assert_eq!(loaded, p);
}

#[test]
fn gang_member_scan_returns_only_members() {
    let db = db();
    let mut member = player("Torgo");
    member.gang = "Iron Fist".into();
    member.gang_flags = 0x0100;
    db.save_player(&member).expect("save member");
    db.save_player(&player("Loner")).expect("save gangless");

    let members = db.load_gang_members().expect("scan");
    assert_eq!(members, vec![("Torgo".into(), "Iron Fist".into(), 0x0100)]);
}

/// The offline-target writes (gangs.md §1.4/§1.5): promote/demote pending
/// bits and the uninvite membership clear, modeled on `bank_evil` —
/// account-scoped writes against the saved row.
#[test]
fn offline_gang_member_writes() {
    use mud_core::gang::{
        GF_LIEUTENANT, GF_PAPERWORK, GF_PENDING_PROMOTE, GF_ROSTER_ONLINE_ONLY,
    };
    let db = db();
    let mut p = player("Torgo");
    p.gang = "Iron Fist".into();
    p.gang_flags = GF_ROSTER_ONLINE_ONLY | GF_PAPERWORK;
    db.save_player(&p).expect("save");

    // Pending promote lands on the saved row (or-mask).
    db.set_player_gang_flags("Torgo", GF_PENDING_PROMOTE, !0)
        .expect("flag write");
    let loaded = db.load_player("Torgo").unwrap().unwrap();
    assert_eq!(
        loaded.gang_flags,
        GF_ROSTER_ONLINE_ONLY | GF_PAPERWORK | GF_PENDING_PROMOTE
    );

    // Offline uninvite: gang cleared, rank/pending bits cleared, the
    // player's own settings (roster view, paperwork) survive.
    db.set_player_gang_flags("Torgo", GF_LIEUTENANT, !0).unwrap();
    db.clear_player_gang("Torgo").expect("clear");
    let loaded = db.load_player("Torgo").unwrap().unwrap();
    assert_eq!(loaded.gang, "");
    assert_eq!(loaded.gang_flags, GF_ROSTER_ONLINE_ONLY | GF_PAPERWORK);

    // Unknown player: a no-op, not an error (bank_evil precedent).
    db.set_player_gang_flags("Nobody", GF_PENDING_PROMOTE, !0)
        .expect("no-op");
    db.clear_player_gang("Nobody").expect("no-op");
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
    // …and an innate slot, which needs the player_ability table the old
    // database never had (M7 slice 6).
    modern.innate[2] = (Some(mud_core::ability::Ability::from_id(129).unwrap()), 1);
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
    assert_eq!(old.poison, 0, "migrated row backfills poison to 0");
    assert_eq!(old.active_spells, [ActiveSpell::default(); 10]);
    assert!(old.ansi, "pre-M7 rows backfill ansi ON (the always-on server)");
    assert_eq!(old.fame, 0, "pre-M7 rows backfill fame 0");
    assert!(old.warn_on_evil, "pre-M7 rows backfill Warn on Evil ON");
    assert_eq!(old.gang, "", "pre-slice-7 rows backfill gangless");
    assert_eq!(old.gang_flags, 0);
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

#[test]
fn monster_kill_and_room_stamp_roundtrip() {
    // M6 slice 5: kill stamps and room respawn stamps survive a "restart"
    // (a fresh open sees the upserted wall-clock values).
    let db = db();
    db.save_monster_kill(9, 1_000_000).expect("save kill");
    db.save_monster_kill(9, 1_000_500).expect("upsert kill");
    db.save_room_stamp(1, 42, 1_000_100).expect("save stamp");
    db.save_room_stamp(1, 42, 1_000_600).expect("upsert stamp");
    db.save_room_stamp(9, 7, 1_000_200).expect("second room");
    assert_eq!(db.load_monster_kills().expect("load"), vec![(9, 1_000_500)]);
    let mut stamps = db.load_room_stamps().expect("load");
    stamps.sort();
    assert_eq!(stamps, vec![(1, 42, 1_000_600), (9, 7, 1_000_200)]);
}

// --- M7 slice 3: permadeath evil banking (crime.md §8) ---

#[test]
fn evil_banks_with_one_time_decay_and_restores() {
    let db = db();
    db.create_account("Reaper", "pw", Gender::Male).unwrap();
    const DAY: i64 = 86_400;
    // First bank on day 100: a fresh bank decays once (retention 90%).
    db.bank_evil("Reaper", 100, 100 * DAY).unwrap();
    let p = db.verify_login("Reaper", "pw").unwrap().unwrap();
    assert_eq!(p.saved_evil, 90, "banked with one-time decay");
    // Re-banking the SAME day does not decay again.
    db.bank_evil("Reaper", 100, 100 * DAY + 60).unwrap();
    let p = db.verify_login("Reaper", "pw").unwrap().unwrap();
    assert_eq!(p.saved_evil, 100, "same-day bank saves unchanged");
    // A later-day bank decays once more.
    db.bank_evil("Reaper", 100, 101 * DAY).unwrap();
    let p = db.verify_login("Reaper", "pw").unwrap().unwrap();
    assert_eq!(p.saved_evil, 90);
}

#[test]
fn negative_and_zero_fame_bank_as_zero() {
    let db = db();
    db.create_account("Pious", "pw", Gender::Male).unwrap();
    db.bank_evil("Pious", -120, 86_400).unwrap();
    let p = db.verify_login("Pious", "pw").unwrap().unwrap();
    assert_eq!(p.saved_evil, 0, "good standing does not follow the account");
}

#[test]
fn gang_shop_roundtrip() {
    use mud_core::content::ItemId;
    use mud_core::gang::{GangShopSlot, GangShopState};
    let db = db();
    assert!(db.load_gang_shops().expect("load").is_empty());
    let mut state = GangShopState {
        last_stocker: "Salad".into(),
        markup: 50,
        ..GangShopState::default()
    };
    state.slots[0] = GangShopSlot {
        item: Some(ItemId(68)),
        count: 3,
        price: 25,
        denom: 1,
    };
    state.slots[7] = GangShopSlot {
        item: Some(ItemId(844)),
        count: 1,
        price: 9999,
        denom: 4,
    };
    db.save_gang_shop(136, &state).expect("save");
    let loaded = db.load_gang_shops().expect("load");
    assert_eq!(loaded, vec![(136, state.clone())]);

    // Upsert clears stale slots.
    state.slots[7] = GangShopSlot::default();
    db.save_gang_shop(136, &state).expect("resave");
    let loaded = db.load_gang_shops().expect("load");
    assert_eq!(loaded[0].1.slots[7], GangShopSlot::default());

    // The offline deposit upserts and accumulates bank-8 rows even
    // with no player row (the book is keyed by name alone).
    db.deposit_gang_gold("Salad", 45).expect("deposit");
    db.deposit_gang_gold("Salad", 5).expect("deposit");
    let mut p = player("Salad");
    db.save_player(&p).expect("save");
    let loaded = db.load_player("Salad").expect("query").expect("found");
    assert_eq!(
        loaded.bankbooks, vec![],
        "CAVEAT (dll-shaped): a later save_player REPLACES the name's \
         bankbook rows — an online stocker must be credited in-session, \
         which is exactly what buy_from_gang_shop does"
    );
    db.deposit_gang_gold("Salad", 45).expect("deposit");
    p.bankbooks = db.load_player("Salad").expect("query").expect("found").bankbooks;
    assert_eq!(p.bankbooks, vec![(8, 45)]);
}
