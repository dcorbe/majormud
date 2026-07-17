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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
    /// milestones (8 = map-change portal, already folded into `dest`;
    /// 10 = text-triggered action exit, hidden from the exits line).
    pub exit_type: u16,
    /// Type 10: the pipe-separated trigger phrases live in this message
    /// ("borrow skiff|go skiff|row skiff" — oracle).
    pub trigger_msg: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    /// Display lines, from the record's seven fixed `desc_N` line fields
    /// (trailing empty lines trimmed).
    pub description: Vec<String>,
    /// `room+0x43c` (`type` column): 1 = shop-active. Rooms with a
    /// `shopnum` but another type (e.g. the Silvermere Temple Healer,
    /// type 3) refuse LIST/buy — the shop reference is inert there.
    pub room_type: i16,
    /// The shop operating in this room (`shopnum` column), if any.
    pub shop: Option<ShopId>,
    /// Statically placed items (fixtures and initial floor stock).
    pub placed_items: Vec<PlacedItem>,
    /// Indexed by `Direction as usize`.
    pub exits: [Option<Exit>; 10],
}

/// A statically placed room item (`roomitems_N` + qty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedItem {
    pub item: ItemId,
    pub quantity: i16,
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

/// One of the ten spawn-loot slots (`knmsr+0x30·i`, uses `+0xe8·i`,
/// dropper `+0xfc+i`). `dropper` is a CARRY chance: rolled once at spawn
/// (`genrdn(1,100) <= dropper`), and everything carried drops at death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootSlot {
    pub item: ItemId,
    pub uses: i16,
    pub dropper: i16,
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
    /// `weaponnumber` — wielded for combat, never dropped at death.
    pub weapon: Option<ItemId>,
    /// The populated spawn-loot slots (empty template slots omitted).
    pub loot: Vec<LootSlot>,
    /// The five attack-form slots (kind 0 = unused).
    pub attacks: [AttackForm; 5],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub abilities: Vec<AbilityValue>,
    /// `+0x324[10]` — class allowlist (`class_1..10`): when non-empty,
    /// only these classes can use the item — and a match bypasses the
    /// weapon/armour permission matrix (user_can_use 0x1fced).
    pub classes: Vec<ClassId>,
    /// `+0x344[10]` — race allowlist (`race_1..10`), same semantics.
    pub races: Vec<RaceId>,
    /// `+0x2f2` — weight units (coins weigh 1/3 each, separately).
    pub weight: i16,
    /// `+0x2f4` — 0 armor, 1 weapon, 6 light, 7 stackable, 0xb class-locked…
    pub item_type: i16,
    /// `+0x31e` — default charges (-1 = permanent).
    pub uses: i16,
    /// `+0x322` in denomination `cost_denomination` (`+0x429`, 0 = copper).
    pub cost: i32,
    pub cost_denomination: i16,
    /// `+0x33e`/`+0x340` — weapon damage range.
    pub min_damage: i16,
    pub max_damage: i16,
    /// `ac` column — armor value contribution (fighter [3] via Σ`+0x39c`).
    pub ac: i16,
    /// `+0x394` — weapon sub-type/hands (1 or 3 = two-handed).
    pub weapon_type: i16,
    /// `+0x396` — armor class-strength requirement.
    pub armour_req: i16,
    /// `+0x398` — worn-location code (0 = not wearable).
    pub worn_on: i16,
    /// `+0x39a` — to-hit/skill rating (feeds attacker accuracy).
    pub accuracy: i16,
    /// `+0x342` — defense rating (feeds defender evasion /10).
    pub defense: i16,
    /// 0 = fixture ("You don't see X here." on get).
    pub gettable: i16,
    /// `+0x3a0` — strength needed to swing without the EU penalty.
    pub req_str: i16,
    /// `+0x3de` — weapon speed (EU numerator).
    pub speed: i16,
    /// `+0x40c`/`+0x410` — pipe-separated verb pools (2nd person | 3rd
    /// person lines); a verb is picked at random per swing.
    pub hit_msg: Option<MessageId>,
    pub miss_msg: Option<MessageId>,
    pub not_droppable: i16,
    pub retain_after_uses: i16,
    pub destroy_on_death: i16,
}

/// Damage element (`spell+0xd0`, `typeofattack`). Resistance keying per
/// `get_spell_random_modifier` (spellcasting.md §4): element 4 has no switch
/// case — unresistable "pure magic" (e.g. magic missile) — and the modifier
/// only applies at all when the spell's target mode is offensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Cold = 0,
    Fire = 1,
    Stone = 2,
    Lightning = 3,
    Magic = 4,
    Water = 5,
    Poison = 6,
}

impl Element {
    pub fn from_i16(v: i16) -> Option<Element> {
        Some(match v {
            0 => Element::Cold,
            1 => Element::Fire,
            2 => Element::Stone,
            3 => Element::Lightning,
            4 => Element::Magic,
            5 => Element::Water,
            6 => Element::Poison,
            _ => return None,
        })
    }

    /// The ability that resists this element; `None` for Magic (unresistable).
    pub fn resist_ability(self) -> Option<Ability> {
        let id = match self {
            Element::Cold => 3,       // Rcol
            Element::Fire => 5,       // Rfir
            Element::Stone => 65,     // ResistStone
            Element::Lightning => 66, // Rlit
            Element::Magic => return None,
            Element::Water => 147,    // ResistWater
            Element::Poison => 21,    // ImmuPoison
        };
        Some(Ability::from_id(id).expect("resist abilities are in the enum"))
    }
}

/// Spell match/delivery type (`spell+0xcc`, `target`) — selects the cast
/// entry point and target iteration (spellcasting.md §1, §3, §4). Variant
/// names are placeholders pending semantic pinning; the predicates encode
/// the decompile's groupings. Shipped data uses {0,1,2,4,6,7,8,11,12,13};
/// 3/5/9/10 are engine-valid but unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType {
    Single0 = 0,
    Single1 = 1,
    Single2 = 2,
    Area3 = 3,
    Special4 = 4,
    Area5 = 5,
    Item6 = 6,
    Item7 = 7,
    Special8 = 8,
    Area9 = 9,
    Area10 = 10,
    AreaB = 11,
    AreaC = 12,
    AreaD = 13,
}

impl MatchType {
    pub fn from_i16(v: i16) -> Option<MatchType> {
        Some(match v {
            0 => MatchType::Single0,
            1 => MatchType::Single1,
            2 => MatchType::Single2,
            3 => MatchType::Area3,
            4 => MatchType::Special4,
            5 => MatchType::Area5,
            6 => MatchType::Item6,
            7 => MatchType::Item7,
            8 => MatchType::Special8,
            9 => MatchType::Area9,
            10 => MatchType::Area10,
            11 => MatchType::AreaB,
            12 => MatchType::AreaC,
            13 => MatchType::AreaD,
            _ => return None,
        })
    }

    /// Requires an item target (`cast_item_target`, §3).
    pub fn is_item(self) -> bool {
        matches!(self, MatchType::Item6 | MatchType::Item7)
    }

    /// Iterates every valid player in the room (§4).
    pub fn room_wide(self) -> bool {
        matches!(
            self,
            MatchType::Area3
                | MatchType::Area5
                | MatchType::Area9
                | MatchType::Area10
                | MatchType::AreaB
                | MatchType::AreaC
                | MatchType::AreaD
        )
    }

    /// Also iterates the room's monsters (§4: 3/5/9/0xb/0xc).
    pub fn hits_monsters(self) -> bool {
        matches!(
            self,
            MatchType::Area3
                | MatchType::Area5
                | MatchType::Area9
                | MatchType::AreaB
                | MatchType::AreaC
        )
    }

    /// Magnitude is divided by the target count (§3: 3/5/9/10).
    pub fn splits_magnitude(self) -> bool {
        matches!(
            self,
            MatchType::Area3 | MatchType::Area5 | MatchType::Area9 | MatchType::Area10
        )
    }
}

/// Target mode (`spell+0xc4`, `spelltype`): `< 3` = offensive/combat-scoped,
/// `>= 3` = benign/self (spellcasting.md §1). Shipped data uses 0, 1, 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    Offensive0 = 0,
    Offensive1 = 1,
    Offensive2 = 2,
    Benign = 3,
}

impl TargetMode {
    pub fn from_i16(v: i16) -> Option<TargetMode> {
        Some(match v {
            0 => TargetMode::Offensive0,
            1 => TargetMode::Offensive1,
            2 => TargetMode::Offensive2,
            3 => TargetMode::Benign,
            _ => return None,
        })
    }

    pub fn is_offensive(self) -> bool {
        !matches!(self, TargetMode::Benign)
    }
}

/// Save class (`spell+0xc6`, `typeofresists`) — when the target of a
/// successful targeted cast gets a saving throw (spellcasting.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveClass {
    /// No save (1121 shipped spells).
    None = 0,
    /// Save only if the target has AntiMagic (51).
    IfAntiMagic = 1,
    /// Target always gets a save.
    Always = 2,
}

impl SaveClass {
    pub fn from_i16(v: i16) -> Option<SaveClass> {
        Some(match v {
            0 => SaveClass::None,
            1 => SaveClass::IfAntiMagic,
            2 => SaveClass::Always,
            _ => return None,
        })
    }
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

/// One shop stock slot (`shopitemnumber/max/now` + the restock triple).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShopStock {
    pub item: Option<ItemId>,
    pub max: i16,
    pub now: i16,
    /// Restock: interval minutes (0 = probabilistic top-up), amount, percent.
    pub restock_time: i16,
    pub restock_amount: i16,
    pub restock_percent: i16,
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
    /// The 20 stock slots.
    pub stock: [ShopStock; 20],
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
    /// `class+0x48` — the weapon/combat factor feeding accuracy and
    /// compute_energy_used (`combat` column).
    pub combat_factor: i16,
    /// `class+0x44` — weapon permission code (`weapon` column): 0-3 one
    /// weapon class, 4 = one-handed, 5 = two-handed, 6 = sharp,
    /// 7 = blunt, 8 = all, 9 = only the config items (quarterstaff 100,
    /// dagger 68).
    pub weapon_code: i16,
    /// `class+0x46` — max wearable armour weight class (`armour` column).
    pub armour_code: i16,
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

/// See [`KNOWN_DANGLING_MONSTER_MESSAGES`]. Item 2078 does not exist; the
/// slot simply never yields loot (the spawn roll's get_item_data fails).
pub const KNOWN_DANGLING_MONSTER_ITEMS: [(MonsterId, ItemId); 1] =
    [(MonsterId(602), ItemId(2078))]; // saracen commander

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
    DanglingMonsterItem {
        monster: MonsterId,
        item: ItemId,
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
            let referenced = monster
                .loot
                .iter()
                .map(|slot| slot.item)
                .chain(monster.weapon);
            for item in referenced {
                let known = KNOWN_DANGLING_MONSTER_ITEMS.contains(&(monster.id, item));
                if !known && !self.items.contains_key(&item) {
                    errors.push(ContentError::DanglingMonsterItem {
                        monster: monster.id,
                        item,
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
