//! The runtime state database (`state.sqlite`): accounts and saved players.
//!
//! This is the standalone server's replacement for the Worldgroup account
//! system and the WCCUSER2 player file. Single writer (the server); passwords
//! are argon2id hashes. The player schema mirrors `mud_core::game::Player`
//! field-for-field and grows with it; databases written by older servers
//! gain any missing columns on open (see [`TABLES`] and
//! `StateDb::add_missing_columns`).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use mud_core::content::{ClassId, ItemId, RaceId, RoomId, SpellId, StatBlock};
use mud_core::game::{AccountProfile, ActiveSpell, Coins, Gender, Player};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug)]
pub enum StateError {
    Db(rusqlite::Error),
    Hash(argon2::password_hash::Error),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Db(e) => write!(f, "state database error: {e}"),
            StateError::Hash(e) => write!(f, "password hash error: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<rusqlite::Error> for StateError {
    fn from(e: rusqlite::Error) -> Self {
        StateError::Db(e)
    }
}

impl From<argon2::password_hash::Error> for StateError {
    fn from(e: argon2::password_hash::Error) -> Self {
        StateError::Hash(e)
    }
}

#[derive(Debug)]
pub enum CreateAccountError {
    NameTaken,
    Other(StateError),
}

/// One table of the canonical schema. `StateDb::init` both creates missing
/// tables from this and `ALTER TABLE ... ADD COLUMN`s any column missing
/// from a pre-existing table, so this is the single source of truth for the
/// on-disk shape — add new columns here and old databases pick them up on
/// the next open.
struct TableDef {
    name: &'static str,
    /// `(column name, type + column constraints)` in canonical order.
    columns: &'static [(&'static str, &'static str)],
    /// Trailing table-level constraint (composite PRIMARY KEY), or `""`.
    constraint: &'static str,
}

const TABLES: &[TableDef] = &[
    TableDef {
        name: "account",
        columns: &[
            ("name", "TEXT PRIMARY KEY COLLATE NOCASE"),
            ("password_hash", "TEXT NOT NULL"),
            ("gender", "TEXT NOT NULL CHECK (gender IN ('M', 'F'))"),
            // crime.md §8: evil banked at permadeath (the GENBB "WCC
            // MAJOR MUD EVIL" analog) + the once-per-day decay stamp.
            ("saved_evil", "INTEGER NOT NULL"),
            ("saved_evil_day", "INTEGER NOT NULL"),
        ],
        constraint: "",
    },
    TableDef {
        name: "player",
        columns: &[
            ("name", "TEXT PRIMARY KEY COLLATE NOCASE"),
            ("gender", "TEXT NOT NULL CHECK (gender IN ('M', 'F'))"),
            ("race", "INTEGER NOT NULL"),
            ("class", "INTEGER NOT NULL"),
            ("level", "INTEGER NOT NULL"),
            ("intellect", "INTEGER NOT NULL"),
            ("wisdom", "INTEGER NOT NULL"),
            ("strength", "INTEGER NOT NULL"),
            ("health", "INTEGER NOT NULL"),
            ("agility", "INTEGER NOT NULL"),
            ("charm", "INTEGER NOT NULL"),
            ("b_intellect", "INTEGER NOT NULL"),
            ("b_wisdom", "INTEGER NOT NULL"),
            ("b_strength", "INTEGER NOT NULL"),
            ("b_health", "INTEGER NOT NULL"),
            ("b_agility", "INTEGER NOT NULL"),
            ("b_charm", "INTEGER NOT NULL"),
            ("hp_base", "INTEGER NOT NULL"),
            ("current_hp", "INTEGER NOT NULL"),
            ("current_mana", "INTEGER NOT NULL"),
            ("hunger", "INTEGER NOT NULL"),
            ("thirst", "INTEGER NOT NULL"),
            ("runic", "INTEGER NOT NULL"),
            ("platinum", "INTEGER NOT NULL"),
            ("gold", "INTEGER NOT NULL"),
            ("silver", "INTEGER NOT NULL"),
            ("copper", "INTEGER NOT NULL"),
            ("lawful", "INTEGER NOT NULL CHECK (lawful IN (0, 1))"),
            ("cp_unspent", "INTEGER NOT NULL"),
            ("cp_lifetime", "INTEGER NOT NULL"),
            ("lives", "INTEGER NOT NULL"),
            ("experience", "INTEGER NOT NULL"),
            ("map", "INTEGER NOT NULL"),
            ("room", "INTEGER NOT NULL"),
            // The poison counter (+0xbe) — M5 slice 5; older databases
            // gain it with a 0 default on open.
            ("poison", "INTEGER NOT NULL"),
            // Per-user ANSI (M7 slice 2, ours). Migrated rows backfill 1:
            // pre-toggle characters only ever ran on the always-on server.
            ("ansi", "INTEGER NOT NULL CHECK (ansi IN (0, 1))"),
            // Crime (M7 slice 3): fame (+0x542) and Warn on Evil
            // (+0x700 & 0x10, backfilled ON — the DLL's creation default).
            ("fame", "INTEGER NOT NULL"),
            ("warn_on_evil", "INTEGER NOT NULL CHECK (warn_on_evil IN (0, 1))"),
            // Quest VM script flags (M7 slice 6): the 64 `flag`-verb bits
            // (`+0x71c`/`+0x460`); pre-slice-6 rows backfill 0.
            ("quest_flags", "INTEGER NOT NULL"),
            // Gangs (M7 slice 7, gangs.md §0): the gang display name
            // (+0x6c8, '' = none) and the rank/notice word (+0x7d4).
            // Pre-slice-7 rows backfill gangless — correct, gangs did
            // not exist.
            ("gang", "TEXT NOT NULL"),
            ("gang_flags", "INTEGER NOT NULL"),
        ],
        constraint: "",
    },
    // M7 slice 7: the WCCGANG2 record (gangs.md §0). One row per gang,
    // keyed by the uppercase name key; membership itself lives on the
    // player rows (the durable truth the boot scan rebuilds the mirror
    // from). Disbanded rows persist until the login sweep drains them.
    TableDef {
        name: "gang",
        columns: &[
            ("name", "TEXT PRIMARY KEY"),
            ("display", "TEXT NOT NULL"),
            ("exp_pool", "INTEGER NOT NULL"),
            ("secondary_pool", "INTEGER NOT NULL"),
            ("wrap", "INTEGER NOT NULL"),
            ("leader", "TEXT NOT NULL"),
            ("created", "INTEGER NOT NULL"),
            ("member_count", "INTEGER NOT NULL"),
            ("flags", "INTEGER NOT NULL"),
        ],
        constraint: "",
    },
    TableDef {
        name: "bankbook",
        columns: &[
            ("name", "TEXT NOT NULL COLLATE NOCASE"),
            ("shop", "INTEGER NOT NULL"),
            ("balance", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (name, shop)",
    },
    TableDef {
        name: "shop_stock",
        columns: &[
            ("shop", "INTEGER NOT NULL"),
            ("slot", "INTEGER NOT NULL"),
            ("now", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (shop, slot)",
    },
    TableDef {
        name: "player_item",
        columns: &[
            ("name", "TEXT NOT NULL COLLATE NOCASE"),
            ("kind", "TEXT NOT NULL CHECK (kind IN ('inv', 'worn', 'weapon'))"),
            ("slot", "INTEGER NOT NULL"),
            ("item", "INTEGER NOT NULL"),
            ("uses", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (name, kind, slot)",
    },
    TableDef {
        name: "player_spell",
        columns: &[
            ("name", "TEXT NOT NULL COLLATE NOCASE"),
            ("spell", "INTEGER NOT NULL"),
            ("temporary", "INTEGER NOT NULL CHECK (temporary IN (0, 1))"),
        ],
        constraint: "PRIMARY KEY (name, spell)",
    },
    // Active duration-spell slots (spellcasting.md §1: 10 per player).
    // Only occupied slots are stored; `slot` is the array index, preserved
    // so slot order — and slot exhaustion — survives a reload.
    TableDef {
        name: "player_effect",
        columns: &[
            ("name", "TEXT NOT NULL COLLATE NOCASE"),
            ("slot", "INTEGER NOT NULL"),
            ("spell", "INTEGER NOT NULL"),
            ("value", "INTEGER NOT NULL"),
            ("remaining", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (name, slot)",
    },
    // M7 slice 6: the innate/quest ability table (quests.md §1.1 —
    // `+0x73a[30]` ids / `+0x776[30]` values). Only occupied slots are
    // stored; `slot` is the array index, preserved so slot order — and
    // slot exhaustion (addability fail-stops on a full table) — survives
    // a reload.
    TableDef {
        name: "player_ability",
        columns: &[
            ("name", "TEXT NOT NULL COLLATE NOCASE"),
            ("slot", "INTEGER NOT NULL"),
            ("ability", "INTEGER NOT NULL"),
            ("value", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (name, slot)",
    },
    // M7 slice 7: gang-shop runtime shelves (gangs.md §3.1 — the DLL
    // overloads the shop record; ours persist here).
    TableDef {
        name: "gang_shop",
        columns: &[
            ("shop", "INTEGER PRIMARY KEY"),
            ("last_stocker", "TEXT NOT NULL"),
            ("markup", "INTEGER NOT NULL"),
        ],
        constraint: "",
    },
    TableDef {
        name: "gang_shop_slot",
        columns: &[
            ("shop", "INTEGER NOT NULL"),
            ("slot", "INTEGER NOT NULL"),
            ("item", "INTEGER NOT NULL"),
            ("count", "INTEGER NOT NULL"),
            ("price", "INTEGER NOT NULL"),
            ("denom", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (shop, slot)",
    },
    // M6 slice 5: limited-population kill stamps (knmsr+0xb4/+0xb6).
    // `killed_at` is wall-clock seconds since the Unix epoch; the boot
    // path hands the CORE the elapsed seconds.
    TableDef {
        name: "monster_population",
        columns: &[
            ("template", "INTEGER NOT NULL"),
            ("killed_at", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (template)",
    },
    // M6 slice 5: room respawn stamps (room+0x562, the dirty-flag save) —
    // a restart no longer resets the refill window.
    TableDef {
        name: "room_stamp",
        columns: &[
            ("map", "INTEGER NOT NULL"),
            ("room", "INTEGER NOT NULL"),
            ("stamped_at", "INTEGER NOT NULL"),
        ],
        constraint: "PRIMARY KEY (map, room)",
    },
];

fn create_table_sql(table: &TableDef) -> String {
    let mut parts: Vec<String> = table
        .columns
        .iter()
        .map(|(name, def)| format!("{name} {def}"))
        .collect();
    if !table.constraint.is_empty() {
        parts.push(table.constraint.to_string());
    }
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({}) STRICT",
        table.name,
        parts.join(", ")
    )
}

pub struct StateDb {
    conn: Connection,
}

fn gender_str(g: Gender) -> &'static str {
    match g {
        Gender::Male => "M",
        Gender::Female => "F",
    }
}

fn gender_from(s: &str) -> Gender {
    match s {
        "M" => Gender::Male,
        "F" => Gender::Female,
        other => unreachable!("CHECK constraint admits only M/F, got {other:?}"),
    }
}

impl StateDb {
    pub fn open(path: &Path) -> Result<StateDb, StateError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<StateDb, StateError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<StateDb, StateError> {
        for table in TABLES {
            conn.execute_batch(&create_table_sql(table))?;
        }
        Self::add_missing_columns(&conn)?;
        Ok(StateDb { conn })
    }

    /// Brings a database written by an older server up to the canonical
    /// schema. `CREATE TABLE IF NOT EXISTS` never touches an existing
    /// table, so every column added after a table first shipped must be
    /// `ALTER TABLE ... ADD COLUMN`ed in here — without this, `save_player`
    /// fails ("table player has no column named ...") on every save
    /// against an old file and the character silently stops persisting.
    fn add_missing_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
        for table in TABLES {
            let existing: std::collections::HashSet<String> = conn
                .prepare("SELECT name FROM pragma_table_info(?1)")?
                .query_map(params![table.name], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for (col, def) in table.columns {
                if existing.contains(*col) {
                    continue;
                }
                // STRICT + NOT NULL: ADD COLUMN requires an explicit
                // default. Migrated tables keep the DEFAULT clause; every
                // write names all columns, so it is never consulted again.
                let default = if def.starts_with("TEXT") { "''" } else { "0" };
                conn.execute_batch(&format!(
                    "ALTER TABLE {} ADD COLUMN {col} {def} DEFAULT {default}",
                    table.name
                ))?;
                Self::backfill(conn, table.name, col)?;
            }
        }
        Ok(())
    }

    /// Targeted backfill for freshly added player columns where the zero
    /// default would be wrong. Only pre-migration rows are affected; both
    /// choices are documented approximations.
    fn backfill(conn: &Connection, table: &str, col: &str) -> Result<(), rusqlite::Error> {
        if table != "player" {
            return Ok(());
        }
        if let Some(effective) = col.strip_prefix("b_") {
            // Old rows never stored base stats. Effective stats equal the
            // base copy except while buffs are active (creation sets both
            // from the race template; buffs modify `stats` only), so the
            // effective column is the least-wrong source — worst case, a
            // buff active at save time gets baked into the base.
            conn.execute(&format!("UPDATE player SET {col} = {effective}"), [])?;
        } else if col == "current_hp" {
            // 0 HP would load the character downed and writhing (and
            // rolling toward the death floor); 1 HP loads them conscious
            // and slow-tick regeneration heals them back to max.
            conn.execute("UPDATE player SET current_hp = 1", [])?;
        } else if col == "ansi" {
            // Pre-toggle characters only ever played on the always-on
            // server; keep their colour.
            conn.execute("UPDATE player SET ansi = 1", [])?;
        } else if col == "warn_on_evil" {
            // The DLL's creation default — evil actions refuse until the
            // player opts out with `SET WARNING OFF`.
            conn.execute("UPDATE player SET warn_on_evil = 1", [])?;
        }
        Ok(())
    }

    pub fn create_account(
        &self,
        name: &str,
        password: &str,
        gender: Gender,
    ) -> Result<(), CreateAccountError> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| CreateAccountError::Other(e.into()))?
            .to_string();
        let result = self.conn.execute(
            "INSERT INTO account (name, password_hash, gender, saved_evil, saved_evil_day) \
             VALUES (?1, ?2, ?3, 0, 0)",
            params![name, hash, gender_str(gender)],
        );
        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(CreateAccountError::NameTaken)
            }
            Err(e) => Err(CreateAccountError::Other(e.into())),
        }
    }

    /// Checks credentials; `Ok(None)` covers both unknown account and wrong
    /// password (callers must not reveal which).
    pub fn verify_login(
        &self,
        name: &str,
        password: &str,
    ) -> Result<Option<AccountProfile>, StateError> {
        let row: Option<(String, String, String, i64)> = self
            .conn
            .query_row(
                "SELECT name, password_hash, gender, saved_evil FROM account WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((canonical, hash, gender, saved_evil)) = row else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&hash)?;
        if Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        Ok(Some(AccountProfile {
            name: canonical,
            gender: gender_from(&gender),
            saved_evil: i16::try_from(saved_evil).unwrap_or(0),
        }))
    }

    /// crime.md §8: bank a permadying character's evil to the account.
    /// The retention decay multiplies ONCE per calendar day regardless of
    /// elapsed days (§8 item 2); negative/zero fame banks 0 — creation
    /// clamps restored negatives anyway (§6.8), so good standing never
    /// survives the account round-trip. Retention percent: the DLL's
    /// option 0x32 default is unread — 90 chosen, ORACLE-VERIFY.
    pub fn bank_evil(&self, name: &str, fame: i16, wall_secs: i64) -> Result<(), StateError> {
        const RETENTION_PCT: i64 = 90;
        let today = wall_secs / 86_400;
        let prev_day: Option<i64> = self
            .conn
            .query_row(
                "SELECT saved_evil_day FROM account WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        let Some(prev_day) = prev_day else {
            return Ok(()); // no account (fixture player) — nothing to bank
        };
        let mut value = i64::from(fame.max(0));
        if prev_day != today {
            value = value * RETENTION_PCT / 100;
        }
        self.conn.execute(
            "UPDATE account SET saved_evil = ?2, saved_evil_day = ?3 WHERE name = ?1",
            params![name, value, today],
        )?;
        Ok(())
    }

    pub fn account_exists(&self, name: &str) -> Result<bool, StateError> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM account WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Test hook: the stored password field for an account.
    pub fn raw_password_field(&self, name: &str) -> String {
        self.conn
            .query_row(
                "SELECT password_hash FROM account WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .expect("account exists")
    }

    /// Test hook: rows for `name` remaining across the per-player side
    /// tables (player_item, bankbook, player_spell, player_effect).
    pub fn side_table_rows(&self, name: &str) -> usize {
        [
            "player_item",
            "bankbook",
            "player_spell",
            "player_effect",
            "player_ability",
        ]
            .iter()
            .map(|table| {
                self.conn
                    .query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE name = ?1"),
                        params![name],
                        |r| r.get::<_, i64>(0),
                    )
                    .expect("count rows") as usize
            })
            .sum()
    }

    pub fn save_player(&self, player: &Player) -> Result<(), StateError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM player_item WHERE name = ?1",
            params![player.name],
        )?;
        tx.execute(
            "DELETE FROM bankbook WHERE name = ?1",
            params![player.name],
        )?;
        for (shop, balance) in &player.bankbooks {
            tx.execute(
                "INSERT INTO bankbook (name, shop, balance) VALUES (?1, ?2, ?3)",
                params![player.name, shop, *balance as i64],
            )?;
        }
        let groups: [(&str, &[(ItemId, i16)]); 3] = [
            ("inv", &player.inventory),
            ("worn", &player.worn),
            ("weapon", player.weapon.as_slice()),
        ];
        for (kind, items) in groups {
            for (slot, (item, uses)) in items.iter().enumerate() {
                tx.execute(
                    "INSERT INTO player_item (name, kind, slot, item, uses) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![player.name, kind, slot as i64, item.0, uses],
                )?;
            }
        }
        tx.execute(
            "DELETE FROM player_spell WHERE name = ?1",
            params![player.name],
        )?;
        for (spell, temporary) in &player.spellbook {
            tx.execute(
                "INSERT INTO player_spell (name, spell, temporary) VALUES (?1, ?2, ?3)",
                params![player.name, spell.0, i64::from(*temporary)],
            )?;
        }
        tx.execute(
            "DELETE FROM player_effect WHERE name = ?1",
            params![player.name],
        )?;
        for (slot, active) in player.active_spells.iter().enumerate() {
            let Some(spell) = active.spell else { continue };
            tx.execute(
                "INSERT INTO player_effect (name, slot, spell, value, remaining) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![player.name, slot as i64, spell.0, active.value, active.remaining],
            )?;
        }
        tx.execute(
            "DELETE FROM player_ability WHERE name = ?1",
            params![player.name],
        )?;
        for (slot, (ability, value)) in player.innate.iter().enumerate() {
            let Some(ability) = ability else { continue };
            tx.execute(
                "INSERT INTO player_ability (name, slot, ability, value) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![player.name, slot as i64, ability.id(), value],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO player (name, gender, race, class, level,
                 intellect, wisdom, strength, health, agility, charm,
                 b_intellect, b_wisdom, b_strength, b_health, b_agility,
                 b_charm, hp_base, current_hp, current_mana, hunger, thirst,
                 runic, platinum, gold, silver, copper, lawful,
                 cp_unspent, cp_lifetime, lives, experience, map, room,
                 poison, ansi, fame, warn_on_evil, quest_flags, gang,
                 gang_flags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                 ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37,
                 ?38, ?39, ?40, ?41)",
            params![
                player.name,
                gender_str(player.gender),
                player.race.0,
                player.class.0,
                player.level,
                player.stats.intellect,
                player.stats.wisdom,
                player.stats.strength,
                player.stats.health,
                player.stats.agility,
                player.stats.charm,
                player.base_stats.intellect,
                player.base_stats.wisdom,
                player.base_stats.strength,
                player.base_stats.health,
                player.base_stats.agility,
                player.base_stats.charm,
                player.hp_base,
                player.current_hp,
                player.current_mana,
                player.hunger,
                player.thirst,
                player.coins.runic,
                player.coins.platinum,
                player.coins.gold,
                player.coins.silver,
                player.coins.copper,
                player.lawful,
                player.cp_unspent,
                player.cp_lifetime,
                player.lives,
                player.experience,
                player.location.map,
                player.location.room,
                player.poison,
                player.ansi,
                player.fame,
                player.warn_on_evil,
                player.quest_flags as i64,
                player.gang,
                player.gang_flags,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Upserts one gang record (the gang dirty-byte save, gangs.md §0).
    pub fn save_gang(&self, gang: &mud_core::gang::Gang) -> Result<(), StateError> {
        self.conn.prepare_cached(
            "INSERT OR REPLACE INTO gang (name, display, exp_pool,
                 secondary_pool, wrap, leader, created, member_count, flags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?.execute(params![
            gang.name_key,
            gang.display,
            gang.exp_pool,
            gang.secondary_pool,
            gang.wrap,
            gang.leader,
            gang.created,
            gang.member_count,
            gang.flags,
        ])?;
        Ok(())
    }

    /// Every gang row (disbanded included — the login sweep needs them),
    /// for `CoreConfig.restored_gangs` at boot.
    pub fn load_gangs(&self) -> Result<Vec<mud_core::gang::Gang>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, display, exp_pool, secondary_pool, wrap, leader,
                 created, member_count, flags
             FROM gang ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(mud_core::gang::Gang {
                    name_key: r.get(0)?,
                    display: r.get(1)?,
                    exp_pool: r.get(2)?,
                    secondary_pool: r.get(3)?,
                    wrap: r.get(4)?,
                    leader: r.get(5)?,
                    created: r.get(6)?,
                    member_count: r.get(7)?,
                    flags: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The boot membership scan: (name, gang, gang_flags) for every
    /// player row with a gang — feeds the Core's offline-roster mirror
    /// (`CoreConfig.restored_gang_members`).
    pub fn load_gang_members(&self) -> Result<Vec<(String, String, u16)>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, gang, gang_flags FROM player WHERE gang != '' ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Offline-target gang-flag write (gangs.md §1.5: pending
    /// promote/demote land on the saved record). New word =
    /// `(old & and_mask) | or_mask`; unknown player = no-op (the
    /// `bank_evil` tolerance).
    pub fn set_player_gang_flags(
        &self,
        name: &str,
        or_mask: u16,
        and_mask: u16,
    ) -> Result<(), StateError> {
        self.conn.execute(
            "UPDATE player SET gang_flags = (gang_flags & ?3) | ?2 WHERE name = ?1",
            params![name, or_mask, and_mask],
        )?;
        Ok(())
    }

    /// Offline uninvite (`remove_offline_user_from_gang`, gangs.md §1.4):
    /// clears the membership string and the rank/pending bits. The
    /// player's own settings bits (roster view 0x8, paperwork 0x4000)
    /// survive — they are commerce/preference state, not membership.
    pub fn clear_player_gang(&self, name: &str) -> Result<(), StateError> {
        use mud_core::gang::{GF_LIEUTENANT, GF_PENDING_DEMOTE, GF_PENDING_PROMOTE};
        let clear = !(GF_LIEUTENANT | GF_PENDING_PROMOTE | GF_PENDING_DEMOTE);
        self.conn.execute(
            "UPDATE player SET gang = '', gang_flags = gang_flags & ?2 WHERE name = ?1",
            params![name, clear],
        )?;
        Ok(())
    }

    /// Upserts a gang shop's runtime shelves (gangs.md §3.1).
    pub fn save_gang_shop(
        &self,
        shop: u16,
        state: &mud_core::gang::GangShopState,
    ) -> Result<(), StateError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO gang_shop (shop, last_stocker, markup)
             VALUES (?1, ?2, ?3)",
            params![shop, state.last_stocker, state.markup],
        )?;
        tx.execute("DELETE FROM gang_shop_slot WHERE shop = ?1", params![shop])?;
        for (i, slot) in state.slots.iter().enumerate() {
            let Some(item) = slot.item else { continue };
            tx.execute(
                "INSERT INTO gang_shop_slot (shop, slot, item, count, price, denom)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![shop, i as i64, item.0, slot.count, slot.price, slot.denom],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every persisted gang-shop state, for `Core::restore_gang_shops`.
    pub fn load_gang_shops(
        &self,
    ) -> Result<Vec<(u16, mud_core::gang::GangShopState)>, StateError> {
        use mud_core::gang::GangShopState;
        let mut out: std::collections::BTreeMap<u16, GangShopState> = self
            .conn
            .prepare("SELECT shop, last_stocker, markup FROM gang_shop")?
            .query_map([], |r| {
                Ok((
                    r.get::<_, u16>(0)?,
                    GangShopState {
                        last_stocker: r.get(1)?,
                        markup: r.get(2)?,
                        ..GangShopState::default()
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut stmt = self
            .conn
            .prepare("SELECT shop, slot, item, count, price, denom FROM gang_shop_slot")?;
        let rows = stmt.query_map([], |r| {
            let slot = r.get::<_, usize>(1)?;
            if slot >= 10 {
                return Err(rusqlite::Error::IntegralValueOutOfRange(1, slot as i64));
            }
            Ok((
                r.get::<_, u16>(0)?,
                slot,
                r.get::<_, u16>(2)?,
                r.get::<_, i16>(3)?,
                r.get::<_, u16>(4)?,
                r.get::<_, i16>(5)?,
            ))
        })?;
        for row in rows {
            let (shop, slot, item, count, price, denom) = row?;
            let state = out.entry(shop).or_default();
            state.slots[slot] = mud_core::gang::GangShopSlot {
                item: Some(mud_core::content::ItemId(item)),
                count,
                price,
                denom,
            };
        }
        Ok(out.into_iter().collect())
    }

    /// The offline bank-8 credit (deposit_gangleaders_account): upsert
    /// the stocker's bankbook row.
    pub fn deposit_gang_gold(&self, stocker: &str, copper: u64) -> Result<(), StateError> {
        self.conn.execute(
            "INSERT INTO bankbook (name, shop, balance) VALUES (?1, 8, ?2)
             ON CONFLICT (name, shop) DO UPDATE SET balance = balance + excluded.balance",
            params![stocker, copper as i64],
        )?;
        Ok(())
    }

    /// Upserts one shop's 20 shelf counts (the shop dirty-byte save).
    pub fn save_shop_stock(&self, shop: u16, counts: &[i16; 20]) -> Result<(), StateError> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO shop_stock (shop, slot, now) VALUES (?1, ?2, ?3)
             ON CONFLICT (shop, slot) DO UPDATE SET now = excluded.now",
        )?;
        for (slot, now) in counts.iter().enumerate() {
            stmt.execute((shop, slot as i64, i64::from(*now)))?;
        }
        Ok(())
    }

    /// Upserts a limited template's kill stamp (wall-clock seconds).
    pub fn save_monster_kill(&self, template: u16, killed_at: i64) -> Result<(), StateError> {
        self.conn.prepare_cached(
            "INSERT INTO monster_population (template, killed_at) VALUES (?1, ?2)
             ON CONFLICT (template) DO UPDATE SET killed_at = excluded.killed_at",
        )?.execute((template, killed_at))?;
        Ok(())
    }

    /// Every persisted kill stamp as (template, killed_at wall-seconds).
    pub fn load_monster_kills(&self) -> Result<Vec<(u16, i64)>, StateError> {
        let mut stmt = self.conn.prepare("SELECT template, killed_at FROM monster_population")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, u16>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Upserts a room's respawn stamp (wall-clock seconds).
    pub fn save_room_stamp(&self, map: u16, room: u16, stamped_at: i64) -> Result<(), StateError> {
        self.conn.prepare_cached(
            "INSERT INTO room_stamp (map, room, stamped_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (map, room) DO UPDATE SET stamped_at = excluded.stamped_at",
        )?.execute((map, room, stamped_at))?;
        Ok(())
    }

    /// Every persisted room stamp as (map, room, stamped_at wall-seconds).
    pub fn load_room_stamps(&self) -> Result<Vec<(u16, u16, i64)>, StateError> {
        let mut stmt = self.conn.prepare("SELECT map, room, stamped_at FROM room_stamp")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, u16>(0)?, row.get::<_, u16>(1)?, row.get::<_, i64>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// All persisted shelf counts, for `Core::restore_shop_stock` at boot.
    pub fn load_shop_stock(&self) -> Result<Vec<(u16, usize, i16)>, StateError> {
        let mut stmt = self
            .conn
            .prepare("SELECT shop, slot, now FROM shop_stock")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, u16>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i16>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Permadeath: remove the character record.
    pub fn delete_player(&self, name: &str) -> Result<(), StateError> {
        let tx = self.conn.unchecked_transaction()?;
        for table in [
            "player",
            "player_item",
            "bankbook",
            "player_spell",
            "player_effect",
            "player_ability",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE name = ?1"),
                params![name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_player(&self, name: &str) -> Result<Option<Player>, StateError> {
        let player = self.load_player_row(name)?;
        let Some(mut player) = player else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT kind, item, uses FROM player_item WHERE name = ?1 ORDER BY slot",
        )?;
        let rows = stmt.query_map(params![name], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ItemId(r.get::<_, u16>(1)?),
                r.get::<_, i16>(2)?,
            ))
        })?;
        for row in rows {
            let (kind, item, uses) = row?;
            match kind.as_str() {
                "worn" => player.worn.push((item, uses)),
                "weapon" => player.weapon = Some((item, uses)),
                _ => player.inventory.push((item, uses)),
            }
        }
        let mut stmt = self
            .conn
            .prepare("SELECT shop, balance FROM bankbook WHERE name = ?1 ORDER BY shop")?;
        let rows = stmt.query_map(params![name], |r| {
            Ok((r.get::<_, u16>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        for row in rows {
            player.bankbooks.push(row?);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT spell, temporary FROM player_spell WHERE name = ?1")?;
        let rows = stmt.query_map(params![name], |r| {
            Ok((SpellId(r.get::<_, u16>(0)?), r.get::<_, bool>(1)?))
        })?;
        for row in rows {
            let (spell, temporary) = row?;
            player.spellbook.insert(spell, temporary);
        }
        let mut stmt = self.conn.prepare(
            "SELECT slot, spell, value, remaining FROM player_effect WHERE name = ?1",
        )?;
        let rows = stmt.query_map(params![name], |r| {
            // Checked like the other columns: a corrupt slot index errors
            // through StateError::Db instead of panicking on the array.
            let slot = r.get::<_, usize>(0)?;
            if slot >= 10 {
                return Err(rusqlite::Error::IntegralValueOutOfRange(0, slot as i64));
            }
            Ok((
                slot,
                SpellId(r.get::<_, u16>(1)?),
                r.get::<_, i16>(2)?,
                r.get::<_, i32>(3)?,
            ))
        })?;
        for row in rows {
            let (slot, spell, value, remaining) = row?;
            player.active_spells[slot] = ActiveSpell {
                spell: Some(spell),
                value,
                remaining,
            };
        }
        let mut stmt = self.conn.prepare(
            "SELECT slot, ability, value FROM player_ability WHERE name = ?1",
        )?;
        let rows = stmt.query_map(params![name], |r| {
            let slot = r.get::<_, usize>(0)?;
            if slot >= 30 {
                return Err(rusqlite::Error::IntegralValueOutOfRange(0, slot as i64));
            }
            Ok((slot, r.get::<_, u16>(1)?, r.get::<_, i16>(2)?))
        })?;
        for row in rows {
            let (slot, ability, value) = row?;
            // Unknown ability ids (content drift under a save) are
            // skipped, the same tolerance the spell loaders show.
            if let Some(ability) = mud_core::ability::Ability::from_id(ability) {
                player.innate[slot] = (Some(ability), value);
            }
        }
        Ok(Some(player))
    }

    fn load_player_row(&self, name: &str) -> Result<Option<Player>, StateError> {
        self.conn
            .query_row(
                "SELECT name, gender, race, class, level,
                     intellect, wisdom, strength, health, agility, charm,
                     b_intellect, b_wisdom, b_strength, b_health, b_agility,
                     b_charm, hp_base, current_hp, current_mana, hunger, thirst,
                     runic, platinum, gold, silver, copper, lawful,
                     cp_unspent, cp_lifetime, lives, experience, map, room,
                     poison, ansi, fame, warn_on_evil, quest_flags, gang,
                     gang_flags
                 FROM player WHERE name = ?1",
                params![name],
                |r| {
                    Ok(Player {
                        name: r.get(0)?,
                        gender: gender_from(&r.get::<_, String>(1)?),
                        race: RaceId(r.get(2)?),
                        class: ClassId(r.get(3)?),
                        level: r.get(4)?,
                        stats: StatBlock {
                            intellect: r.get(5)?,
                            wisdom: r.get(6)?,
                            strength: r.get(7)?,
                            health: r.get(8)?,
                            agility: r.get(9)?,
                            charm: r.get(10)?,
                        },
                        base_stats: StatBlock {
                            intellect: r.get(11)?,
                            wisdom: r.get(12)?,
                            strength: r.get(13)?,
                            health: r.get(14)?,
                            agility: r.get(15)?,
                            charm: r.get(16)?,
                        },
                        hp_base: r.get(17)?,
                        current_hp: r.get(18)?,
                        current_mana: r.get(19)?,
                        hunger: r.get(20)?,
                        thirst: r.get(21)?,
                        coins: Coins {
                            runic: r.get(22)?,
                            platinum: r.get(23)?,
                            gold: r.get(24)?,
                            silver: r.get(25)?,
                            copper: r.get(26)?,
                        },
                        lawful: r.get(27)?,
                        inventory: Vec::new(),
                        weapon: None,
                        bankbooks: Vec::new(),
                        worn: Vec::new(),
                        cp_unspent: r.get(28)?,
                        cp_lifetime: r.get(29)?,
                        lives: r.get(30)?,
                        experience: r.get(31)?,
                        location: RoomId {
                            map: r.get(32)?,
                            room: r.get(33)?,
                        },
                        spellbook: BTreeMap::new(),
                        poison: r.get(34)?,
                        active_spells: Default::default(),
                        ansi: r.get(35)?,
                        fame: r.get(36)?,
                        warn_on_evil: r.get(37)?,
                        quest_flags: r.get::<_, i64>(38)? as u64,
                        gang: r.get(39)?,
                        gang_flags: r.get(40)?,
                        // Runtime stealth flags — never persisted.
                        hidden: false,
                        sneak_armed: false,
                        // Filled by the player_ability side table (slice 6).
                        innate: [(None, 0); 30],
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}
