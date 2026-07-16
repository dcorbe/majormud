//! Static game content: the typed, validated form of the nine content tables
//! extracted from the WG3-NT data files (`re/docs/vir_schemas.md`).
//!
//! Loading from SQLite lives in `mud-server`; this module only defines the
//! model and its cross-reference validation. Structs carry the fields current
//! milestones need — they grow as systems are implemented.

use std::collections::BTreeMap;

use crate::ability::Ability;

/// A room key: MajorMUD addresses rooms as (map, room), never room alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RoomId {
    pub map: u16,
    pub room: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonsterId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpellId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShopId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RaceId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(pub u16);

/// The ten exit directions, in the game's storage order
/// (`roomexit_1` = North … `roomexit_10` = Down).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(usize)]
pub enum Direction {
    North = 0,
    South = 1,
    East = 2,
    West = 3,
    NorthEast = 4,
    NorthWest = 5,
    SouthEast = 6,
    SouthWest = 7,
    Up = 8,
    Down = 9,
}

impl Direction {
    pub const ALL: [Direction; 10] = [
        Direction::North,
        Direction::South,
        Direction::East,
        Direction::West,
        Direction::NorthEast,
        Direction::NorthWest,
        Direction::SouthEast,
        Direction::SouthWest,
        Direction::Up,
        Direction::Down,
    ];

    pub fn opposite(self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::East => Direction::West,
            Direction::West => Direction::East,
            Direction::NorthEast => Direction::SouthWest,
            Direction::NorthWest => Direction::SouthEast,
            Direction::SouthEast => Direction::NorthWest,
            Direction::SouthWest => Direction::NorthEast,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }
}

/// An effect slot: every game object encodes effects as `(Ability, value)`
/// pairs against the shared ability table (`re/docs/abilities.md`). Slots
/// holding `Ability::Empty` are omitted at load time.
pub type AbilityValue = (Ability, i16);

/// The six primary stats in the game's storage order
/// (Int, Wis, Str, Hea, Agl, Chm — `re/docs/records.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatBlock {
    pub intellect: u16,
    pub wisdom: u16,
    pub strength: u16,
    pub health: u16,
    pub agility: u16,
    pub charm: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    pub dest: RoomId,
    /// Raw `roomtype_N` value; semantics per type are handled by later
    /// milestones (8 = map-change portal, already folded into `dest`).
    pub exit_type: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    /// Display lines, from the record's seven fixed `desc_N` line fields
    /// (trailing empty lines trimmed).
    pub description: Vec<String>,
    /// The shop operating in this room (`shopnum` column), if any.
    pub shop: Option<ShopId>,
    /// Indexed by `Direction as usize`.
    pub exits: [Option<Exit>; 10],
}

/// One of the five monster attack forms (`knmsr+0x128/0x138` tables;
/// `attack*_N` columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AttackForm {
    /// 0 = empty, 1 = melee, 2 = cast, 3 = rob (`attacktype`).
    pub kind: i16,
    /// Melee accuracy, or spell id for casts (`attackaccuspell`).
    pub accuracy: i16,
    /// Cumulative selection threshold 0-100 (`attackper`).
    pub weight: i16,
    /// Melee min damage / cast percent (`attackminhcastper`).
    pub min_damage: i16,
    /// Melee max damage / cast level (`attackmaxhcastlvl`).
    pub max_damage: i16,
    pub hit_msg: Option<MessageId>,
    pub miss_msg: Option<MessageId>,
    /// EU spent per swing (`attackenergy`).
    pub energy: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monster {
    pub id: MonsterId,
    pub name: String,
    pub move_msg: Option<MessageId>,
    pub death_msg: Option<MessageId>,
    pub abilities: Vec<AbilityValue>,
    /// `hitpoints` — spawn HP.
    pub hitpoints: i32,
    /// `experience` (worth) and `expmulti` (percent-ish multiplier).
    pub experience: i32,
    pub exp_multi: i32,
    pub armour_class: i16,
    pub damage_resist: i16,
    pub magic_resist: i16,
    /// Backstab defence.
    pub bs_defence: i16,
    /// Per-round energy pool/regen.
    pub energy: i32,
    pub coins: [u32; 5],
    /// The five attack-form slots (kind 0 = unused).
    pub attacks: [AttackForm; 5],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub abilities: Vec<AbilityValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spell {
    pub id: SpellId,
    pub name: String,
    pub short_name: String,
    pub cast_msg_a: Option<MessageId>,
    pub cast_msg_b: Option<MessageId>,
    pub abilities: Vec<AbilityValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shop {
    pub id: ShopId,
    pub name: String,
    /// `shop+0xcc` — 8 = trainer/guild (`shoptype` column).
    pub shop_type: i16,
    /// `shop+0xce`/`+0xd0` — trainable level band (`shopminlvl`/`shopmaxlvl`).
    pub min_level: i16,
    pub max_level: i16,
    /// `shop+0xd2` — markup percent (`shopmarkup`).
    pub markup: i16,
    /// `shop+0xd6` — 0 = any class, else required class id (`shopclasslimit`).
    pub class_limit: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Race {
    pub id: RaceId,
    pub name: String,
    pub abilities: Vec<AbilityValue>,
    /// Starting stat template, copied verbatim at creation — MajorMUD does
    /// not roll stats (`re/docs/character_creation.md` §2.2).
    pub base_stats: StatBlock,
    /// Racial caps the CP editor may raise stats to.
    pub max_stats: StatBlock,
    /// Character points granted at creation.
    pub cp: u16,
    /// `race+0x2c` — flat max-HP per level (`hpbonus` column).
    pub hp_per_level: i16,
    /// `race+0x62` — exp-curve base contribution (`expchart` column).
    pub exp_chart: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub id: ClassId,
    pub name: String,
    pub abilities: Vec<AbilityValue>,
    /// `class+0x20` — flat max-HP per level (`minhp` column).
    pub hp_per_level: i16,
    /// `class+0x22` — HP-base seed at creation and per-level roll bound
    /// (`maxhp` column).
    pub hp_seed: i16,
    /// `class+0x40` — 0 non-caster, 1-4 caster stat groups, 5 Kai
    /// (`magictype` column).
    pub caster_group: i16,
    /// `class+0x42` — casting level factor (`magiclvl` column).
    pub casting_factor: i16,
    /// `class+0x24` — exp-curve base contribution (`exp` column).
    pub exp_base: i16,
}

/// Dangling references present in the shipped 1.11p data itself. The original
/// engine tolerates them (the death message silently doesn't print), so they
/// are exempt from validation rather than "fixed".
pub const KNOWN_DANGLING_MONSTER_MESSAGES: [(MonsterId, MessageId); 2] = [
    (MonsterId(789), MessageId(3551)),  // young gypsy girl
    (MonsterId(1017), MessageId(3553)), // Mandrake the Songweaver
];

/// See [`KNOWN_DANGLING_MONSTER_MESSAGES`].
pub const KNOWN_DANGLING_SPELL_MESSAGES: [(SpellId, MessageId); 1] =
    [(SpellId(1055), MessageId(3499))]; // "BCNS"

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentError {
    UnresolvedExit {
        room: RoomId,
        direction: Direction,
        dest: RoomId,
    },
    DanglingMonsterMessage {
        monster: MonsterId,
        message: MessageId,
    },
    DanglingSpellMessage {
        spell: SpellId,
        message: MessageId,
    },
}

/// All static content, keyed for deterministic iteration.
#[derive(Debug, Default)]
pub struct Content {
    pub rooms: BTreeMap<RoomId, Room>,
    pub monsters: BTreeMap<MonsterId, Monster>,
    pub items: BTreeMap<ItemId, Item>,
    pub spells: BTreeMap<SpellId, Spell>,
    pub messages: BTreeMap<MessageId, Message>,
    pub shops: BTreeMap<ShopId, Shop>,
    pub races: BTreeMap<RaceId, Race>,
    pub classes: BTreeMap<ClassId, Class>,
}

impl Content {
    pub fn add_room(&mut self, room: Room) {
        self.rooms.insert(room.id, room);
    }

    pub fn add_monster(&mut self, monster: Monster) {
        self.monsters.insert(monster.id, monster);
    }

    pub fn add_item(&mut self, item: Item) {
        self.items.insert(item.id, item);
    }

    pub fn add_spell(&mut self, spell: Spell) {
        self.spells.insert(spell.id, spell);
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.insert(message.id, message);
    }

    pub fn add_shop(&mut self, shop: Shop) {
        self.shops.insert(shop.id, shop);
    }

    pub fn add_race(&mut self, race: Race) {
        self.races.insert(race.id, race);
    }

    pub fn add_class(&mut self, class: Class) {
        self.classes.insert(class.id, class);
    }

    /// Checks every cross-reference and returns all violations, excluding the
    /// documented `KNOWN_DANGLING_MONSTER_MESSAGES`. An empty result means the
    /// content is internally consistent.
    pub fn validate(&self) -> Vec<ContentError> {
        let mut errors = Vec::new();

        for room in self.rooms.values() {
            for direction in Direction::ALL {
                if let Some(exit) = &room.exits[direction as usize]
                    && !self.rooms.contains_key(&exit.dest)
                {
                    errors.push(ContentError::UnresolvedExit {
                        room: room.id,
                        direction,
                        dest: exit.dest,
                    });
                }
            }
        }

        for monster in self.monsters.values() {
            for msg in [monster.move_msg, monster.death_msg].into_iter().flatten() {
                let known = KNOWN_DANGLING_MONSTER_MESSAGES.contains(&(monster.id, msg));
                if !known && !self.messages.contains_key(&msg) {
                    errors.push(ContentError::DanglingMonsterMessage {
                        monster: monster.id,
                        message: msg,
                    });
                }
            }
        }

        for spell in self.spells.values() {
            for msg in [spell.cast_msg_a, spell.cast_msg_b].into_iter().flatten() {
                let known = KNOWN_DANGLING_SPELL_MESSAGES.contains(&(spell.id, msg));
                if !known && !self.messages.contains_key(&msg) {
                    errors.push(ContentError::DanglingSpellMessage {
                        spell: spell.id,
                        message: msg,
                    });
                }
            }
        }

        errors
    }
}
