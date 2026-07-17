# M5 Slice 1: Spell Content Layer — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Load and boot-validate the full ~20-field spell record for all 1379
spells, plus run the slice-2 oracle expedition that unblocks spellbook design.

**Architecture:** Pure content-layer work — new field types and validation in
`mud-core::content`, column mapping in `mud-server::content_db`. No game
behavior changes. Design: `docs/plans/2026-07-16-m5-magic-design.md`; spec:
`re/docs/spellcasting.md` (§1 field table).

**Tech Stack:** Rust workspace (`crates/mud-core`, `crates/mud-server`),
rusqlite 0.37, content DB at `re/mmud_wgnt.sqlite`.

**Facts pinned during planning (do not re-derive):**

- Column↔struct-offset mapping was verified against the `_raw` blobs for all
  1379 spells. Columns are the source of truth. (`levelcap` sits at a
  different disk offset than memory `+0xa2` for 74 rows — disk record ≠
  memory struct there; the column is still correct.)
- `typeofresists` (+0xc6) is the save class: 0 none / 1 if-AntiMagic /
  2 always. Data: 1121/167/91.
- `typeofattack` domain in data: 0–6. Element 4 = unresistable magic.
- `target` (match type) domain in data: {0,1,2,4,6,7,8,11,12,13}; engine
  accepts 0..=13 (`get_spell_match_type`).
- `spelltype` (target mode) domain in data: {0,1,3}; `< 3` = offensive.
- Zero scaling denominators ship (magic missile `maxincrease=1,
  lvlsmaxincr=0`) and are legal — runtime guard yields 0. NOT a load error.
- Spell-ref abilities (EndCast 151, RemovesSpell 122, KillSpell 153,
  GiveTempSpell 160): value 0 = none sentinel; all nonzero values resolve.
- Spell 1 (magic missile) pins: mana=1, level=1, min=4, max=12, energy=1000,
  difficulty=15, levelcap=6, magerya=1, mageryb=1, target=8, spelltype=0,
  typeofresists=0, typeofattack=4, duration=0, undefined01=0,
  maxincrease/lvlsmaxincr=(1,0), minincrease/lvlsminincr=(0,0),
  durincrease/lvlsdurincr=(0,0).

---

### Task 1: Spell field enums (Element, MatchType, TargetMode, SaveClass)

**Files:**
- Modify: `crates/mud-core/src/content.rs` (types near `Spell`, ~line 250)
- Test: `crates/mud-core/tests/content.rs`

**Step 1: Write the failing tests** (append to `tests/content.rs`):

```rust
#[test]
fn element_maps_ids_and_resist_abilities() {
    use mud_core::content::Element;
    assert_eq!(Element::from_i16(4), Some(Element::Magic));
    assert_eq!(Element::from_i16(7), None);
    assert_eq!(Element::Magic.resist_ability(), None); // no case 4 in get_spell_random_modifier
    assert_eq!(Element::Fire.resist_ability(), Some(Ability::from_id(5).unwrap())); // Rfir
    assert_eq!(Element::Poison.resist_ability(), Some(Ability::from_id(21).unwrap())); // ImmuPoison
}

#[test]
fn match_type_predicates_follow_spec_groupings() {
    use mud_core::content::MatchType;
    assert_eq!(MatchType::from_i16(14), None);
    assert_eq!(MatchType::from_i16(-1), None);
    let mt = |n| MatchType::from_i16(n).unwrap();
    // spellcasting.md §3/§4 groupings
    for n in [6, 7] { assert!(mt(n).is_item()); }
    for n in [3, 5, 9, 10, 11, 12, 13] { assert!(mt(n).room_wide()); }
    for n in [3, 5, 9, 11, 12] { assert!(mt(n).hits_monsters()); }
    for n in [3, 5, 9, 10] { assert!(mt(n).splits_magnitude()); }
    for n in [0, 1, 2, 4, 8] { assert!(!mt(n).room_wide() && !mt(n).is_item()); }
    assert!(!mt(10).hits_monsters());
    assert!(!mt(11).splits_magnitude());
}

#[test]
fn target_mode_offensive_threshold_is_three() {
    use mud_core::content::TargetMode;
    assert!(TargetMode::from_i16(0).unwrap().is_offensive());
    assert!(TargetMode::from_i16(2).unwrap().is_offensive());
    assert!(!TargetMode::from_i16(3).unwrap().is_offensive());
    assert_eq!(TargetMode::from_i16(4), None);
}

#[test]
fn save_class_maps_typeofresists() {
    use mud_core::content::SaveClass;
    assert_eq!(SaveClass::from_i16(0), Some(SaveClass::None));
    assert_eq!(SaveClass::from_i16(1), Some(SaveClass::IfAntiMagic));
    assert_eq!(SaveClass::from_i16(2), Some(SaveClass::Always));
    assert_eq!(SaveClass::from_i16(3), None);
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p mud-core --test content 2>&1 | tail -20`
Expected: compile FAILURE — `Element`, `MatchType`, `TargetMode`, `SaveClass`
not found in `mud_core::content`.

**Step 3: Implement** in `content.rs` (above `pub struct Spell`). Follow the
existing doc-comment style: every field/type cites its struct offset and
column. Use the `Ability` type already imported there.

```rust
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
```

**Step 4: Run to verify pass**

Run: `cargo test -p mud-core --test content 2>&1 | tail -5`
Expected: PASS (all tests, including pre-existing ones).

**Step 5: Commit**

```bash
git add crates/mud-core/src/content.rs crates/mud-core/tests/content.rs
git commit -m "feat: spell field enums — Element, MatchType, TargetMode, SaveClass"
```

---

### Task 2: ScalePair (per-level scaling fraction)

**Files:**
- Modify: `crates/mud-core/src/content.rs`
- Test: `crates/mud-core/tests/content.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn scale_pair_guards_zero_denominator() {
    use mud_core::content::ScalePair;
    // Magic missile ships per=1, levels=0 — the engine's guard yields 0.
    assert_eq!(ScalePair { per: 1, levels: 0 }.scaled(10), 0);
    assert_eq!(ScalePair { per: 3, levels: 2 }.scaled(10), 15);
    assert_eq!(ScalePair { per: 1, levels: 3 }.scaled(8), 2); // integer division
    assert_eq!(ScalePair::NONE.scaled(50), 0);
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p mud-core --test content scale_pair 2>&1 | tail -10`
Expected: compile FAILURE — `ScalePair` not found.

**Step 3: Implement** in `content.rs`:

```rust
/// A per-level scaling fraction: `per` points per `levels` levels
/// (numerator/denominator byte pairs at spell `+0xf2/f3`, `+0xf6/f7`,
/// `+0xf8/f9`). The engine guards zero denominators — they contribute 0
/// (magic missile ships one; spellcasting.md §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalePair {
    pub per: u8,
    pub levels: u8,
}

impl ScalePair {
    pub const NONE: ScalePair = ScalePair { per: 0, levels: 0 };

    /// `per * level / levels`, 0 when the denominator is 0.
    pub fn scaled(self, level: i32) -> i32 {
        if self.levels == 0 {
            0
        } else {
            i32::from(self.per) * level / i32::from(self.levels)
        }
    }
}
```

**Step 4: Run to verify pass** — same command, expected PASS.

**Step 5: Commit**

```bash
git add crates/mud-core/src/content.rs crates/mud-core/tests/content.rs
git commit -m "feat: ScalePair per-level scaling fraction with zero-denominator guard"
```

---

### Task 3: Full spell record — struct + loader (one commit)

The struct extension breaks `mud-server`'s `load_spells` until the loader
learns the new columns, so struct + fixture + loader land as ONE task and ONE
commit. Do not commit mid-task.

**Files:**
- Modify: `crates/mud-core/src/content.rs:250-258` (`Spell`)
- Modify: `crates/mud-core/tests/content.rs` (fixture + existing literal at ~line 125)
- Modify: `crates/mud-server/src/content_db.rs:375-393` (`load_spells`), plus
  a `to_u8` helper next to `to_u16`/`to_i16` (~line 69)
- Test: `crates/mud-server/tests/load_real_db.rs`

**Step 1: Write the failing test** (append to `load_real_db.rs`; follow the
existing `content_db::load(&db_path())` pattern):

```rust
#[test]
fn magic_missile_cast_fields_load_exactly() {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, SpellId, TargetMode};
    let content = content_db::load(&db_path()).expect("load content db");
    let mm = &content.spells[&SpellId(1)];
    assert_eq!(mm.name, "magic missile");
    assert_eq!(mm.mana_cost, 1);
    assert_eq!(mm.required_power, 1);
    assert_eq!((mm.min_base, mm.max_base), (4, 12));
    assert_eq!(mm.round_cost, 1000);
    assert_eq!(mm.base_chance, 15);
    assert_eq!(mm.level_cap, 6);
    assert_eq!(mm.class_gate_group, 1);
    assert_eq!(mm.required_class_level, 1);
    assert_eq!(mm.target_mode, TargetMode::Offensive0);
    assert_eq!(mm.save_class, SaveClass::None);
    assert_eq!(mm.match_type, MatchType::Special8);
    assert_eq!(mm.element, Element::Magic);
    assert_eq!(mm.duration, 0);
    assert_eq!(mm.duration_per_level, 0);
    assert_eq!(mm.max_increase, ScalePair { per: 1, levels: 0 });
    assert_eq!(mm.min_increase, ScalePair::NONE);
    assert_eq!(mm.duration_increase, ScalePair::NONE);
}
```

Run: `cargo test -p mud-server --test load_real_db 2>&1 | tail -10`
Expected: compile FAILURE — `Spell` has none of these fields.

**Step 2: Extend `Spell`** in `content.rs` — keep existing fields first;
field docs cite offset + column per house style:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spell {
    pub id: SpellId,
    pub name: String,
    pub short_name: String,
    pub cast_msg_a: Option<MessageId>,
    pub cast_msg_b: Option<MessageId>,
    pub abilities: Vec<AbilityValue>,
    /// `+0xa2` `levelcap` — caster level is clamped to this before scaling.
    pub level_cap: i16,
    /// `+0xbc` `energy` — round-action cost, deducted from the round pool.
    pub round_cost: i16,
    /// `+0xbe` `level` — required caster spell power ("too powerful for you").
    pub required_power: i16,
    /// `+0xc0` `min` — base magnitude lower bound.
    pub min_base: i16,
    /// `+0xc2` `max` — base magnitude upper bound.
    pub max_base: i16,
    /// `+0xc4` `spelltype`.
    pub target_mode: TargetMode,
    /// `+0xc6` `typeofresists` — target saving-throw class.
    pub save_class: SaveClass,
    /// `+0xc8` `difficulty` — base success chance %; `>= 200` auto-succeeds.
    pub base_chance: i16,
    /// `+0xca` `undefined01` — duration-per-level multiplier (duration max
    /// = this × effective level; spellcasting.md §4).
    pub duration_per_level: i16,
    /// `+0xcc` `target`.
    pub match_type: MatchType,
    /// `+0xce` `duration` — base duration in ticks; 0 = instant.
    pub duration: i16,
    /// `+0xd0` `typeofattack`.
    pub element: Element,
    /// `+0xd6` `magerya` — class-gate group; 0 = ungated.
    pub class_gate_group: i16,
    /// `+0xf0` `mana` — mana cost (half is charged on a failed roll).
    pub mana_cost: i16,
    /// `+0xf2/+0xf3` `maxincrease/lvlsmaxincr` — max-bound per-level scaling.
    pub max_increase: ScalePair,
    /// `+0xf4` `mageryb` — required level within the class.
    pub required_class_level: i16,
    /// `+0xf6/+0xf7` `minincrease/lvlsminincr` — min-bound per-level scaling.
    pub min_increase: ScalePair,
    /// `+0xf8/+0xf9` `durincrease/lvlsdurincr` — duration per-level scaling.
    pub duration_increase: ScalePair,
}
```

**Step 3: Fix the mud-core test literal.** Add a fixture helper next to the
existing `monster(n)` helper in `tests/content.rs`, and rewrite the literal
in `known_dangling_spell_message_is_allowlisted` to use it:

```rust
fn spell(n: u16) -> Spell {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, TargetMode};
    Spell {
        id: SpellId(n),
        name: format!("spell {n}"),
        short_name: String::new(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![],
        level_cap: 0,
        round_cost: 0,
        required_power: 0,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 0,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
    }
}
```

In the existing test:

```rust
let mut s = spell(1055);
s.name = "BCNS".into();
s.cast_msg_b = Some(MessageId(3499));
content.add_spell(s);
```

(Move the `use mud_core::content::{Spell, SpellId}` import to the top of the
file if it isn't already there.)

Run: `cargo test -p mud-core 2>&1 | tail -5` — expected PASS.
(`mud-server` is still red; next step fixes it.)

**Step 4: Update the loader.** Add the helper:

```rust
fn to_u8(table: &'static str, field: &str, v: i64) -> Result<u8, LoadError> {
    u8::try_from(v).map_err(|_| invalid(table, format!("{field} = {v} does not fit u8")))
}
```

Replace `load_spells`:

```rust
fn load_spells(db: &Connection, content: &mut Content) -> Result<(), LoadError> {
    let mut stmt = db.prepare(&format!(
        "SELECT number, name, shortname, castmsga, castmsgb, {}, {}, \
         levelcap, energy, level, min, max, spelltype, typeofresists, \
         difficulty, undefined01, target, duration, typeofattack, magerya, \
         mana, maxincrease, lvlsmaxincr, mageryb, minincrease, lvlsminincr, \
         durincrease, lvlsdurincr FROM spell",
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
        let target_mode = field("spelltype", 30)?;
        let save_class = field("typeofresists", 31)?;
        let match_type = field("target", 34)?;
        let element = field("typeofattack", 36)?;
        content.add_spell(Spell {
            id: SpellId(to_u16("spell", "number", row.get(0)?)?),
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
            target_mode: TargetMode::from_i16(target_mode)
                .ok_or_else(|| invalid("spell", format!("spelltype = {target_mode}")))?,
            save_class: SaveClass::from_i16(save_class)
                .ok_or_else(|| invalid("spell", format!("typeofresists = {save_class}")))?,
            base_chance: field("difficulty", 32)?,
            duration_per_level: field("undefined01", 33)?,
            match_type: MatchType::from_i16(match_type)
                .ok_or_else(|| invalid("spell", format!("target = {match_type}")))?,
            duration: field("duration", 35)?,
            element: Element::from_i16(element)
                .ok_or_else(|| invalid("spell", format!("typeofattack = {element}")))?,
            class_gate_group: field("magerya", 37)?,
            mana_cost: field("mana", 38)?,
            max_increase: pair("maxincrease", 39, "lvlsmaxincr", 40)?,
            required_class_level: field("mageryb", 41)?,
            min_increase: pair("minincrease", 42, "lvlsminincr", 43)?,
            duration_increase: pair("durincrease", 44, "lvlsdurincr", 45)?,
        });
    }
    Ok(())
}
```

Update the `use mud_core::content::{...}` import at the top of
`content_db.rs` to add `Element, MatchType, SaveClass, ScalePair, TargetMode`.

Column indices: 0–4 fixed, 5–14 `abilitya_*`, 15–24 `abilityb_*`, 25+ as
listed in the SELECT. Double-check by counting the SELECT, not by trusting
this paragraph.

**Step 5: Run the full workspace**

Run: `cargo test 2>&1 | tail -10`
Expected: PASS everywhere — including `full_database_loads_and_validates`,
which now proves every one of the 1379 spells carries valid enum values.

**Step 6: Clippy, then commit**

Run: `cargo clippy --all-targets 2>&1 | tail -5` — fix anything it raises.

```bash
git add crates/mud-core/src/content.rs crates/mud-core/tests/content.rs \
        crates/mud-server/src/content_db.rs crates/mud-server/tests/load_real_db.rs
git commit -m "feat: full spell cast-path record — all 1379 spells load and boot-validate"
```

---

### Task 4: Spell-reference validation (EndCast / dispel / temp-spell)

**Files:**
- Modify: `crates/mud-core/src/content.rs` (`ContentError`, `validate`)
- Test: `crates/mud-core/tests/content.rs`

**Step 1: Write the failing tests** (these use the `spell(n)` fixture from
Task 3):

```rust
#[test]
fn dangling_spell_reference_fails_validation() {
    use mud_core::content::ContentError;
    let mut content = Content::default();
    let mut s = spell(1);
    // EndCast (151) pointing at a spell that doesn't exist.
    s.abilities = vec![(Ability::from_id(151).unwrap(), 999)];
    content.add_spell(s);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingSpellRef {
            spell: SpellId(1),
            ability: Ability::from_id(151).unwrap(),
            referenced: SpellId(999),
        }]
    );
}

#[test]
fn zero_spell_reference_is_the_none_sentinel() {
    // 14 shipped slots carry EndCast/RemovesSpell value 0 = "none".
    let mut content = Content::default();
    let mut s = spell(1);
    s.abilities = vec![(Ability::from_id(151).unwrap(), 0)];
    content.add_spell(s);
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn resolving_spell_references_pass() {
    let mut content = Content::default();
    let mut s = spell(1);
    s.abilities = vec![
        (Ability::from_id(122).unwrap(), 2), // RemovesSpell -> spell 2
        (Ability::from_id(153).unwrap(), 2), // KillSpell -> spell 2
        (Ability::from_id(160).unwrap(), 2), // GiveTempSpell -> spell 2
    ];
    content.add_spell(s);
    content.add_spell(spell(2));
    assert_eq!(content.validate(), vec![]);
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p mud-core --test content spell_reference 2>&1 | tail -10`
Expected: compile FAILURE — no `DanglingSpellRef` variant.

**Step 3: Implement.** Add the variant to `ContentError`:

```rust
    DanglingSpellRef {
        spell: SpellId,
        ability: Ability,
        referenced: SpellId,
    },
```

In `validate()`, after the existing spell-message loop:

```rust
        // EndCast (151), RemovesSpell (122), KillSpell (153) and
        // GiveTempSpell (160) values name other spells; 0 = none.
        let spell_refs =
            [122, 151, 153, 160].map(|id| Ability::from_id(id).expect("in the enum"));
        for spell in self.spells.values() {
            for &(ability, value) in &spell.abilities {
                if spell_refs.contains(&ability)
                    && value != 0
                    && !self.spells.contains_key(&SpellId(value as u16))
                {
                    errors.push(ContentError::DanglingSpellRef {
                        spell: spell.id,
                        ability,
                        referenced: SpellId(value as u16),
                    });
                }
            }
        }
```

Note: `value as u16` — all shipped references are positive; a negative value
would wrap and fail the lookup, which is the desired loud outcome.

**Step 4: Run the full workspace to verify pass**

Run: `cargo test 2>&1 | tail -5`
Expected: PASS — including the full-DB test, which proves all shipped
references resolve.

**Step 5: Clippy, then commit**

```bash
git add crates/mud-core/src/content.rs crates/mud-core/tests/content.rs
git commit -m "feat: validate EndCast/dispel/temp-spell references at boot"
```

---

### Task 5: Oracle expedition — spell learning & listings (slice-2 prep)

Exploratory, not TDD. Harness docs: `tools/oracle/README.md` (MBBSEmu at
`~/mbbsemu`, telnet 2327, BBS account Oracle/test123). MBBSEmu data may be
edited freely; oracle characters are disposable. Name validation takes ~10
min per new character — batch other work while waiting.

**Goal:** answer the questions blocking spellbook/cast-command design.

**Checklist:**

1. Roll a fresh **Mage** (or Priest — whichever the class list makes the
   cheapest caster; note both if quick). Capture the creation transcript.
2. Immediately after creation: run `spells` — capture exact output (empty
   book vs pre-seeded class spells answers the trainer-grant question at
   level 1).
3. Visit the Newhaven guild/trainer shop, `list` — capture scroll listings:
   names, prices, any `(Too powerful)` suffix. If Newhaven has no caster
   guild, note it and take the ferry to Silvermere.
4. Buy the cheapest scroll. Try the verbs: `learn <scroll>`, `use <scroll>`,
   `read <scroll>` — record which works and the exact success string.
5. `spells` again — capture the learned-spell listing format (columns,
   ordering, mana display).
6. `train` a level if affordable; `spells` again — did training add spells?
   (This settles trainer-grant vs scroll-only.)
7. Failure-message capture: `cast zzz` (unknown), cast a known spell with
   insufficient mana (drain first), cast a spell too high for the class.
   One transcript each.
8. Cast the learned spell successfully (at self or a rat) — capture the
   success/broadcast strings and the prompt's mana display.
9. Save transcripts under `re/oracle/` following the existing naming
   pattern; append findings to `re/docs/spellcasting.md` (new §
   "Learning & listings (oracle-measured)") and commit as
   `doc: oracle findings — spell learning model + listing formats`.

**CHECKPOINT — STOP HERE.** Review the expedition findings with Daniel, then
write the slice-2/slice-3 implementation plan (spellbook + cast skeleton)
from the measured data.
