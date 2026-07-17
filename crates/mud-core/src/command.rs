//! The in-game command parser.
//!
//! Model (oracle-calibrated, `oracle_ambiguity2.raw`): every verb carries a
//! **minimum abbreviation length**; the input's first word matches a verb iff
//! it is a prefix of the verb's name AND at least that minimum long. Entries
//! are tried in table order (first match wins — `hel` hits help before
//! health). Anything that matches nothing — too-short prefix, unknown word,
//! or an argument the handler can't resolve — falls through to SAY, the
//! parser's universal fallback. Verified pairs: q→quit, exp/ex, exi→exits,
//! st→status, he→health vs hel→help, to→top, trai/tra, g→get.
//!
//! Argument-taking verbs that require an argument print a `Syntax: VERB
//! {...}` line when given none (oracle: `ai` → "Syntax: AID {user name}");
//! attack is the exception — bare attack auto-picks a target.

use crate::content::Direction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Move(Direction),
    Look,
    Exits,
    Status,
    Experience,
    Health,
    /// `spells` — list the learned spellbook.
    Spells,
    Help,
    Top,
    Train,
    /// `attack [target]` — empty target means auto-pick.
    Attack(String),
    /// `cast [spell [target]]` — bare form prints the syntax line; a cast
    /// NEVER auto-picks a target (spellcasting.md §8.9).
    Cast(String),
    /// `aid <player>` — stabilize a downed player.
    Aid(String),
    /// `get <item or coins>`.
    Get(String),
    /// `drop <item>`.
    Drop(String),
    Inventory,
    /// `arm`/`wield`/`equip <weapon>`.
    Arm(String),
    /// `wear <armor>`.
    Wear(String),
    /// `remove <worn armor>`.
    Remove(String),
    /// `use <item>` — LearnSp scrolls today; charged items later.
    Use(String),
    /// `read <item>` — like `use`, plus the unowned-item description path.
    Read(String),
    /// Shop commands.
    List,
    Buy(String),
    Sell(String),
    Deposit(String),
    Withdraw(String),
    Balance,
    Quit,
    Blank,
    Unknown(String),
}

/// What an argument-taking handler did with its input. `FallThrough` makes
/// the dispatcher say the raw line (the parser's universal fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Handled,
    FallThrough,
}

/// Exact-match aliases, checked before the verb table (single/double letter
/// shortcuts that must not be shadowed by prefix matching).
const ALIASES: [(&str, Command); 12] = [
    ("n", Command::Move(Direction::North)),
    ("s", Command::Move(Direction::South)),
    ("e", Command::Move(Direction::East)),
    ("w", Command::Move(Direction::West)),
    ("ne", Command::Move(Direction::NorthEast)),
    ("nw", Command::Move(Direction::NorthWest)),
    ("se", Command::Move(Direction::SouthEast)),
    ("sw", Command::Move(Direction::SouthWest)),
    ("u", Command::Move(Direction::Up)),
    ("d", Command::Move(Direction::Down)),
    ("l", Command::Look),
    ("x", Command::Quit),
];

/// Verb constructors for the table.
#[derive(Clone, Copy)]
enum Verb {
    Plain(fn() -> Command),
    /// Takes the rest of the line as an argument (may be empty).
    WithArgs(fn(String) -> Command),
}

/// (name, minimum abbreviation length, constructor) in match order.
/// All direction minimums are ORACLE-verified (oracle_directions.raw):
/// north/south/west = full word, east = 3 (eat blocks 2), down = 3,
/// up = 2, diagonals = 6. Hand-authored asymmetry is the original's.
const VERBS: [(&str, usize, Verb); 39] = [
    ("north", 5, Verb::Plain(|| Command::Move(Direction::North))),
    ("south", 5, Verb::Plain(|| Command::Move(Direction::South))),
    ("east", 3, Verb::Plain(|| Command::Move(Direction::East))),
    ("west", 4, Verb::Plain(|| Command::Move(Direction::West))),
    ("northeast", 6, Verb::Plain(|| Command::Move(Direction::NorthEast))),
    ("northwest", 6, Verb::Plain(|| Command::Move(Direction::NorthWest))),
    ("southeast", 6, Verb::Plain(|| Command::Move(Direction::SouthEast))),
    ("southwest", 6, Verb::Plain(|| Command::Move(Direction::SouthWest))),
    ("up", 2, Verb::Plain(|| Command::Move(Direction::Up))),
    ("down", 3, Verb::Plain(|| Command::Move(Direction::Down))),
    ("attack", 1, Verb::WithArgs(Command::Attack)), // ORACLE: a/at/att
    // Min 1 like attack, so `c` and `c args` both cast (MEASURED §8.9).
    // ORACLE-VERIFY: only c/cast measured; ca/cas assumed by prefix model.
    ("cast", 1, Verb::WithArgs(Command::Cast)),
    ("aid", 2, Verb::WithArgs(Command::Aid)),       // ORACLE: ai
    ("get", 1, Verb::WithArgs(Command::Get)),       // ORACLE: g/ge/get
    ("drop", 2, Verb::WithArgs(Command::Drop)),
    ("inventory", 1, Verb::Plain(|| Command::Inventory)), // ORACLE: i
    ("arm", 2, Verb::WithArgs(Command::Arm)),       // ORACLE: ar arms
    ("wield", 3, Verb::WithArgs(Command::Arm)),     // ORACLE: wi says, wie arms
    ("equip", 2, Verb::WithArgs(Command::Arm)),     // ORACLE: eq
    ("wear", 3, Verb::WithArgs(Command::Wear)),     // min 3: "we" says (oracle)
    ("remove", 3, Verb::WithArgs(Command::Remove)),
    // ORACLE-VERIFY min abbrev: unmeasured; "u" is the up alias, so 2.
    ("use", 2, Verb::WithArgs(Command::Use)),
    // ORACLE-VERIFY min abbrev: unmeasured; 2 cannot shadow remove's
    // oracle minimum of 3 ("rem" is not a prefix of "read").
    ("read", 2, Verb::WithArgs(Command::Read)),
    ("list", 2, Verb::Plain(|| Command::List)),
    ("buy", 2, Verb::WithArgs(Command::Buy)),
    ("sell", 3, Verb::WithArgs(Command::Sell)),
    ("deposit", 3, Verb::WithArgs(Command::Deposit)),
    ("withdraw", 4, Verb::WithArgs(Command::Withdraw)),
    ("balance", 3, Verb::Plain(|| Command::Balance)),
    ("look", 2, Verb::Plain(|| Command::Look)),     // ORACLE: lo
    ("exits", 3, Verb::Plain(|| Command::Exits)),   // ORACLE: exi (ex says)
    ("experience", 3, Verb::Plain(|| Command::Experience)), // ORACLE: exp
    ("status", 2, Verb::Plain(|| Command::Status)), // ORACLE: st/sta/stat
    ("help", 3, Verb::Plain(|| Command::Help)),     // ORACLE: hel (before health)
    ("health", 2, Verb::Plain(|| Command::Health)), // ORACLE: he
    // ORACLE-VERIFY min abbrev: unmeasured; 2 is unambiguous ("s" is the
    // south alias, "st" hits status first).
    ("spells", 2, Verb::Plain(|| Command::Spells)),
    ("top", 2, Verb::Plain(|| Command::Top)),       // ORACLE: to (t says)
    ("train", 4, Verb::Plain(|| Command::Train)),   // ORACLE: trai (tra says)
    ("quit", 1, Verb::Plain(|| Command::Quit)),     // ORACLE: q
];

pub fn parse(input: &str) -> Command {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Command::Blank;
    }
    let verb = trimmed
        .split_whitespace()
        .next()
        .expect("non-empty after trim")
        .to_ascii_lowercase();

    for (alias, command) in &ALIASES {
        if verb == *alias {
            return command.clone();
        }
    }
    for (name, min, kind) in &VERBS {
        if verb.len() >= *min && name.starts_with(&verb) {
            return match kind {
                Verb::Plain(make) => make(),
                Verb::WithArgs(make) => make(trimmed[verb.len()..].trim().to_string()),
            };
        }
    }
    Command::Unknown(trimmed.to_string())
}
