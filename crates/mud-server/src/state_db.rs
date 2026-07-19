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
            "INSERT INTO account (name, password_hash, gender) VALUES (?1, ?2, ?3)",
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
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT name, password_hash, gender FROM account WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((canonical, hash, gender)) = row else {
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
        }))
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
        ["player_item", "bankbook", "player_spell", "player_effect"]
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
            "INSERT OR REPLACE INTO player (name, gender, race, class, level,
                 intellect, wisdom, strength, health, agility, charm,
                 b_intellect, b_wisdom, b_strength, b_health, b_agility,
                 b_charm, hp_base, current_hp, current_mana, hunger, thirst,
                 runic, platinum, gold, silver, copper, lawful,
                 cp_unspent, cp_lifetime, lives, experience, map, room,
                 poison)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                 ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35)",
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
            ],
        )?;
        tx.commit()?;
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
                     poison
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
                        // Fame persists with the M7 crime system; until a
                        // fame source exists every save-load sees 0.
                        fame: 0,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}
