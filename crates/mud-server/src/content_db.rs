//! Loads the static content database (`mmud_wgnt.sqlite`, extracted from the
//! WG3-NT `.VIR` files) into the typed `mud_core::content` model.
//!
//! The load is strict: any value that doesn't fit its model type (an unknown
//! ability id, an id outside `u16`) is a `LoadError`, not a silent skip.
//! Cross-reference checking is `Content::validate`'s job, not ours.

use std::fmt;
use std::path::Path;

use mud_core::ability::Ability;
use mud_core::content::{
    AbilityValue, Class, ClassId, Content, Exit, Item, ItemId, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, Shop, ShopId, Spell, SpellId, StatBlock,
};
use rusqlite::Connection;

#[derive(Debug)]
pub enum LoadError {
    Db(rusqlite::Error),
    Invalid {
        table: &'static str,
        detail: String,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Db(e) => write!(f, "database error: {e}"),
            LoadError::Invalid { table, detail } => write!(f, "invalid {table} record: {detail}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<rusqlite::Error> for LoadError {
    fn from(e: rusqlite::Error) -> Self {
        LoadError::Db(e)
    }
}

pub fn load(path: &Path) -> Result<Content, LoadError> {
    let db = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut content = Content::default();
    load_rooms(&db, &mut content)?;
    load_monsters(&db, &mut content)?;
    load_items(&db, &mut content)?;
    load_spells(&db, &mut content)?;
    load_messages(&db, &mut content)?;
    load_shops(&db, &mut content)?;
    load_races(&db, &mut content)?;
    load_classes(&db, &mut content)?;
    Ok(content)
}

fn invalid(table: &'static str, detail: impl Into<String>) -> LoadError {
    LoadError::Invalid {
        table,
        detail: detail.into(),
    }
}

fn to_u16(table: &'static str, field: &str, v: i64) -> Result<u16, LoadError> {
    u16::try_from(v).map_err(|_| invalid(table, format!("{field} = {v} does not fit u16")))
}

fn to_i16(table: &'static str, field: &str, v: i64) -> Result<i16, LoadError> {
    i16::try_from(v).map_err(|_| invalid(table, format!("{field} = {v} does not fit i16")))
}

/// `0` (and negatives) mean "no message" in the data.
fn opt_message(table: &'static str, field: &str, v: i64) -> Result<Option<MessageId>, LoadError> {
    if v <= 0 {
        Ok(None)
    } else {
        Ok(Some(MessageId(to_u16(table, field, v)?)))
    }
}

/// Reads the ten `(abilitya_N, abilityb_N)` columns starting at `first_col`
/// (pairing by index). Slots with ability id 0 (`empty`) are filler.
fn ability_pairs(
    table: &'static str,
    row: &rusqlite::Row<'_>,
    first_a: usize,
    first_b: usize,
) -> Result<Vec<AbilityValue>, LoadError> {
    let mut pairs = Vec::new();
    for i in 0..10 {
        let id: i64 = row.get(first_a + i)?;
        let value: i64 = row.get(first_b + i)?;
        if id == 0 {
            continue;
        }
        let id = to_u16(table, "ability id", id)?;
        let ability = Ability::from_id(id)
            .ok_or_else(|| invalid(table, format!("unknown ability id {id}")))?;
        pairs.push((ability, to_i16(table, "ability value", value)?));
    }
    Ok(pairs)
}

fn ability_cols(prefix: &str) -> String {
    (1..=10)
        .map(|i| format!("{prefix}_{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn load_rooms(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let exits = (1..=10)
        .map(|i| format!("roomexit_{i}, roomtype_{i}, para1_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let descs = (1..=7)
        .map(|i| format!("desc_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT mapnumber, roomnumber, name, shopnum, {descs}, {exits} FROM room"
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let map = to_u16("room", "mapnumber", row.get(0)?)?;
        let id = RoomId {
            map,
            room: to_u16("room", "roomnumber", row.get(1)?)?,
        };
        let shopnum: i64 = row.get(3)?;
        let mut description: Vec<String> = (4..11)
            .map(|i| row.get(i))
            .collect::<Result<_, _>>()?;
        while description.last().is_some_and(|l| l.is_empty()) {
            description.pop();
        }
        let mut room = Room {
            id,
            name: row.get(2)?,
            description,
            shop: (shopnum > 0)
                .then(|| to_u16("room", "shopnum", shopnum).map(ShopId))
                .transpose()?,
            exits: Default::default(),
        };
        for d in 0..10 {
            let dest: i64 = row.get(11 + d * 3)?;
            if dest <= 0 {
                continue;
            }
            let exit_type = to_u16("room", "roomtype", row.get(12 + d * 3)?)?;
            // Exit type 8 is a map-change portal: destination map in para1.
            let dest_map = if exit_type == 8 {
                to_u16("room", "para1", row.get(13 + d * 3)?)?
            } else {
                map
            };
            room.exits[d] = Some(Exit {
                dest: RoomId {
                    map: dest_map,
                    room: to_u16("room", "roomexit", dest)?,
                },
                exit_type,
            });
        }
        content.add_room(room);
    }
    Ok(())
}

fn load_monsters(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, movemsg, deathmsg, {}, {} FROM monster",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_monster(Monster {
            id: MonsterId(to_u16("monster", "number", row.get(0)?)?),
            name: row.get(1)?,
            move_msg: opt_message("monster", "movemsg", row.get(2)?)?,
            death_msg: opt_message("monster", "deathmsg", row.get(3)?)?,
            abilities: ability_pairs("monster", row, 4, 14)?,
        });
    }
    Ok(())
}

fn load_items(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, {}, {} FROM item",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_item(Item {
            id: ItemId(to_u16("item", "number", row.get(0)?)?),
            name: row.get(1)?,
            abilities: ability_pairs("item", row, 2, 12)?,
        });
    }
    Ok(())
}

fn load_spells(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, shortname, castmsga, castmsgb, {}, {} FROM spell",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_spell(Spell {
            id: SpellId(to_u16("spell", "number", row.get(0)?)?),
            name: row.get(1)?,
            short_name: row.get(2)?,
            cast_msg_a: opt_message("spell", "castmsga", row.get(3)?)?,
            cast_msg_b: opt_message("spell", "castmsgb", row.get(4)?)?,
            abilities: ability_pairs("spell", row, 5, 15)?,
        });
    }
    Ok(())
}

fn load_messages(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt =
        db.prepare("SELECT number, messageline1, messageline2, messageline3 FROM message")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut lines: Vec<String> = vec![row.get(1)?, row.get(2)?, row.get(3)?];
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        content.add_message(Message {
            id: MessageId(to_u16("message", "number", row.get(0)?)?),
            lines,
        });
    }
    Ok(())
}

fn load_shops(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(
        "SELECT number, name, shoptype, shopminlvl, shopmaxlvl, shopmarkup, \
         shopclasslimit FROM shop",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_shop(Shop {
            id: ShopId(to_u16("shop", "number", row.get(0)?)?),
            name: row.get(1)?,
            shop_type: to_i16("shop", "shoptype", row.get(2)?)?,
            min_level: to_i16("shop", "shopminlvl", row.get(3)?)?,
            max_level: to_i16("shop", "shopmaxlvl", row.get(4)?)?,
            markup: to_i16("shop", "shopmarkup", row.get(5)?)?,
            class_limit: to_i16("shop", "shopclasslimit", row.get(6)?)?,
        });
    }
    Ok(())
}

fn load_races(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, {}, {}, \
         minint, minwil, minstr, minhea, minagl, minchm, \
         maxint, maxwil, maxstr, maxhea, maxagl, maxchm, cp, hpbonus, \
         expchart FROM race",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_race(Race {
            id: RaceId(to_u16("race", "number", row.get(0)?)?),
            name: row.get(1)?,
            abilities: ability_pairs("race", row, 2, 12)?,
            base_stats: stat_block("race", row, 22)?,
            max_stats: stat_block("race", row, 28)?,
            cp: to_u16("race", "cp", row.get(34)?)?,
            hp_per_level: to_i16("race", "hpbonus", row.get(35)?)?,
            exp_chart: to_i16("race", "expchart", row.get(36)?)?,
        });
    }
    Ok(())
}

/// Reads six consecutive stat columns (Int, Wis, Str, Hea, Agl, Chm).
fn stat_block(
    table: &'static str,
    row: &rusqlite::Row<'_>,
    first: usize,
) -> Result<StatBlock, LoadError> {
    Ok(StatBlock {
        intellect: to_u16(table, "int", row.get(first)?)?,
        wisdom: to_u16(table, "wil", row.get(first + 1)?)?,
        strength: to_u16(table, "str", row.get(first + 2)?)?,
        health: to_u16(table, "hea", row.get(first + 3)?)?,
        agility: to_u16(table, "agl", row.get(first + 4)?)?,
        charm: to_u16(table, "chm", row.get(first + 5)?)?,
    })
}

fn load_classes(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, {}, {}, minhp, maxhp, magictype, magiclvl, exp FROM class",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        content.add_class(Class {
            id: ClassId(to_u16("class", "number", row.get(0)?)?),
            name: row.get(1)?,
            abilities: ability_pairs("class", row, 2, 12)?,
            hp_per_level: to_i16("class", "minhp", row.get(22)?)?,
            hp_seed: to_i16("class", "maxhp", row.get(23)?)?,
            caster_group: to_i16("class", "magictype", row.get(24)?)?,
            casting_factor: to_i16("class", "magiclvl", row.get(25)?)?,
            exp_base: to_i16("class", "exp", row.get(26)?)?,
        });
    }
    Ok(())
}
