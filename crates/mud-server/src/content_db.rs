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
    AbilityValue, AttackForm, Class, ClassId, Content, Element, Exit, Item, ItemId, LootSlot,
    MatchType, Message, MessageId, Monster, MonsterId, PlacedItem, Race, RaceId, Room, RoomId,
    SaveClass, ScalePair, Shop, ShopId, ShopStock, Spell, SpellId, StatBlock, TargetMode,
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

fn to_u8(table: &'static str, field: &str, v: i64) -> Result<u8, LoadError> {
    u8::try_from(v).map_err(|_| invalid(table, format!("{field} = {v} does not fit u8")))
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
        .map(|i| format!("roomexit_{i}, roomtype_{i}, para1_{i}, para2_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let descs = (1..=7)
        .map(|i| format!("desc_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let placed = (1..=17)
        .map(|i| format!("roomitems_{i}, roomitemqty_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT mapnumber, roomnumber, name, shopnum, {descs}, {exits}, {placed}, type, attributes, \
         monstertype, maxregen, minindex, maxindex, delay, permnpc, bynumber, controlroom, maxarea FROM room"
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
        let mut placed_items = Vec::new();
        for i in 0..17 {
            let base = 51 + i * 2;
            let item: i64 = row.get(base)?;
            if item > 0 {
                let quantity = to_i16("room", "roomitemqty", row.get(base + 1)?)?;
                placed_items.push(PlacedItem {
                    item: ItemId(to_u16("room", "roomitems", item)?),
                    quantity: quantity.max(1),
                });
            }
        }
        let mut room = Room {
            id,
            name: row.get(2)?,
            description,
            room_type: to_i16("room", "type", row.get(51 + 17 * 2)?)?,
            attributes: to_i16("room", "attributes", row.get(51 + 17 * 2 + 1)?)?,
            shop: (shopnum > 0)
                .then(|| to_u16("room", "shopnum", shopnum).map(ShopId))
                .transpose()?,
            placed_items,
            exits: Default::default(),
            spawn_zone: to_i16("room", "monstertype", row.get(87)?)?,
            spawn_cap: to_i16("room", "maxregen", row.get(88)?)?,
            min_level: to_i16("room", "minindex", row.get(89)?)?,
            max_level: to_i16("room", "maxindex", row.get(90)?)?,
            respawn_delay: to_i16("room", "delay", row.get(91)?)?,
            // Forced spawn is the u4 at room+0x468 (generate_monster
            // 20168/20233); Nightmare's `bynumber` Long@0x466 straddles it
            // by two bytes, so the id is the column's high word.
            forced_monster: {
                let bynumber: i64 = row.get(93)?;
                let forced = (bynumber >> 16) & 0xffff;
                (forced > 0)
                    .then(|| to_u16("room", "bynumber", forced).map(MonsterId))
                    .transpose()?
            },
            boss_monster: {
                let permnpc: i64 = row.get(92)?;
                (permnpc > 0)
                    .then(|| to_u16("room", "permnpc", permnpc).map(MonsterId))
                    .transpose()?
            },
            // controlroom names a room on the SAME map (monsters.md §2).
            linked_room: {
                let control: i64 = row.get(94)?;
                (control > 0)
                    .then(|| {
                        to_u16("room", "controlroom", control)
                            .map(|r| RoomId { map, room: r })
                    })
                    .transpose()?
            },
            linked_cap: to_i16("room", "maxarea", row.get(95)?)?,
        };
        for d in 0..10 {
            let dest: i64 = row.get(11 + d * 4)?;
            if dest <= 0 {
                continue;
            }
            let exit_type = to_u16("room", "roomtype", row.get(12 + d * 4)?)?;
            // Exit type 8 is a map-change portal: destination map in para1.
            // Type 10 is a text-triggered exit: para1 is its phrase message.
            let dest_map = if exit_type == 8 {
                to_u16("room", "para1", row.get(13 + d * 4)?)?
            } else {
                map
            };
            let trigger_msg = if exit_type == 10 {
                opt_message("room", "para1", row.get(13 + d * 4)?)?
            } else {
                None
            };
            let para1: i64 = row.get(13 + d * 4)?;
            let para2: i64 = row.get(14 + d * 4)?;
            room.exits[d] = Some(Exit {
                dest: RoomId {
                    map: dest_map,
                    room: to_u16("room", "roomexit", dest)?,
                },
                exit_type,
                trigger_msg,
                // Raw para1: damage for types 9/0x18, the secret gate for
                // 7/0xb (monsters.md §3); clamped, ids never exceed i32.
                param: i32::try_from(para1).unwrap_or(0),
                // para2 != 0 = door closed (all shipped doors).
                door_closed: para2 != 0,
            });
        }
        content.add_room(room);
    }
    Ok(())
}

fn load_monsters(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let attack_cols = (1..=5)
        .map(|i| {
            format!(
                "attacktype_{i}, attackaccuspell_{i}, attackper_{i}, \
                 attackminhcastper_{i}, attackmaxhcastlvl_{i}, attackhitmsg_{i}, \
                 attackdodgemsg_{i}, attackmissmsg_{i}, attackenergy_{i}"
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let loot_cols = (1..=10)
        .map(|i| format!("itemnumber_{i}, itemuses_{i}, itemdropper_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, movemsg, deathmsg, {}, {}, \
         hitpoints, experience, expmulti, ac, dr, mr, bsdefence, energy, \
         runic, platinum, gold, silver, copper, {attack_cols}, \
         weaponnumber, {loot_cols}, \"index\", \"group\", follow, alignment, \
         type, something3, nothing2, gamelimit, hpregen, regentime FROM monster",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut attacks = [AttackForm::default(); 5];
        for (i, form) in attacks.iter_mut().enumerate() {
            let base = 37 + i * 9;
            *form = AttackForm {
                kind: to_i16("monster", "attacktype", row.get(base)?)?,
                accuracy: to_i16("monster", "attackaccuspell", row.get(base + 1)?)?,
                weight: to_i16("monster", "attackper", row.get(base + 2)?)?,
                min_damage: to_i16("monster", "attackminhcastper", row.get(base + 3)?)?,
                max_damage: to_i16("monster", "attackmaxhcastlvl", row.get(base + 4)?)?,
                hit_msg: opt_message("monster", "attackhitmsg", row.get(base + 5)?)?,
                dodge_msg: opt_message("monster", "attackdodgemsg", row.get(base + 6)?)?,
                miss_msg: opt_message("monster", "attackmissmsg", row.get(base + 7)?)?,
                energy: to_i16("monster", "attackenergy", row.get(base + 8)?)?,
            };
        }
        let weapon_col = 37 + 5 * 9;
        let weapon = match to_u16("monster", "weaponnumber", row.get(weapon_col)?)? {
            0 => None,
            id => Some(ItemId(id)),
        };
        let mut loot = Vec::new();
        let loot_end = weapon_col + 1 + 10 * 3;
        for i in 0..10 {
            let base = weapon_col + 1 + i * 3;
            let item = to_u16("monster", "itemnumber", row.get(base)?)?;
            if item == 0 {
                continue;
            }
            loot.push(LootSlot {
                item: ItemId(item),
                uses: to_i16("monster", "itemuses", row.get(base + 1)?)?,
                dropper: to_i16("monster", "itemdropper", row.get(base + 2)?)?,
            });
        }
        content.add_monster(Monster {
            id: MonsterId(to_u16("monster", "number", row.get(0)?)?),
            name: row.get(1)?,
            move_msg: opt_message("monster", "movemsg", row.get(2)?)?,
            death_msg: opt_message("monster", "deathmsg", row.get(3)?)?,
            abilities: ability_pairs("monster", row, 4, 14)?,
            hitpoints: i32::try_from(row.get::<_, i64>(24)?)
                .map_err(|_| invalid("monster", "hitpoints overflow"))?,
            experience: i32::try_from(row.get::<_, i64>(25)?)
                .map_err(|_| invalid("monster", "experience overflow"))?,
            exp_multi: i32::try_from(row.get::<_, i64>(26)?)
                .map_err(|_| invalid("monster", "expmulti overflow"))?,
            armour_class: to_i16("monster", "ac", row.get(27)?)?,
            damage_resist: to_i16("monster", "dr", row.get(28)?)?,
            magic_resist: to_i16("monster", "mr", row.get(29)?)?,
            bs_defence: to_i16("monster", "bsdefence", row.get(30)?)?,
            energy: i32::try_from(row.get::<_, i64>(31)?)
                .map_err(|_| invalid("monster", "energy overflow"))?,
            coins: [
                coin("monster", row, 32)?,
                coin("monster", row, 33)?,
                coin("monster", row, 34)?,
                coin("monster", row, 35)?,
                coin("monster", row, 36)?,
            ],
            weapon,
            loot,
            attacks,
            level: to_i16("monster", "index", row.get(loot_end)?)?,
            roam_class: to_i16("monster", "group", row.get(loot_end + 1)?)?,
            aggression: to_i16("monster", "follow", row.get(loot_end + 2)?)?,
            behaviour: to_i16("monster", "alignment", row.get(loot_end + 3)?)?,
            herd_mode: to_i16("monster", "type", row.get(loot_end + 4)?)?,
            herd_id: to_i16("monster", "something3", row.get(loot_end + 5)?)?,
            follower_cap: to_i16("monster", "nothing2", row.get(loot_end + 6)?)?,
            game_limit: to_i16("monster", "gamelimit", row.get(loot_end + 7)?)?,
            hp_regen: to_i16("monster", "hpregen", row.get(loot_end + 8)?)?,
            unique_cooldown: to_i16("monster", "regentime", row.get(loot_end + 9)?)?,
        });
    }
    Ok(())
}

fn coin(table: &'static str, row: &rusqlite::Row<'_>, idx: usize) -> Result<u32, LoadError> {
    let v: i64 = row.get(idx)?;
    u32::try_from(v.max(0)).map_err(|_| invalid(table, format!("coin at {idx} overflow")))
}

fn load_items(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    // Items carry 20 ability slots (not 10 like the other tables).
    let a_cols = (1..=20)
        .map(|i| format!("abilitya_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let b_cols = (1..=20)
        .map(|i| format!("abilityb_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, {a_cols}, {b_cols}, \
         weight, type, uses, cost, costtype, minhit, maxhit, ac, weapon, \
         armour, wornon, accuracy, dr, gettable, reqstr, speed, hitmsg, \
         missmsg, notdroppable, retainafteruses, destroyondeath, \
         class_1, class_2, class_3, class_4, class_5, class_6, class_7, \
         class_8, class_9, class_10, race_1, race_2, race_3, race_4, \
         race_5, race_6, race_7, race_8, race_9, race_10, \
         desc1, desc2, desc3, desc4, desc5, desc6, desc7, desc8, desc9 \
         FROM item"
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut abilities = Vec::new();
        for i in 0..20 {
            let id: i64 = row.get(2 + i)?;
            let value: i64 = row.get(22 + i)?;
            if id == 0 {
                continue;
            }
            let id = to_u16("item", "ability id", id)?;
            let ability = Ability::from_id(id)
                .ok_or_else(|| invalid("item", format!("unknown ability id {id}")))?;
            abilities.push((ability, to_i16("item", "ability value", value)?));
        }
        let base = 42;
        let mut classes = Vec::new();
        let mut races = Vec::new();
        for i in 0..10 {
            let c: i64 = row.get(base + 21 + i)?;
            if c > 0 {
                classes.push(ClassId(to_u16("item", "class", c)?));
            }
            let r: i64 = row.get(base + 31 + i)?;
            if r > 0 {
                races.push(RaceId(to_u16("item", "race", r)?));
            }
        }
        // desc1..desc9 trail the query (indexes base+41..base+49).
        let mut description: Vec<String> = (base + 41..base + 50)
            .map(|i| row.get(i))
            .collect::<Result<_, _>>()?;
        while description.last().is_some_and(|l| l.is_empty()) {
            description.pop();
        }
        content.add_item(Item {
            id: ItemId(to_u16("item", "number", row.get(0)?)?),
            name: row.get(1)?,
            description,
            abilities,
            classes,
            races,
            weight: to_i16("item", "weight", row.get(base)?)?,
            item_type: to_i16("item", "type", row.get(base + 1)?)?,
            uses: to_i16("item", "uses", row.get(base + 2)?)?,
            cost: i32::try_from(row.get::<_, i64>(base + 3)?)
                .map_err(|_| invalid("item", "cost overflow"))?,
            cost_denomination: to_i16("item", "costtype", row.get(base + 4)?)?,
            min_damage: to_i16("item", "minhit", row.get(base + 5)?)?,
            max_damage: to_i16("item", "maxhit", row.get(base + 6)?)?,
            ac: to_i16("item", "ac", row.get(base + 7)?)?,
            weapon_type: to_i16("item", "weapon", row.get(base + 8)?)?,
            armour_req: to_i16("item", "armour", row.get(base + 9)?)?,
            worn_on: to_i16("item", "wornon", row.get(base + 10)?)?,
            accuracy: to_i16("item", "accuracy", row.get(base + 11)?)?,
            defense: to_i16("item", "dr", row.get(base + 12)?)?,
            gettable: to_i16("item", "gettable", row.get(base + 13)?)?,
            req_str: to_i16("item", "reqstr", row.get(base + 14)?)?,
            speed: to_i16("item", "speed", row.get(base + 15)?)?,
            hit_msg: opt_message("item", "hitmsg", row.get(base + 16)?)?,
            miss_msg: opt_message("item", "missmsg", row.get(base + 17)?)?,
            not_droppable: to_i16("item", "notdroppable", row.get(base + 18)?)?,
            retain_after_uses: to_i16("item", "retainafteruses", row.get(base + 19)?)?,
            destroy_on_death: to_i16("item", "destroyondeath", row.get(base + 20)?)?,
        });
    }
    Ok(())
}

fn load_spells(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, shortname, castmsga, castmsgb, {}, {}, \
         levelcap, energy, level, min, max, spelltype, typeofresists, \
         difficulty, undefined01, target, duration, typeofattack, magerya, \
         mana, maxincrease, lvlsmaxincr, mageryb, minincrease, lvlsminincr, \
         durincrease, lvlsdurincr, msgstyle FROM spell",
        ability_cols("abilitya"),
        ability_cols("abilityb"),
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let field = |name: &str, idx: usize| -> Result<i16, LoadError> {
            to_i16("spell", name, row.get(idx)?)
        };
        let pair = |na: &str, ia: usize, nb: &str, ib: usize| -> Result<ScalePair, LoadError> {
            Ok(ScalePair {
                per: to_u8("spell", na, row.get(ia)?)?,
                levels: to_u8("spell", nb, row.get(ib)?)?,
            })
        };
        let num = to_u16("spell", "number", row.get(0)?)?;
        let target_mode = field("spelltype", 30)?;
        let save_class = field("typeofresists", 31)?;
        let match_type = field("target", 34)?;
        let element = field("typeofattack", 36)?;
        content.add_spell(Spell {
            id: SpellId(num),
            name: row.get(1)?,
            short_name: row.get(2)?,
            cast_msg_a: opt_message("spell", "castmsga", row.get(3)?)?,
            cast_msg_b: opt_message("spell", "castmsgb", row.get(4)?)?,
            abilities: ability_pairs("spell", row, 5, 15)?,
            level_cap: field("levelcap", 25)?,
            round_cost: field("energy", 26)?,
            required_power: field("level", 27)?,
            min_base: field("min", 28)?,
            max_base: field("max", 29)?,
            target_mode: TargetMode::from_i16(target_mode).ok_or_else(|| {
                invalid("spell", format!("spell {num}: spelltype = {target_mode}"))
            })?,
            save_class: SaveClass::from_i16(save_class).ok_or_else(|| {
                invalid("spell", format!("spell {num}: typeofresists = {save_class}"))
            })?,
            base_chance: field("difficulty", 32)?,
            duration_per_level: field("undefined01", 33)?,
            match_type: MatchType::from_i16(match_type)
                .ok_or_else(|| invalid("spell", format!("spell {num}: target = {match_type}")))?,
            duration: field("duration", 35)?,
            element: Element::from_i16(element)
                .ok_or_else(|| invalid("spell", format!("spell {num}: typeofattack = {element}")))?,
            class_gate_group: field("magerya", 37)?,
            mana_cost: field("mana", 38)?,
            max_increase: pair("maxincrease", 39, "lvlsmaxincr", 40)?,
            required_class_level: field("mageryb", 41)?,
            min_increase: pair("minincrease", 42, "lvlsminincr", 43)?,
            duration_increase: pair("durincrease", 44, "lvlsdurincr", 45)?,
            msg_style: field("msgstyle", 46)?,
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
    let stock_cols = (1..=20)
        .map(|i| {
            format!(
                "shopitemnumber_{i}, shopmax_{i}, shopnow_{i}, shoprgntime_{i}, \
                 shoprgnnumber_{i}, shoprgnpercentage_{i}"
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, shoptype, shopminlvl, shopmaxlvl, shopmarkup, \
         shopclasslimit, {stock_cols} FROM shop"
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut stock = [ShopStock::default(); 20];
        for (i, slot) in stock.iter_mut().enumerate() {
            let base = 7 + i * 6;
            let item: i64 = row.get(base)?;
            *slot = ShopStock {
                item: (item > 0)
                    .then(|| to_u16("shop", "shopitemnumber", item).map(ItemId))
                    .transpose()?,
                max: to_i16("shop", "shopmax", row.get(base + 1)?)?,
                now: to_i16("shop", "shopnow", row.get(base + 2)?)?,
                restock_time: to_i16("shop", "shoprgntime", row.get(base + 3)?)?,
                restock_amount: to_i16("shop", "shoprgnnumber", row.get(base + 4)?)?,
                restock_percent: to_i16("shop", "shoprgnpercentage", row.get(base + 5)?)?,
            };
        }
        content.add_shop(Shop {
            id: ShopId(to_u16("shop", "number", row.get(0)?)?),
            name: row.get(1)?,
            shop_type: to_i16("shop", "shoptype", row.get(2)?)?,
            min_level: to_i16("shop", "shopminlvl", row.get(3)?)?,
            max_level: to_i16("shop", "shopmaxlvl", row.get(4)?)?,
            markup: to_i16("shop", "shopmarkup", row.get(5)?)?,
            class_limit: to_i16("shop", "shopclasslimit", row.get(6)?)?,
            stock,
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
        "SELECT number, name, {}, {}, minhp, maxhp, magictype, magiclvl, exp, combat, \
         weapon, armour FROM class",
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
            combat_factor: to_i16("class", "combat", row.get(27)?)?,
            weapon_code: to_i16("class", "weapon", row.get(28)?)?,
            armour_code: to_i16("class", "armour", row.get(29)?)?,
        });
    }
    Ok(())
}
