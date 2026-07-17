//! Every player-visible string the engine emits, in one place.
//!
//! Strings marked `VERIFIED` were read out of WCCMMUD.DLL; strings marked
//! `ORACLE-VERIFY` are placeholders to be corrected against MBBSEmu
//! transcripts. Keeping them here makes those corrections one-line diffs.

/// VERIFIED (DLL): broadcast when a player enters the game.
pub fn entered_realm(name: &str) -> String {
    format!("{name} just entered the Realm.")
}

/// VERIFIED (DLL): broadcast when a player leaves the game.
pub fn left_realm(name: &str) -> String {
    format!("{name} just left the Realm.")
}

/// VERIFIED (oracle): input matching no command is spoken aloud.
pub fn you_say(what: &str) -> String {
    format!("You say \"{what}\"")
}

/// VERIFIED (DLL): what the rest of the room hears.
pub fn says(name: &str, what: &str) -> String {
    format!("{name} says \"{what}\"")
}

/// VERIFIED (DLL): moving where no exit exists.
pub const NO_EXIT: &str = "There is no exit in that direction!";

/// VERIFIED (DLL): the exits-line prefix and empty-exits marker.
pub const OBVIOUS_EXITS: &str = "Obvious exits: ";
pub const NO_EXITS: &str = "NONE!!!";

/// VERIFIED (DLL): the occupant-line prefix.
pub const ALSO_HERE: &str = "Also here: ";

use crate::content::Direction;

/// Display name used in the exits list. Vertical exits show as "up"/"down"
/// (USER TESTIMONY — the earlier "above"/"below" reading of the DLL string
/// table was wrong; that pair belongs to the vertical arrival broadcasts,
/// see [`arrived_from`]).
pub fn direction_shown(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        Direction::NorthEast => "northeast",
        Direction::NorthWest => "northwest",
        Direction::SouthEast => "southeast",
        Direction::SouthWest => "southwest",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

/// VERIFIED (DLL): departure broadcast. Compass exits use
/// "just left to the <dir>."; vertical exits have dedicated phrasings.
pub fn left_via(name: &str, direction: Direction) -> String {
    match direction {
        Direction::Up => format!("{name} just left upwards."),
        Direction::Down => format!("{name} just left downwards."),
        d => format!("{name} just left to the {}.", direction_shown(d)),
    }
}

/// VERIFIED (DLL) format string for compass arrivals; vertical arrivals use
/// "from above"/"from below" without the article (ORACLE-VERIFY the exact
/// vertical wording).
pub fn arrived_from(name: &str, direction: Direction) -> String {
    match direction {
        Direction::Up => format!("{name} just arrived from above."),
        Direction::Down => format!("{name} just arrived from below."),
        d => format!("{name} just arrived from the {}.", direction_shown(d)),
    }
}

/// VERIFIED (DLL + oracle): character-creation prompts. Blank input gets the
/// "choose a race/class" wording; a wrong entry gets the "valid" wording.
pub const CHOOSE_RACE: &str = "Please choose a race from the following list:";
pub const CHOOSE_CLASS: &str = "Please choose a class from the following list:";
pub const RACE_PROMPT: &str = "Please choose your race [ ? for help ] :";
pub const CLASS_PROMPT: &str = "Please choose your class [ ? for help ] :";
pub const EMPTY_RACE: &str = "You must choose a race. [ ? for help ]";
pub const EMPTY_CLASS: &str = "You must choose a class. [ ? for help ]";
pub const INVALID_RACE: &str = "You must choose a valid race. [ ? for help ]";
pub const INVALID_CLASS: &str = "You must choose a valid class. [ ? for help ]";

/// VERIFIED (oracle): creation list entry — "[N]" padded to four columns.
pub fn list_entry(number: u16, name: &str) -> String {
    format!("{:<4} {}\n", format!("[{number}]"), name)
}

/// VERIFIED (oracle): the exp command line. The parenthesised number is the
/// exp still needed; the percent is progress toward the next level.
pub fn exp_line(exp: u64, level: u16, needed: u64) -> String {
    let remaining = needed.saturating_sub(exp);
    let percent = if needed == 0 { 100 } else { exp * 100 / needed };
    format!("Exp: {exp} Level: {level} Exp needed for next level: {needed} ({remaining}) [{percent}%]")
}

/// VERIFIED (oracle): the health command line.
pub fn health_line(current: i32, max: i32) -> String {
    let percent = if max == 0 { 0 } else { current * 100 / max };
    format!("Health:{current:>6}/{max:<6}[{percent}%]")
}

/// VERIFIED (oracle/DLL): training messages.
pub const TRAIN_WRONG_ROOM: &str = "You must be in an appropriate training room to train!";
pub const TRAIN_NO_EXP: &str = "You do not have the required experience to train yet!";
pub const TRAIN_NO_MONEY: &str = "You do not have the money required for your training.";

/// VERIFIED (DLL): " and you receive training to attain level %d." — the
/// leading fragment follows the payment sentence; ORACLE-VERIFY the full
/// combined line once a fund run is captured.
pub fn train_success(level: u16) -> String {
    format!("you receive training to attain level {level}.")
}

/// VERIFIED (oracle): the Lawful prompt (verbatim, including the double
/// space before [Yes/No]).
pub const LAWFUL_PARAGRAPH: &str = "\
You must now choose if you want to be a truly 'lawful' citizen of the realm.
If you answer YES to this question then you will never be allowed to instigate
any action which would give you evil points, and any player that attacks or
robs from you will receive three times the regular evil points in return. You
can still attack those with a bad reputation. This option is designed
solely for those who want to stay away from the player combat aspects of the
game, and choosing it for any other reason (Item storage, etc...) is strictly
prohibited.

Remember, this is a very important choice. Once you have chosen Lawfulness,
you may not remove this title, unless you start a new character.
";
pub const LAWFUL_QUESTION: &str = "Do you want to be Lawful?  [Yes/No]";

/// VERIFIED (oracle): the delayed-exit announcement; dots follow one per
/// second until departure.
pub const EXIT_MEDITATION: &str = "You will exit after a period of silent meditation.";

/// VERIFIED (oracle): rejection while the exit meditation is pending —
/// commands are refused, not silently swallowed.
pub const MEDITATION_BLOCKED: &str = "You may not perform any commands while waiting to exit!";

/// VERIFIED (oracle): syntax lines for argument commands invoked bare.
pub const SYNTAX_AID: &str = "Syntax: AID {user name}";
pub const SYNTAX_GET: &str = "Syntax: GET {Item Name}";

/// VERIFIED (oracle, first line; remainder ORACLE-VERIFY).
pub const HELP_BANNER: &str = "Type HELP followed by a topic for help on that topic";

/// VERIFIED (oracle): the top command header.
pub const TOP_HEADER: &str = "Top Heroes of the Realm\n-=-=-=-=-=-=-=-=-=-=-=-";

// --- inventory strings (VERIFIED oracle_m4_items.raw / round2) ---
pub const CARRYING_NOTHING: &str = "You are carrying Nothing!";
pub const NO_KEYS: &str = "You have no keys.";

pub fn took_item(name: &str) -> String {
    format!("You took {name}.")
}
pub fn dropped_item(name: &str) -> String {
    format!("You dropped {name}.")
}
pub fn dont_have_to_drop(name: &str) -> String {
    format!("You don't have {name} to drop!")
}
pub fn dont_see_here(name: &str) -> String {
    format!("You don't see {name} here.")
}
pub fn dont_see_coins(plural: &str) -> String {
    format!("You don't see any {plural}")
}
/// VERIFIED (oracle_bank.raw): "You picked up 11 silver nobles".
pub fn took_coins(count: u32, name_one: &str, name_many: &str) -> String {
    let name = if count == 1 { name_one } else { name_many };
    format!("You picked up {count} {name}")
}

/// A coin listing high->low ("1 gold crown, 9 copper farthings").
pub fn coin_listing(drawers: [u32; 5]) -> Option<String> {
    coin_pile_names(drawers)
}

// --- banking strings (VERIFIED oracle_bank3.raw) ---
pub fn balance_lines(shop: &str, shop_id: u16, copper: u64, gold_ratio: u64) -> String {
    let gold = copper / gold_ratio;
    let rem_silver = (copper % gold_ratio) / 10;
    format!(
        "Your balance at {shop} (#{shop_id}) is:\nOn deposit: {copper} copper farthings [{gold}. {rem_silver} gold crowns]"
    )
}
pub fn deposited(coins: &str) -> String {
    format!("You deposit {coins}.")
}
pub fn withdrew(copper: u64) -> String {
    format!(
        "You withdrew {copper} {}.",
        if copper == 1 { "copper farthing" } else { "copper farthings" }
    )
}
pub const UNREASONABLE_AMOUNT: &str = "Please specify a more reasonable amount.";
pub const NOT_IN_BANK_DEPOSIT: &str = "You cannot DEPOSIT if you are not in a bank!";
pub const NOT_IN_BANK_WITHDRAW: &str = "You cannot WITHDRAW if you are not in a bank!";


/// Encumbrance descriptor bands (only "None" oracle-observed; the rest
/// ORACLE-VERIFY).
pub fn encumbrance_descriptor(percent: i64) -> &'static str {
    match percent {
        p if p < 33 => "None",
        p if p < 66 => "Light",
        p if p < 100 => "Medium",
        _ => "Heavy",
    }
}

// --- equipment strings (VERIFIED oracle_m4_round2.raw except as noted) ---
pub fn now_holding(name: &str) -> String {
    format!("You are now holding {name}.")
}
pub fn not_unequipped(name: &str) -> String {
    format!("You do not have {name} left unequipped.")
}
pub fn now_wearing(name: &str) -> String {
    format!("You are now wearing {name}.")
}
pub fn removed_item(name: &str) -> String {
    format!("You have removed {name}.")
}

/// The worn-location name table (`PTR_s_Nowhere_004801dc`), indexed by
/// item `wornon`; rendered as the worn suffix ("chain coif (Head)").
pub const WORN_LOCATIONS: [&str; 17] = [
    "Nowhere", "Worn", "Head", "Hands", "Finger", "Feet", "Arms", "Back",
    "Neck", "Legs", "Waist", "Torso", "Off-Hand", "Wrist", "Ears", "Eyes",
    "Face",
];

pub fn worn_location(worn_on: i16) -> &'static str {
    WORN_LOCATIONS
        .get(usize::try_from(worn_on).unwrap_or(0))
        .copied()
        .unwrap_or("Nowhere")
}
pub fn not_wearing(name: &str) -> String {
    format!("You are not wearing {name}.")
}

// --- shop strings (list format + wordings VERIFIED oracle_m4_verify.raw) ---
pub const SHOP_HEADER: &str = "The following items are for sale here:\n\nItem                          Quantity    Price\n------------------------------------------------------";

/// One free row: name %-30, quantity %-10, three-space gutter, "Free".
pub fn shop_row_free(name: &str, quantity: i16) -> String {
    format!("{name:<30}{quantity:<10}   Free")
}

/// One priced row: the price VALUE is in the item's own cost denomination
/// (shelf = cost x (markup+100)/100), right-aligned width 4, then the
/// denomination label ("lantern ... 40           4 gold crowns").
pub fn shop_row_priced(name: &str, quantity: i16, value: i64, denomination: usize) -> String {
    let (one, many) = COIN_NAMES[denomination.min(4)];
    let label = if value == 1 { one } else { many };
    format!("{name:<30}{quantity:<10}{value:>4} {label}")
}

pub fn bought_free(name: &str) -> String {
    format!("You just bought {name} for nothing.")
}
/// `coins` = the coins actually handed over (deduct_currency's tell line).
pub fn bought_for(name: &str, coins: &str) -> String {
    format!("You just bought {name} for {coins}.")
}
pub fn not_known_item(name: &str) -> String {
    format!("{name} is not a known item.")
}
pub fn cannot_buy_here(name: &str) -> String {
    format!("You cannot buy {name} here!")
}
/// ORACLE-VERIFY wording.
pub fn cannot_afford(name: &str) -> String {
    format!("You cannot afford {name}.")
}
pub fn sold_for(name: &str, price: &str) -> String {
    format!("You sold {name} for {price}.")
}
pub fn cannot_sell_here(name: &str) -> String {
    format!("You cannot sell {name} here.")
}
pub const NOT_IN_SHOP_LIST: &str = "You cannot LIST if you are not in a shop!";
/// VERIFIED (spellcasting.md §8.3): list-row suffixes. Non-scroll items
/// gate by user_can_use → CANT_USE_SUFFIX. LearnSp scrolls gate by
/// spell_gate on the taught spell: WrongClass → CANT_USE_SUFFIX,
/// TooPowerful (character level below the spell's required power) →
/// TOO_POWERFUL_SUFFIX.
pub const CANT_USE_SUFFIX: &str = " (You can't use)";
pub const TOO_POWERFUL_SUFFIX: &str = " (Too powerful)";
pub const MAY_NOT_WEAR: &str = "You may not wear that item!";
pub const MAY_NOT_USE_WEAPON: &str = "You may not use that weapon.";

// --- spellbook strings (VERIFIED oracle_spell_train.raw /
// oracle_spell_learning.raw; spellcasting.md §8.5) ---

/// VERIFIED (oracle): the `spells` listing header pair.
pub const SPELLS_HEADER: &str = "You have the following spells:\nLevel Mana Short Spell Name";
/// VERIFIED (oracle): the empty-book reply — a single line, no header,
/// no trailing blank.
pub const NO_SPELLS: &str = "You have no spells.";

/// VERIFIED (oracle_spell_train.raw lines 74-75/190-192): one book row.
/// Measured columns: level right-aligned width 3, mana right-aligned
/// width 4, four spaces, short name left-aligned width 6, spell name
/// left-aligned width 30 — trailing spaces are part of the line
/// (`  1   4    blur  blur` + 26 spaces). Shipped short names run 0-5
/// chars (`spray` = 5), so the short column's 6 could also be 5 + a gutter —
/// indistinguishable in the data we have.
pub fn spell_row(level: i16, mana: i16, short: &str, name: &str) -> String {
    format!("{level:>3}{mana:>4}    {short:<6}{name:<30}")
}

// --- use/read strings (VERIFIED oracle_spell_learning.raw /
// oracle_use_verbs.raw / oracle_use_verbs2.raw; spellcasting.md §8.4) ---

/// VERIFIED (§8.4): the learn line shared by both verbs.
pub fn learned_spell(item: &str, spell: &str) -> String {
    format!("You read {item} and learn the spell {spell}.")
}
/// VERIFIED (§8.4): `read`'s epilogue line (`use` prints a blank line
/// instead).
pub const SCROLL_DISINTEGRATES: &str = "Its magic used, the scroll disintegrates.";
/// VERIFIED (§8.4): refusal for a too-high or wrong-class scroll AND for
/// an owned item with no use action — the item is kept in every case.
pub const MAY_NOT_USE_ITEM: &str = "You may not use that item!";
/// VERIFIED (§8.4): a scroll whose spell is already in the book — both
/// verbs, not consumed.
pub const ALREADY_KNOW_SCROLL: &str = "You realize that you already know this scroll!";
/// VERIFIED (§8.4): `use {arg}` with no owned match; `use` never falls
/// back to the shop shelf.
pub fn dont_have(name: &str) -> String {
    format!("You don't have {name}.")
}
/// VERIFIED (§8.4): `read {arg}` with nothing owned and nothing visible.
pub fn do_not_see_here(name: &str) -> String {
    format!("You do not see {name} here!")
}

/// The item description paragraph (VERIFIED oracle_spell_cast.raw `read
/// scroll of smite`, raw bytes): the stored desc lines re-flow as one word
/// stream, each word emitted with a trailing space; a line breaks before
/// the word that would pass the wrap column, and the break swallows the
/// pending space — so interior lines end flush and the final line keeps
/// one trailing space. Wrap column 79: the measured break bounds it to
/// 77..=79 (line ends at col 77, next word would end at 80). ORACLE-VERIFY
/// with a description whose lines re-flow near the boundary.
pub fn item_description(lines: &[String]) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    for word in lines.iter().flat_map(|l| l.split_whitespace()) {
        if col > 0 && col + word.len() > 79 {
            out.pop(); // the wrap swallows the pending space
            out.push('\n');
            col = 0;
        }
        out.push_str(word);
        out.push(' ');
        col += word.len() + 1;
    }
    out
}

// --- healer strings (VERIFIED oracle_healer2.raw) ---
pub fn healed(coins: &str) -> String {
    format!("You hand over {coins} and all your wounds are healed.")
}
pub fn not_poisoned(coins: &str) -> String {
    format!("You hand over {coins} and find that you were not poisoned!")
}

/// A copper amount as coin words.
pub fn copper_amount(total: u64) -> String {
    let (one, many) = COIN_NAMES[0];
    format!("{total} {}", if total == 1 { one } else { many })
}

// --- combat strings (VERIFIED oracle_attack3.raw / oracle_downed.raw / DLL) ---
pub const COMBAT_ENGAGED: &str = "*Combat Engaged*";
pub const COMBAT_OFF: &str = "*Combat Off*";
pub const NO_TARGET: &str = "You don't see your target here.";
pub const MORTALLY_WOUNDED: &str = "You may not do that while you are mortally wounded!";

/// "You punch kobold thief for 1 damage!" — monster name without article.
pub fn player_hit(verb: &str, target: &str, damage: i32) -> String {
    format!("You {verb} {target} for {damage} damage!")
}

/// "You swing at kobold thief!" — the verb is the weapon's miss verb.
pub fn player_miss(verb: &str, target: &str) -> String {
    format!("You {verb} {target}!")
}

/// "Your swing at kobold thief hits, but glances off its armour."
pub fn player_glance(verb: &str, target: &str) -> String {
    format!("Your {verb} {target} hits, but glances off its armour.")
}

/// ORACLE-VERIFY: the critical variant was not captured.
pub fn player_crit(verb: &str, target: &str, damage: i32) -> String {
    format!("You critically {verb} {target} for {damage} damage!")
}

/// "The kobold thief stabs you for 5 damage!" — the verb comes from the
/// attack form's hit message; "hits" is the fallback until message-table
/// rendering lands.
pub fn monster_hit(name: &str, verb: &str, damage: i32) -> String {
    format!("The {name} {verb} you for {damage} damage!")
}

/// "%s drops to the ground!" (DLL + oracle).
pub fn drops_to_ground(name: &str) -> String {
    format!("{name} drops to the ground!")
}

/// DLL "%s is dead." — the monster-kill announcement (article form
/// ORACLE-VERIFY).
pub fn monster_dead(name: &str) -> String {
    format!("The {name} is dead.")
}

/// VERIFIED (DLL): "You gain %s experience."
pub fn gain_experience(amount: u64) -> String {
    format!("You gain {amount} experience.")
}

/// VERIFIED (DLL): coin denomination names, low to high.
pub const COIN_NAMES: [(&str, &str); 5] = [
    ("copper farthing", "copper farthings"),
    ("silver noble", "silver nobles"),
    ("gold crown", "gold crowns"),
    ("platinum piece", "platinum pieces"),
    ("runic coin", "runic coins"), // slot 5 is sysop-defined; ORACLE-VERIFY
];

/// VERIFIED (oracle): "You notice 7 silver nobles, 43 copper farthings here."
/// — piles listed high to low.
pub fn coin_pile_names(piles: [u32; 5]) -> Option<String> {
    let mut parts = Vec::new();
    for idx in (0..5).rev() {
        let count = piles[idx];
        if count == 0 {
            continue;
        }
        let (one, many) = COIN_NAMES[idx];
        parts.push(format!("{count} {}", if count == 1 { one } else { many }));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// VERIFIED (oracle): the status prompt. Caster/Kai variants ORACLE-VERIFY.
pub fn prompt(hp: i32, mana: i32, caster_group: i16) -> String {
    match caster_group {
        1..=4 => format!("[HP={hp}/MA={mana}]:"),
        5 => format!("[HP={hp}/KAI={mana}]:"),
        _ => format!("[HP={hp}]:"),
    }
}

/// Inputs for the status sheet (`show_status`, decompile 0x34448).
pub struct SheetData<'a> {
    pub name: &'a str,
    pub race: &'a str,
    pub class: &'a str,
    pub level: u16,
    pub lives: u16,
    pub cp: u16,
    pub experience: u64,
    pub hp_current: i32,
    pub hp_max: i32,
    pub armour_class: i32,
    pub armour_max: i32,
    pub stats: crate::content::StatBlock,
    pub derived: &'a crate::stats::Derived,
}

/// VERIFIED (oracle): the nine-line status sheet, byte-exact to the
/// transcript except the whitelisted Martial Arts WG3-NT/DOS divergence.
/// Three columns at 0/18/39; right column label+value is 18 wide.
/// Non-caster layout; the caster Mana/Spellcasting line is ORACLE-VERIFY.
pub fn stat_sheet(d: &SheetData<'_>) -> String {
    let mut out = String::new();
    let mut row = |left: String, mid: String, right: String| {
        if mid.is_empty() && left.is_empty() {
            out.push_str(&format!("{:<39}{right}\n", ""));
        } else if mid.is_empty() {
            out.push_str(&format!("{left:<39}{right}\n"));
        } else {
            out.push_str(&format!("{left:<18}{mid:<21}{right}\n"));
        }
    };
    row(
        format!("Name: {}", d.name),
        String::new(),
        format!("Lives/CP:{:>7}/{:<5}", d.lives, d.cp),
    );
    row(
        format!("Race: {}", d.race),
        format!("Exp: {}", d.experience),
        format!("Perception:{:>7}", d.derived.perception),
    );
    row(
        format!("Class: {}", d.class),
        format!("Level: {}", d.level),
        format!("Stealth:{:>10}", d.derived.stealth),
    );
    row(
        format!("Hits:{:>6}/{}", d.hp_current, d.hp_max),
        format!("Armour Class:{:>4}/{}", d.armour_class, d.armour_max),
        format!("Thievery:{:>9}", d.derived.thievery),
    );
    row(
        String::new(),
        String::new(),
        format!("Traps:{:>12}", d.derived.find_traps),
    );
    row(
        String::new(),
        String::new(),
        format!("Picklocks:{:>8}", d.derived.picklocks),
    );
    row(
        format!("Strength:{:>4}", d.stats.strength),
        format!("Agility:{:>3}", d.stats.agility),
        format!("Tracking:{:>9}", d.derived.tracking),
    );
    row(
        format!("Intellect:{:>3}", d.stats.intellect),
        format!("Health:{:>4}", d.stats.health),
        format!("Martial Arts:{:>5}", d.derived.dodge),
    );
    row(
        format!("Willpower:{:>3}", d.stats.wisdom),
        format!("Charm:{:>5}", d.stats.charm),
        format!("MagicRes:{:>9}", d.derived.magic_resist),
    );
    out
}
