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

/// Display name used in the exits list and arrival broadcasts. Up/down show
/// as "above"/"below" (VERIFIED: DLL exit string table).
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
        Direction::Up => "above",
        Direction::Down => "below",
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

/// VERIFIED (DLL) format string; ORACLE-VERIFY for vertical arrivals
/// ("arrived from the above/below" is presumed).
pub fn arrived_from(name: &str, direction: Direction) -> String {
    format!("{name} just arrived from the {}.", direction_shown(direction))
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
