//! The runtime state database (`state.sqlite`): accounts and saved players.
//!
//! This is the standalone server's replacement for the Worldgroup account
//! system and the WCCUSER2 player file. Single writer (the server); passwords
//! are argon2id hashes. The player schema mirrors `mud_core::game::Player`
//! field-for-field and grows with it.

use std::fmt;
use std::path::Path;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use mud_core::content::{ClassId, RaceId, RoomId, StatBlock};
use mud_core::game::{AccountProfile, Gender, Player};
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

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS account (
    name          TEXT PRIMARY KEY COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    gender        TEXT NOT NULL CHECK (gender IN ('M', 'F'))
) STRICT;
CREATE TABLE IF NOT EXISTS player (
    name         TEXT PRIMARY KEY COLLATE NOCASE,
    gender       TEXT NOT NULL CHECK (gender IN ('M', 'F')),
    race         INTEGER NOT NULL,
    class        INTEGER NOT NULL,
    level        INTEGER NOT NULL,
    intellect    INTEGER NOT NULL,
    wisdom       INTEGER NOT NULL,
    strength     INTEGER NOT NULL,
    health       INTEGER NOT NULL,
    agility      INTEGER NOT NULL,
    charm        INTEGER NOT NULL,
    b_intellect  INTEGER NOT NULL,
    b_wisdom     INTEGER NOT NULL,
    b_strength   INTEGER NOT NULL,
    b_health     INTEGER NOT NULL,
    b_agility    INTEGER NOT NULL,
    b_charm      INTEGER NOT NULL,
    hp_base      INTEGER NOT NULL,
    current_hp   INTEGER NOT NULL,
    current_mana INTEGER NOT NULL,
    hunger       INTEGER NOT NULL,
    thirst       INTEGER NOT NULL,
    cp_unspent   INTEGER NOT NULL,
    cp_lifetime  INTEGER NOT NULL,
    lives        INTEGER NOT NULL,
    experience   INTEGER NOT NULL,
    map          INTEGER NOT NULL,
    room         INTEGER NOT NULL
) STRICT;
";

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
        conn.execute_batch(SCHEMA)?;
        Ok(StateDb { conn })
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

    pub fn save_player(&self, player: &Player) -> Result<(), StateError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO player (name, gender, race, class, level,
                 intellect, wisdom, strength, health, agility, charm,
                 b_intellect, b_wisdom, b_strength, b_health, b_agility,
                 b_charm, hp_base, current_hp, current_mana, hunger, thirst,
                 cp_unspent, cp_lifetime, lives, experience, map, room)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                 ?26, ?27, ?28)",
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
                player.cp_unspent,
                player.cp_lifetime,
                player.lives,
                player.experience,
                player.location.map,
                player.location.room,
            ],
        )?;
        Ok(())
    }

    pub fn load_player(&self, name: &str) -> Result<Option<Player>, StateError> {
        self.conn
            .query_row(
                "SELECT name, gender, race, class, level,
                     intellect, wisdom, strength, health, agility, charm,
                     b_intellect, b_wisdom, b_strength, b_health, b_agility,
                     b_charm, hp_base, current_hp, current_mana, hunger, thirst,
                     cp_unspent, cp_lifetime, lives, experience, map, room
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
                        cp_unspent: r.get(22)?,
                        cp_lifetime: r.get(23)?,
                        lives: r.get(24)?,
                        experience: r.get(25)?,
                        location: RoomId {
                            map: r.get(26)?,
                            room: r.get(27)?,
                        },
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}
