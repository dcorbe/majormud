//! The quest text-block scripting VM — pure pieces (quests.md §2;
//! `perform_matched_action` 0x70209, decompile 68548-69689).
//!
//! A text-block line is a `:`-separated chain of tokens, each
//! `verb arg arg …`. The dispatcher matches the first word
//! case-insensitively against the verb table (decompile 68600-68695) and
//! each verb returns a control code. Execution lives on `Core`
//! (`game.rs::perform_matched_action`) because nearly every verb touches
//! world state; this module holds the vocabulary and parsing, the
//! `crime.rs` precedent for pure logic.
//!
//! Four verb strings the earlier RE pass left unresolved were pinned by
//! reading their arms: `DAT_0048fedc` = `cast` (68602/69639),
//! `DAT_0048ff6a` = `race` (68634/69245), `DAT_0048ffac` = `text`
//! (68648/69046), `DAT_00490051` = `flag` (68691/68319 — a whole verb
//! quests.md §2 missed: 64 player flag bits at `+0x71c`/`+0x460` with
//! set/check/fail/clear sub-ops; zero shipped uses).

/// A verb's control code (quests.md §2): the VM returns the LAST
/// dispatched verb's code — `0` when no token matched a verb at all
/// (unknown tokens are silently skipped, dispatch tail 68695-68708).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActionCode {
    /// No verb dispatched (the DLL's initial `local_24 = 0`, 68583).
    NoOp = 0,
    /// "Line consumed, keep going" (1).
    Continue = 1,
    /// "Condition failed — stop this chain" (2). The DLL also restores
    /// the un-parsed tail into the caller's buffer (`local_8[-1] = ':'`);
    /// we return the code and leave the input untouched.
    FailStop = 2,
}

/// The `perform_matched_action` verb table, in the DLL's dispatch order
/// (decompile 68600-68695). `sameto` is a case-insensitive full-word
/// match of the token's first word; order is irrelevant to us because
/// matches are exact, not prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestVerb {
    Price,
    Cast,
    GiveAbility,
    AddAbility,
    Teleport,
    RemoveAbility,
    NoMonsters,
    Monsters,
    NeedMonster,
    Summon,
    Message,
    TakeItem,
    AddExp,
    GiveItem,
    HideItem,
    CheckAbility,
    Class,
    Race,
    GoodAligned,
    EvilAligned,
    MinLevel,
    MaxLevel,
    CheckItem,
    FailItem,
    Text,
    RoomText,
    RoomItem,
    FailRoomItem,
    ClearItem,
    AddEvil,
    TestAbility,
    FailAbility,
    RemoteAction,
    Random,
    AddDelay,
    TestSkill,
    CheckSkill,
    GiveCoins,
    TestTournament,
    CheckSpell,
    Flag,
    LearnSpell,
}

/// Split one `:`-chain token into its verb and argument tail.
/// Returns `None` for an unknown or empty first word — the DLL skips
/// such tokens without touching the control code.
pub fn parse_action_token(token: &str) -> Option<(QuestVerb, &str)> {
    let token = token.trim_start();
    let (word, rest) = match token.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim_start()),
        None => (token, ""),
    };
    use QuestVerb::*;
    let verb = match word.to_ascii_lowercase().as_str() {
        "price" => Price,
        "cast" => Cast,
        "giveability" => GiveAbility,
        "addability" => AddAbility,
        "teleport" => Teleport,
        "removeability" => RemoveAbility,
        "nomonsters" => NoMonsters,
        "monsters" => Monsters,
        "needmonster" => NeedMonster,
        "summon" => Summon,
        "message" => Message,
        "takeitem" => TakeItem,
        "addexp" => AddExp,
        "giveitem" => GiveItem,
        "hideitem" => HideItem,
        "checkability" => CheckAbility,
        "class" => Class,
        "race" => Race,
        "goodaligned" => GoodAligned,
        "evilaligned" => EvilAligned,
        "minlevel" => MinLevel,
        "maxlevel" => MaxLevel,
        "checkitem" => CheckItem,
        "failitem" => FailItem,
        "text" => Text,
        "roomtext" => RoomText,
        "roomitem" => RoomItem,
        "failroomitem" => FailRoomItem,
        "clearitem" => ClearItem,
        "addevil" => AddEvil,
        "testability" => TestAbility,
        "failability" => FailAbility,
        "remoteaction" => RemoteAction,
        "random" => Random,
        "adddelay" => AddDelay,
        "testskill" => TestSkill,
        "checkskill" => CheckSkill,
        "givecoins" => GiveCoins,
        "test_tournament" => TestTournament,
        "checkspell" => CheckSpell,
        "flag" => Flag,
        "learnspell" => LearnSpell,
        _ => return None,
    };
    Some((verb, rest))
}

/// The C `atol` the arms parse every argument with: optional sign, then
/// leading digits; anything else (including a missing word) reads 0.
pub fn atol(word: &str) -> i64 {
    let word = word.trim_start();
    let (sign, digits) = match word.strip_prefix('-') {
        Some(d) => (-1, d),
        None => (1, word.strip_prefix('+').unwrap_or(word)),
    };
    let mut n: i64 = 0;
    for c in digits.chars().take_while(char::is_ascii_digit) {
        n = n.saturating_mul(10).saturating_add(i64::from(c as u8 - b'0'));
    }
    sign * n
}

/// The named-skill table for `testskill`/`checkskill`
/// (`FUN_0046f53a` 67965-68071): each name maps to a player value source
/// and the roll range `genrdn(0, range)` used by the `testskill` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillName {
    Agility,
    Strength,
    Intellect,
    Wisdom,
    Health,
    Charm,
    Spellcasting,
    Perception,
    Stealth,
    Thievery,
    Traps,
    Picklocks,
    Tracking,
    MagicResistance,
    CurrentHp,
}

impl SkillName {
    pub fn parse(word: &str) -> Option<SkillName> {
        use SkillName::*;
        Some(match word.to_ascii_lowercase().as_str() {
            "agility" => Agility,
            "strength" => Strength,
            "intellect" => Intellect,
            "wisdom" => Wisdom,
            "health" => Health,
            "charm" => Charm,
            "spellcasting" => Spellcasting,
            "perception" => Perception,
            "stealth" => Stealth,
            "thievery" => Thievery,
            "traps" => Traps,
            "picklocks" => Picklocks,
            "tracking" => Tracking,
            "magicresistance" => MagicResistance,
            "current_hp" => CurrentHp,
            _ => return None,
        })
    }

    /// The `testskill` roll range (67995-68070): stats and
    /// magicresistance 150, spellcasting 110, the six skill words 175,
    /// current_hp (and unknown names) 100.
    pub fn roll_range(self) -> i32 {
        use SkillName::*;
        match self {
            Agility | Strength | Intellect | Wisdom | Health | Charm | MagicResistance => 0x96,
            Spellcasting => 0x6e,
            Perception | Stealth | Thievery | Traps | Picklocks | Tracking => 0xaf,
            CurrentHp => 100,
        }
    }
}
