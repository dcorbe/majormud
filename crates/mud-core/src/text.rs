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
    format!("{}You say \"{what}\"{}", color::GREEN, color::RESET)
}

/// VERIFIED (DLL): what the rest of the room hears.
pub fn says(name: &str, what: &str) -> String {
    format!("{}{name} says \"{what}\"{}", color::GREEN, color::RESET)
}

/// VERIFIED (DLL): moving where no exit exists.
pub const NO_EXIT: &str = "There is no exit in that direction!";

/// VERIFIED (DLL): the exits-line prefix and empty-exits marker.
pub const OBVIOUS_EXITS: &str = "Obvious exits: ";
pub const NO_EXITS: &str = "NONE!!!";

/// VERIFIED (DLL): the occupant-line prefix.
pub const ALSO_HERE: &str = "Also here: ";

use crate::content::Direction;

/// The stock palette (single scheme, hardcoded in the DLL's strings —
/// dumped from the oracle raws 2026-07-19; the only user knob is the
/// MBBS ANSI on/off setting, `CoreConfig::ansi`).
pub mod color {
    /// Room name (`1;36` bright cyan).
    pub const ROOM_NAME: &str = "\x1b[1;36m";
    /// Body text / the prompt frame (`0;37` white).
    pub const PLAIN: &str = "\x1b[0;37m";
    /// Obvious exits + says (`0;32` green).
    pub const GREEN: &str = "\x1b[0;32m";
    /// The Also-here line (`0;35` magenta) ...
    pub const ALSO: &str = "\x1b[0;35m";
    /// ... with names in `1;35` bright magenta.
    pub const ALSO_NAME: &str = "\x1b[1;35m";
    /// "You notice" floor line (`0;36` cyan).
    pub const NOTICE: &str = "\x1b[0;36m";
    /// Incoming monster attack/miss lines (`0;36` cyan).
    pub const INCOMING: &str = "\x1b[0;36m";
    /// Damage lines, both directions, and the low-HP prompt number
    /// (`1;31` bright red).
    pub const DAMAGE: &str = "\x1b[1;31m";
    /// Your swings that never CONNECTED — the plain miss and the defender's
    /// parry (`0;36` cyan, the same family as the incoming monster lines).
    ///
    /// MEASURED (`oracle_dodge_parry_{control,acc-mid,acc-high}.raw`): 24
    /// plain misses and 46 parries, all `0;36`, against 36 glances all
    /// `0;31`. This constant used to be one "miss/glance family" at `0;31`,
    /// which painted two thirds of it the wrong colour.
    pub const YOUR_MISS: &str = "\x1b[0;36m";
    /// Your swing that connected and was soaked by armour (`0;31` red).
    pub const YOUR_GLANCE: &str = "\x1b[0;31m";
    /// *Combat Engaged*/*Combat Off* (`0;33` yellow).
    pub const COMBAT_MARK: &str = "\x1b[0;33m";
    /// A mover's name in movement/arrival lines (`1;33` bright yellow).
    pub const MOVE_NAME: &str = "\x1b[1;33m";
    /// The adjacent-room rumble (`0;35` magenta).
    pub const RUMBLE: &str = "\x1b[0;35m";
    pub const RESET: &str = "\x1b[0m";
}

/// ORACLE-VERIFY: forced removal when gear becomes alignment-illegal
/// (update_allowed_worn_items, crime.md §2.4 — wording from the M4
/// deferral note, unmeasured).
pub fn item_force_removed(name: &str) -> String {
    format!("Your {name} has been removed.")
}

/// SEARCH refusal for a non-direction argument (theft.md §9).
pub const SEARCH_WHY: &str = "Why would you want to search that?";

/// PICKLOCK strings (theft.md §8, verbatim).
pub const SYNTAX_PICKLOCK: &str = "Syntax: PICKLOCK {direction}";
pub const PICK_FAILS: &str = "Your skill fails you this time.";
/// ORACLE-VERIFY: walking into a locked type-2 door (the open-door
/// command family is unmodeled; wording guessed).
pub const DOOR_CLOSED: &str = "The door is closed!";

/// ROB / FORGIVE strings (theft.md §3-5, verbatim).
pub const SYNTAX_ROB: &str = "Syntax: ROB {user/monster}";
pub const DONT_SEE_ANYWHERE: &str = "You don't see that anywhere!";
pub const ROB_FROM_THAT: &str = "Why would you want to rob from that?";
pub const ROB_WAY_OF_LIFE: &str =
    "You have chosen a way of life which prevents this action.";
pub const ROB_YOURSELF: &str = "Why would you want to rob yourself?";
pub const ROB_UNBALANCED: &str =
    "Such an action would result in a very unbalanced game.";
pub const ROB_GUILT: &str =
    "You are overcome with a feeling of guilt and return your hands to your own pockets";

/// Gendered pronouns (`+0x7d6`; FUN_0041d89d/8dd/91d).
pub fn pronoun_subject(gender: crate::game::Gender) -> &'static str {
    match gender {
        crate::game::Gender::Male => "he",
        crate::game::Gender::Female => "she",
    }
}
pub fn pronoun_object(gender: crate::game::Gender) -> &'static str {
    match gender {
        crate::game::Gender::Male => "him",
        crate::game::Gender::Female => "her",
    }
}
pub fn pronoun_possessive(gender: crate::game::Gender) -> &'static str {
    match gender {
        crate::game::Gender::Male => "his",
        crate::game::Gender::Female => "her",
    }
}

/// The five currency display names (table 0x480248; runic's "User
/// Defined" placeholder is board-configured — ORACLE-VERIFY the live
/// board's name).
pub fn currency_name(idx: usize) -> &'static str {
    ["copper farthings", "silver nobles", "gold crowns", "platinum pieces", "runic coins"]
        [idx.min(4)]
}

/// cmd_backstab (0x4889da): a wielded weapon without BSAccu.
pub const CANNOT_BACKSTAB_WEAPON: &str = "You cannot backstab with this weapon!";

/// HIDE <item> refusal for NotDroppable gear (theft.md §11.2).
pub const MAY_NOT_HIDE_ITEM: &str = "You may not hide that item!";

/// The command-delay gate (theft.md §11, [plain]).
pub const MUST_WAIT: &str = "You must wait before you may do that!";

/// SNEAK refusal while being fought (theft.md §11.1).
pub const MAY_NOT_SNEAK: &str = "You may not sneak right now!";

/// Sneak movement lines (theft.md §11.1, perception-filtered, dkyellow).
pub fn sneak_out(name: &str, direction: Direction) -> String {
    let tail = match direction {
        Direction::Up => "sneaking out upwards".to_string(),
        Direction::Down => "sneaking out downwards".to_string(),
        d => format!("sneaking out to the {}", direction_shown(d)),
    };
    format!("You notice {name} {tail}.")
}

pub fn sneak_in_from(name: &str, from: Direction) -> String {
    let tail = match from {
        Direction::Up => "sneak in from above".to_string(),
        Direction::Down => "sneak in from below".to_string(),
        d => format!("sneak in from the {}", direction_shown(d)),
    };
    format!("You notice {name} {tail}.")
}

/// Alignment-restricted exits (crime.md §3, 0x47e31e/0x47e349).
pub const EXIT_TOO_GOOD: &str = "You are too good to go through this exit!";
pub const EXIT_TOO_EVIL: &str = "You are too evil to go through this exit!";

/// SET EVIL (cmd_set 54203-54212). Both confirms read verbatim out of
/// the shipped DLL — they sit adjacent in the string table, OFF at
/// 0xd76f1 and ON at 0xd772e. An earlier pass had ON as "...stopped from
/// performing evil actions" and OFF as "You will no longer be warned
/// before...", neither of which the board ever prints; anything matching
/// on this wording (the client's evil-warning toggle does) would have
/// missed.
pub const SET_EVIL_WARN_ON: &str =
    "You will now be warned and stopped from doing most evil actions.";
pub const SET_EVIL_WARN_OFF: &str =
    "You will no longer be stopped from performing evil actions.";

/// OURS (divergence — the real board keys ANSI on the MBBS account):
/// the `ansi` toggle's confirmations.
pub const ANSI_NOW_ON: &str = "ANSI colour is now ON.";
pub const ANSI_NOW_OFF: &str = "ANSI colour is now OFF.";

/// `get_random_name` (0x424172): the spawn-adjective walk over a name
/// block. Per line a candidate composes — `A:` base sep suffix, `B:`
/// prefix sep base, `F:` full replace, `N:` base, anything else the line
/// verbatim — then `genrdn(0,100)` accepts on <= 9; running off the block
/// (or the 1000-line ceiling) keeps the LAST candidate. Separator " "
/// (`DAT_00480efa`), strncpy/strncat caps 28/29 bytes, empty final
/// candidate falls back to the base name. A trailing newline's empty tail
/// is the buffer terminator, not a line; interior empty lines compose an
/// empty candidate (the DLL's verbatim branch).
pub fn generate_name(base: &str, block: &str, roll: &mut dyn FnMut(i32, i32) -> i32) -> String {
    fn take(s: &str, n: usize) -> &str {
        if s.len() <= n {
            return s;
        }
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
    let mut pieces: Vec<&str> = block.split('\n').collect();
    if pieces.last() == Some(&"") {
        pieces.pop();
    }
    let mut candidate = String::new();
    for (i, line) in pieces.iter().enumerate() {
        if i >= 1000 {
            break;
        }
        let bytes = line.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let tail = &line[2..];
            match bytes[0] {
                b'A' => {
                    candidate = format!("{base} ");
                    let room = 0x1dusize.saturating_sub(candidate.len());
                    candidate.push_str(take(tail, room));
                }
                b'B' => {
                    candidate = format!("{} ", take(tail, 0x1c));
                    let room = 0x1dusize.saturating_sub(candidate.len());
                    candidate.push_str(take(base, room));
                }
                b'F' => candidate = take(tail, 0x1d).to_string(),
                b'N' => candidate = take(base, 0x1d).to_string(),
                _ => candidate = take(line, 0x1d).to_string(),
            }
        } else {
            candidate = take(line, 0x1d).to_string();
        }
        if roll(0, 100) <= 9 {
            break;
        }
    }
    if candidate.is_empty() {
        base.to_string()
    } else {
        candidate
    }
}

/// Removes every ANSI escape sequence (the MBBS non-graphics path).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC [ params final-byte
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// Display name used in the exits list. Vertical exits show as "up"/"down"
/// (USER TESTIMONY — the earlier "above"/"below" reading of the DLL string
/// table was wrong; that pair belongs to the vertical arrival broadcasts,
/// see [`walks_in_from`]).
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
    let tail = match direction {
        Direction::Up => "just left upwards.".to_string(),
        Direction::Down => "just left downwards.".to_string(),
        d => format!("just left to the {}.", direction_shown(d)),
    };
    format!("{}{name}{} {tail}{}", color::MOVE_NAME, color::GREEN, color::RESET)
}

/// VERIFIED (oracle 2026-07-18): walk-arrival broadcast, players and
/// monsters alike — "Kaimon walks into the room from the east." /
/// "Oracle walks into the room from above." The DLL's "just arrived from"
/// string never appears for walks in any capture; it is the
/// generate_monster spawn-arrival flavour line (monsters.md §2 step 7,
/// landed with the M6 spawner).
pub fn walks_in_from(name: &str, direction: Direction) -> String {
    let tail = match direction {
        Direction::Up => "walks into the room from above.".to_string(),
        Direction::Down => "walks into the room from below.".to_string(),
        d => format!("walks into the room from the {}.", direction_shown(d)),
    };
    format!("{}{name}{} {tail}{}", color::MOVE_NAME, color::GREEN, color::RESET)
}

/// VERIFIED (oracle + DLL 0x48105a "%s%s moves into the room from the %s."):
/// wander-arrival broadcast — bare instance name, e.g. "happy guardsman
/// moves into the room from the east." (the format's second %s is an
/// invisible ANSI junk sequence). Distinct from the spawn/player arrival
/// "walks into the room from" line.
pub fn monster_moves_in_from(name: &str, direction: Direction) -> String {
    let tail = match direction {
        Direction::Up => "moves into the room from above.".to_string(),
        Direction::Down => "moves into the room from below.".to_string(),
        d => format!("moves into the room from the {}.", direction_shown(d)),
    };
    format!("{}{name}{} {tail}{}", color::MOVE_NAME, color::GREEN, color::RESET)
}

/// VERIFIED (DLL 0x481399): the confusion fumble line,
/// check_monster_confusion — bare instance name.
pub fn monster_confused_fumble(name: &str) -> String {
    format!("{name} looks around stupidly and foams at the mouth!")
}

/// VERIFIED (DLL 0x481081/0x4810a2): the default spawn-arrival line
/// (generate_monster, template `movemsg` 0) — bare instance name; `None`
/// direction = no qualifying plain exit.
pub fn spawn_arrived(name: &str, from: Option<Direction>) -> String {
    let tail = match from {
        None => "just arrived from nowhere.".to_string(),
        Some(d) => format!("just arrived from the {}.", direction_shown(d)),
    };
    format!("{}{name}{} {tail}{}", color::MOVE_NAME, color::GREEN, color::RESET)
}

/// VERIFIED (DLL 0x483e02/20/3e): the adjacent-room spawn rumble
/// (display_entry_movement) — `direction` is as seen FROM the adjacent
/// room (the reverse-direction table).
pub fn hear_movement(direction: Direction) -> String {
    let tail = match direction {
        Direction::Up => "You hear movement above you!".to_string(),
        Direction::Down => "You hear movement below you!".to_string(),
        d => format!("You hear movement to the {}.", direction_shown(d)),
    };
    format!("{}{tail}{}", color::RUMBLE, color::RESET)
}

/// VERIFIED (DLL 0x4810d8 family): the killer-visible coin drops in
/// check_kill_monster — "%s platinum drop to the ground." etc.; runic
/// uses the configured currency name. Amount slot is the plain number.
pub fn coins_drop(amount: u32, denom: &str) -> String {
    format!("{amount} {denom} drop to the ground.")
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

/// VERIFIED (oracle): the health command line. The mana clause appears
/// whenever the max pool is non-zero (show_health 0x34af7 gates on
/// `+0x600 != 0`), captioned `Kai:` for caster group 5 and `Mana:`
/// otherwise — MEASURED §8.2 (mage) / §8.12 (mystic, absent at L1 where
/// max kai is 0).
pub fn health_line(current: i32, max: i32, mana: i32, mana_max: i32, caster_group: i16) -> String {
    let percent = if max == 0 { 0 } else { current * 100 / max };
    let mut line = format!("Health:{current:>6}/{max:<6}[{percent}%]");
    if mana_max != 0 {
        let caption = if caster_group == 5 { "Kai:" } else { "Mana:" };
        let mana_percent = mana * 100 / mana_max;
        line.push_str(&format!("  {caption}{mana:>4}/{mana_max:<4}[{mana_percent}%]"));
    }
    line
}

/// VERIFIED (oracle/DLL): training messages.
pub const TRAIN_WRONG_ROOM: &str = "You must be in an appropriate training room to train!";
pub const TRAIN_NO_EXP: &str = "You do not have the required experience to train yet!";
pub const TRAIN_NO_MONEY: &str = "You do not have the money required for your training.";

/// VERIFIED (§8.7/§8.12 receipts): the payment sentence lists the coins
/// actually handed over ("5 silver nobles" mage, "50 copper farthings"
/// mystic — the deduct_currency change-making, same as buy).
pub fn train_hand_over(coins: &str, level: u16) -> String {
    format!("You hand over {coins} and you receive training to attain level {level}.")
}
/// VERIFIED (oracle_spell_train.raw / §8.12): the receipt header.
pub const TRAIN_RECEIVE_HEADER: &str = "You receive the following:";
/// VERIFIED (oracle_spell_train.raw / §8.12): the CP line.
pub fn train_cp_line(cp: u16) -> String {
    format!("{cp} additional character points")
}
/// VERIFIED (§8.12): the kai grant header, after the CP line; one power
/// name per line follows.
pub const KAI_LEARN_HEADER: &str = "You learn the following Kai abilities:";

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
/// VERIFIED (spellcasting.md §8.9): bare `cast`/`c`.
pub const SYNTAX_CAST: &str = "Syntax: CAST {spell} [{target}]";

/// VERIFIED (spellcasting.md §8.6): the cast argument that resolved to no
/// learned spell, echoed verbatim.
pub fn dont_know_cast(arg: &str) -> String {
    format!("You do not know how to cast {arg}.")
}

/// VERIFIED (spellcasting.md §8.6): one cast per combat round.
pub const ALREADY_CAST: &str = "You have already cast a spell this round!";
/// VERIFIED (spellcasting.md §8.6): the mana gate.
pub const NOT_ENOUGH_MANA: &str = "You do not have enough mana to cast that spell.";

// --- kai/mystic wording (VERIFIED oracle_kai_mystic*.raw; spellcasting.md
// §8.12) — the caster_group-5 variants of the cast surfaces. ---

/// VERIFIED (§8.12): the mystic `cast` hard refusal — before any argument
/// parsing; note the double space after "KAI!".
pub const KAI_NO_CAST: &str = "You may not cast... You are KAI!  You must invoke your powers.";
/// VERIFIED (§8.12): the mystic `spells` redirect (single space there).
pub const KAI_NO_SPELLS: &str =
    "You may not list your spells. You are KAI! You must list your powers.";
/// ORACLE-VERIFY: a non-kai `powers` is unmeasured — the parallel of the
/// kai `spells` redirect, chosen by symmetry.
pub const NON_KAI_NO_POWERS: &str =
    "You may not list your powers. You are not KAI! You must list your spells.";
/// ORACLE-VERIFY: a non-kai `invoke` is unmeasured — the parallel of the
/// kai `cast` refusal, chosen by symmetry (double space kept).
pub const NON_KAI_NO_INVOKE: &str =
    "You may not invoke... You are not KAI!  You must cast your spells.";
/// VERIFIED (§8.12): bare invoke.
pub const SYNTAX_INVOKE: &str = "Syntax: INVOKE {power} [{target}]";
/// VERIFIED (§8.12): the kai-specific mana gate wording.
pub const NOT_ENOUGH_KAI: &str = "You do not have enough kai to invoke that power.";
/// VERIFIED (§8.12): the one-per-round flag, invoke wording — checked
/// with kai still in the pool, charges nothing.
pub const ALREADY_INVOKED: &str = "You have already invoked a power this round!";
/// VERIFIED (§8.12): the empty `powers` reply.
pub const NO_POWERS: &str = "You have no powers.";
/// VERIFIED (§8.12): the `powers` header pair — Mana becomes Kai, "Spell
/// Name" stays.
pub const POWERS_HEADER: &str = "You have the following powers:\nLevel Kai  Short Spell Name";

/// VERIFIED (§8.12, byte-exact): one `powers` row. Unlike `spell_row`'s
/// left-aligned 6-wide short column, the kai short is RIGHT-aligned width
/// 4 with a two-space gutter (visible on `owl`; invisible in §8.5 where
/// every mage shortname is exactly 4 chars).
pub fn power_row(level: i16, kai: i16, short: &str, name: &str) -> String {
    format!("{level:>3}{kai:>4}    {short:>4}  {name:<30}")
}
/// DLL string (spec §2 level gate). ORACLE-VERIFY: unreachable via
/// scroll-learned books, so never observed live; reachable via slice-4
/// temp spells.
pub const SPELL_TOO_POWERFUL: &str = "This spell is too powerful for you.";

/// VERIFIED (DLL strings dump 3146): the quest VM `learnspell` verb's
/// class-gate refusal (FUN_0046fff6 68459/68469).
pub const LEARNSPELL_CANT: &str = "You don't know what to do with this!";

/// VERIFIED (DLL string `s_A_concealed_passage_opens_to_the`, 66067):
/// the remoteaction lever-reveal broadcast, `%s` = the direction.
pub fn concealed_passage_opens(direction: &str) -> String {
    format!("A concealed passage opens to the {direction}!")
}

/// VERIFIED (DLL strings dump 3147): the `learnspell` success line
/// (68477; `%s` = the spell's long name).
pub fn learn_spell(name: &str) -> String {
    format!("You learn the spell {name}.")
}

/// VERIFIED (oracle §8.6/§8.9): the caster's failed success-roll line.
pub fn cast_fail(spell: &str) -> String {
    format!("You attempt to cast {spell}, but fail.")
}

/// DLL string 00485be0 ("%s attempted to cast %s, but failed."), emitted to
/// the room beside the caster's fail line (decompiled 39015/39375/43647).
/// ORACLE-VERIFY: the template is DLL-exact, but single-session captures
/// cannot show the observer side live.
pub fn cast_fail_room(caster: &str, spell: &str) -> String {
    format!("{caster} attempted to cast {spell}, but failed.")
}

/// VERIFIED (spellcasting.md §8.9): bare offensive cast with no resolvable
/// bare-cast form — empty room or monsters-only room alike.
pub const MUST_SPECIFY_TARGET: &str = "You must specify a target for that spell!";

/// VERIFIED (§8.13): a benign single-target cast at a monster (`c blur
/// cat`) and an area cast with an explicit monster word (`c stnk cat`)
/// both refuse with this, uncharged.
pub const MAY_NOT_CAST_ON_MONSTER: &str = "You may not cast that spell on a monster!";

/// VERIFIED (§8.13): an area cast with an explicit player word
/// (`c flash oracle`), uncharged.
pub const MAY_NOT_CAST_ON_USER: &str = "You may not cast that spell on a user!";

/// The third member of the same refusal family (`cast_item_target`
/// 44367-44369): a spell whose match type is not 6 or 7, aimed at a
/// carried item. ORACLE-VERIFY: never measured live — the wording is
/// read straight out of the DLL string table (file offset 854240,
/// exactly 362 bytes past the monster variant, matching the
/// `0x48632c - 0x4861c2` VA delta).
pub const MAY_NOT_CAST_ON_ITEM: &str = "You may not cast that spell on an item!";

/// VERIFIED (§8.13): an area cast with no valid target in the room —
/// a real pre-charge gate (mana unchanged), fired alone AND with other
/// players present (players never count as area targets).
pub const SPELL_NO_EFFECT_IN_ROOM: &str = "Your spell has no effect in this room!";

/// VERIFIED (§8.13): the caster's failed-roll line for a TARGETED cast
/// ("at Oracle" — the target-less form is [`cast_fail`]).
pub fn cast_fail_at(spell: &str, target: &str) -> String {
    format!("You attempt to cast {spell} at {target}, but fail.")
}

/// VERIFIED (§8.13): the room line beside [`cast_fail_at`]. The TARGET
/// sees neither — a failed attempt is invisible to its victim.
pub fn cast_fail_at_room(caster: &str, spell: &str, target: &str) -> String {
    format!("{caster} attempted to cast {spell} at {target}, but failed.")
}

/// The target's own line of the resist family (spec §3 "You resisted
/// %s's %s"; caster/room siblings [`cast_resisted`]/[`cast_resisted_room`]).
/// ORACLE-VERIFY: no learnable benign spell carries a save class, so the
/// player-target form was never measurable live.
pub fn you_resisted(caster: &str, spell: &str) -> String {
    format!("You resisted {caster}'s {spell}.")
}

/// VERIFIED (spellcasting.md §8.6/§8.9): bare offensive cast in a
/// protected room (room `attributes & 1` — the Newhaven shops).
pub const CAST_GUILT: &str =
    "You are overcome with a feeling of guilt and break off your attack.";

// --- item-target casts (cast_item_target 0x49232, DetectMagic case 0x1a
// --- 44620-44667). All decompile-only — ORACLE-VERIFY (detect magic IS
// --- learnable live: scroll 121, Newhaven Mage Spell Shop). Banding is on
// --- the ITEM's Magical(28) value: 1 / 2-3 / 4-5 / 6+ / absent.
pub fn glows_faintly(item: &str) -> String {
    format!("{item} glows faintly, indicating a small amount of magic within.")
}
pub fn glows_softly(item: &str) -> String {
    format!("{item} glows softly, indicating a good amount of magic within.")
}
pub fn glows_brightly(item: &str) -> String {
    format!("{item} glows brightly, indicating a large amount of magic within.")
}
pub fn blinding_aura(item: &str) -> String {
    format!(
        "You are almost blinded by the aura from {item}, indicating immense magical properties!"
    )
}
pub const NO_MAGIC_IN_ITEM: &str = "You detect no magic in that item!";
/// DLL 0x4864e4 ("%s casts %s on %s.") — the item-cast room line (note the
/// PERIOD; the castmsgb room lines end in bangs).
pub fn casts_spell_on(caster: &str, spell: &str, target: &str) -> String {
    format!("{caster} casts {spell} on {target}.")
}

/// DLL string 00485de3 ("Your spell has no effect on %s.") — the SpellImmu
/// (139) refusal on a monster target (decompile cast_monster_target
/// 43630-43638). ORACLE-VERIFY: no starter spell/monster pair reaches it.
pub fn spell_no_effect_on(target: &str) -> String {
    format!("Your spell has no effect on {target}.")
}

/// DLL string 00485fe3 ("You attempt to cast %s at %s, but the spell is
/// resisted.") — the caster line when the monster's saving throw succeeds
/// (decompile cast_monster_target 44234-44236). ORACLE-VERIFY: the starter
/// spells are all SaveClass::None, so this is unreachable live for now.
pub fn cast_resisted(spell: &str, target: &str) -> String {
    format!("You attempt to cast {spell} at {target}, but the spell is resisted.")
}

/// DLL string 00486034 ("%s resisted %s's %s.") — the room line beside
/// [`cast_resisted`] (decompile 44240). ORACLE-VERIFY as above.
pub fn cast_resisted_room(target: &str, caster: &str, spell: &str) -> String {
    format!("{target} resisted {caster}'s {spell}.")
}

// --- monster casts (monster_cast 0x27cc3; spellcasting.md §6). The DLL
// --- prefixes each with an ANSI prompt-redraw blob (DAT_004812d1), not
// --- part of the message text. The HIT fan-out is MEASURED (§8.14:
// --- castmsgb victim line WITH damage / room line per the record,
// --- typically without — moaning spirit 82, dragonfish 359); the
// --- resist/fizzle families and the record-less default pair below
// --- remain decompile-extracted, read from the binary at their cited
// --- addresses (every §8.14-reachable caster carried a 100% form and
// --- never rolled a resist).

/// DLL 004812fb ("You resisted %s's cast of %s.") — the victim's line of
/// the monster-cast resist family (decompile monster_cast 23067-23068);
/// %s slots are the monster's instance name (no article) and the spell.
pub fn you_resisted_monster_cast(monster: &str, spell: &str) -> String {
    format!("You resisted {monster}'s cast of {spell}.")
}

/// DLL 0048131a ("%s resisted %s's cast of %s.") — the room line beside
/// [`you_resisted_monster_cast`] (23071-23074).
pub fn resisted_monster_cast_room(victim: &str, monster: &str, spell: &str) -> String {
    format!("{victim} resisted {monster}'s cast of {spell}.")
}

/// DLL 00481338 ("The %s attempted to cast %s at you, but failed.") — the
/// victim's line when the monster's cast-chance roll fails (23749-23752):
/// a monster fizzle is NOT silent, unlike a player's out-of-mana round.
pub fn monster_cast_fizzle(monster: &str, spell: &str) -> String {
    format!("The {monster} attempted to cast {spell} at you, but failed.")
}

/// DLL 00481369 ("The %s attempted to cast %s at %s, but failed.") — the
/// room line beside [`monster_cast_fizzle`] (23753-23757).
pub fn monster_cast_fizzle_room(monster: &str, spell: &str, victim: &str) -> String {
    format!("The {monster} attempted to cast {spell} at {victim}, but failed.")
}

/// DLL 00481277 ("%s cast %s on you.") — monster_display_spell_success's
/// victim-line fallback when the spell has no castmsgb record (21687-21689).
/// Note the PERIOD: the player-side default twins (00485978) end likewise.
pub fn monster_cast_default(monster: &str, spell: &str) -> String {
    format!("{monster} cast {spell} on you.")
}

/// DLL 0048128a ("%s cast %s on %s.") — the room-line fallback beside
/// [`monster_cast_default`].
pub fn monster_cast_default_room(monster: &str, spell: &str, victim: &str) -> String {
    format!("{monster} cast {spell} on {victim}.")
}

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

/// DLL string 0xbd28d (" and your poisoning is cured.") — the healer's
/// poisoned curing purchase. Trailing PERIOD, unlike [`not_poisoned`]'s
/// bang. ORACLE-VERIFY: still no capture of THIS string — §8.14's live
/// poisoned cure went through the Silvermere Temple healer, a TEXTBLOCK
/// service (10 gold, "The healer casts cure poison on you!"), not the
/// healer-shop path this string belongs to.
pub fn poisoning_cured(coins: &str) -> String {
    format!("You hand over {coins} and your poisoning is cured.")
}

/// VERIFIED (§8.14, patched-counter lifecycle): the slow-tick poison line
/// (`regeneration.md` §4, decompile 19518-19524; DLL string 0xc775d) —
/// measured live at counter 5: the line + counter damage + regen in the
/// SAME tick (net -4), zero ticks after the healer cure.
pub const YOU_FEEL_ILL: &str = "You feel ill.";

/// A copper amount as coin words.
pub fn copper_amount(total: u64) -> String {
    let (one, many) = COIN_NAMES[0];
    format!("{total} {}", if total == 1 { one } else { many })
}

// --- combat strings (VERIFIED oracle_attack3.raw / oracle_downed.raw / DLL) ---
pub const COMBAT_ENGAGED: &str = "\x1b[0;33m*Combat Engaged*\x1b[0m";
pub const COMBAT_OFF: &str = "\x1b[0;33m*Combat Off*\x1b[0m";
pub const NO_TARGET: &str = "You don't see your target here.";
pub const MORTALLY_WOUNDED: &str = "You may not do that while you are mortally wounded!";

/// "You punch kobold thief for 1 damage!" — monster name without article.
pub fn player_hit(verb: &str, target: &str, damage: i32) -> String {
    format!("{}You {verb} {target} for {damage} damage!{}", color::DAMAGE, color::RESET)
}

/// "You swing at kobold thief!" — the verb is the weapon's miss verb.
/// This is result 0, the to-hit failure; result 3 has its own wording, see
/// [`player_dodge`].
pub fn player_miss(verb: &str, target: &str) -> String {
    format!("{}You {verb} {target}!{}", color::YOUR_MISS, color::RESET)
}

/// "You swing at kobold thief who dodges your attack!" — result 3, the
/// defender's parry.
///
/// MEASURED (`charm.md` §8.3, `re/oracle/oracle_dodge_parry_*.raw`): the
/// board words this apart from the plain miss above, which we used to
/// render for both outcomes. WCCMMUD.DLL carries the template verbatim at
/// file offset 0xca40d, `You %s %s who dodges your attack!` — one slot
/// after the plain miss `You %s %s!` (0xca3ea) and one before the
/// monster-side result-3 pair (0xca430 victim view, 0xca464 room view).
///
/// MEASURED (2026-07-26, the same raws): the colour DOES match the plain
/// miss — 46 parry lines and 24 plain misses all carry `0;36`. The earlier
/// note here said the capture was ANSI-stripped and the attribute bytes
/// unmeasurable; that was wrong, the raws carry ANSI throughout, and
/// reading them showed the plain miss had itself been painted with the
/// glance's red.
pub fn player_dodge(verb: &str, target: &str) -> String {
    format!(
        "{}You {verb} {target} who dodges your attack!{}",
        color::YOUR_MISS,
        color::RESET
    )
}

/// "Your swing at kobold thief hits, but glances off its armour."
pub fn player_glance(verb: &str, target: &str) -> String {
    format!(
        "{}Your {verb} {target} hits, but glances off its armour.{}",
        color::YOUR_GLANCE,
        color::RESET
    )
}

/// ORACLE-VERIFY: the critical variant was not captured.
pub fn player_crit(verb: &str, target: &str, damage: i32) -> String {
    format!(
        "{}You critically {verb} {target} for {damage} damage!{}",
        color::DAMAGE,
        color::RESET
    )
}

/// Fills a DB printf-style message template: each `%s`/`%d` consumes the
/// next argument in order (damage arrives pre-formatted — the DLL renders
/// the observer's damage through `get_damage_descriptor`, a `%d` sprintf,
/// and passes the result as a string). Slots beyond the argument list
/// render empty, exactly like the DLL's always-passed trailing `""`
/// arguments (decompile `attack_monster_user` 0x2e34b).
///
/// Sibling: [`render_cast_line`] does %-substitution for spell messages —
/// there overflow slots pass through UNCHANGED (its decompile path has no
/// trailing `""` args), so the two policies intentionally differ.
pub fn fill_message(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && matches!(chars.peek(), Some('s' | 'd')) {
            chars.next();
            out.push_str(args.get(next).copied().unwrap_or(""));
            next += 1;
        } else {
            out.push(c);
        }
    }
    out
}

// Generic monster-swing templates, used when an attack form lacks its
// message records. Four shipped 1.11p monsters have such record-less melee
// forms: healer (47, wielding item 64), zombie (492), and ju-ju zombie
// (493 and 772). Extracted verbatim from WCCMMUD.DLL seg 0x1140 (file base
// 0xc9c00; identical strings in the WG3-NT build, `attack_monster_user`
// 0x2e34b / 16-bit 1040:4f2a). The verb/weapon `%s` slots are filled from
// the wielded weapon's records (`move_monster_to_fighter` 1040:1739);
// unarmed monsters render them empty.

/// 1140:0x7b7 — record-less hit, victim view: (name, hit verb, damage).
pub const MONSTER_HIT_TPL: &str = "%s %s you for %d damage!";

/// 1140:0x7d1 — record-less hit, room view: (name, hit verb, victim,
/// damage descriptor — `get_damage_descriptor` renders the plain number).
pub const MONSTER_HIT_ROOM_TPL: &str = "%s %s %s for %s damage!";

/// 1140:0xf6b — record-less glance (result 1), victim view: (name, swing
/// verb).
pub const MONSTER_GLANCE_TPL: &str = "%s's %s hits you, but your armour deflects.";

/// 1140:0xf98 — record-less glance, room view: (name, swing verb — the
/// VICTIM-view slot, same as 0xf6b —, victim, possessive pronoun).
pub const MONSTER_GLANCE_ROOM_TPL: &str = "%s's %s hits %s, but glances off %s armour.";

/// 1140:0xfc5 — record-less parry (result 3), victim view: (name, swing
/// verb, weapon name).
pub const MONSTER_DODGE_TPL: &str = "%s %s you with %s, but you dodge!";

/// 1140:0xfe8 — record-less parry, room view: (name, swing verb, victim,
/// weapon name, subject pronoun).
pub const MONSTER_DODGE_ROOM_TPL: &str = "%s %s %s with its %s, but %s dodges.";

/// 1140:0x100e — record-less plain miss, victim view: (name, swing verb,
/// weapon name).
pub const MONSTER_MISS_TPL: &str = "%s %s you with %s.";

/// 1140:0x1022 — record-less plain miss, room view: (name, swing verb,
/// victim, weapon name).
pub const MONSTER_MISS_ROOM_TPL: &str = "%s %s %s with its %s.";

/// `attack_monster_user` runs `toupper` on the first byte of every
/// composed swing line (a no-op for record templates starting "The ...").
pub fn capitalize_first(mut line: String) -> String {
    if let Some(first) = line.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    line
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

// The five monster-vs-monster room lines (`attack_monster_monster`
// 27255-27328; charm.md §3). Every one is composed into the shared
// `DAT_004964a9` buffer, first byte upcased ([`capitalize_first`] at the
// call site), and `tell_room`'d — the survivor lines to the DEFENDER's
// room, the kill line to the ATTACKER's. VERIFIED against the shipped
// `.rdata` (0x481f73..0x481fe7): only the hit line ends in "!", and the
// glance line has TWO slots, not the three the decompiler's mangled
// symbol name suggests. The DLL prefixes each with a colour code
// (glance 0;31, dodge/miss 0;36, hit 1;31, kill 1;37) — unpainted here
// like every other room broadcast (see `monster_swing_lines`).

/// `0x481f87` — a landed monster-vs-monster swing.
pub fn monster_attacked_monster(attacker: &str, defender: &str) -> String {
    format!("{attacker} just attacked {defender}!")
}

/// `0x481f9d` — result 1, the armour-deflected glance. The DLL fills the
/// possessive slot with the ATTACKER and never names the weapon.
pub fn monster_glanced_off_monster(attacker: &str, defender: &str) -> String {
    format!("{attacker}'s just glanced off of {defender}'s armour.")
}

/// `0x481fc4` — result 3, the parry. Defender first (27267).
pub fn monster_dodged_monster(defender: &str, attacker: &str) -> String {
    format!("{defender} just dodged an attack from {attacker}.")
}

/// `0x481fe7` — result 0, the plain miss.
pub fn monster_missed_monster(attacker: &str, defender: &str) -> String {
    format!("{attacker} just missed an attack against {defender}.")
}

/// `0x481f73` — the kill line (27322); the defender's name is captured
/// before `check_kill_monster` frees the record (27253-27254).
pub fn monster_killed_monster(attacker: &str, defender: &str) -> String {
    format!("{attacker} just killed {defender}.")
}

/// VERIFIED (DLL): "You gain %s experience."
pub fn gain_experience(amount: u64) -> String {
    format!("You gain {amount} experience.")
}

/// Which audience a `castmsgb` line addresses. The discriminant is the
/// line index within the message record: line 1 → caster, line 2 → target,
/// line 3 → everyone else in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastAudience {
    Caster = 0,
    Target = 1,
    Room = 2,
}

/// Substitution arguments for a cast message. Absent arguments (no target,
/// no damage roll) are skipped, so target-less templates consume a prefix
/// of the per-audience order.
pub struct CastMsgArgs<'a> {
    pub caster: &'a str,
    pub target: Option<&'a str>,
    pub spell: &'a str,
    pub damage: Option<i32>,
}

/// Renders one line of a spell's `castmsgb` record, substituting `%s`/`%d`
/// left to right from the audience-appropriate argument order. Two order
/// tables, keyed on `odd_style` (callers pass `spell.msg_style & 1 == 1` —
/// the decompile branches display_spell_success on `spell+0xa4 & 1`):
///
/// EVEN (VERIFIED, oracle §8.6 + mmud_wgnt.sqlite messages 3242/2/7):
/// - caster line: spell, target, damage (the caster never appears);
/// - target line: caster, spell, damage;
/// - room line: caster, spell, target, damage.
///
/// ODD (~441 shipped spells, incl. fireball 120 / deathtouch 58; decompile
/// MEASURED 2026-07-30 (charm.md §8.6): the caster line renders the
/// spell-record message with the target exactly as modeled — "You sing
/// the song of charming to kobold!" live — and casting at your own
/// engaged target prints *Combat Off* first (the cast disengages
/// autocombat before the success line).
/// display_spell_success else-branch 38040-38124: caster prf(line, target,
/// damage), target prf(line, damage), room prf(line, target, damage) — NO
/// spell-name slot and NO caster name anywhere; shape: message 8524).
/// ORACLE-VERIFY: odd rendering is decompile-only — the lowest learnable
/// odd spells are annointed hands (744, L10 benign instant, scroll 1179 /
/// shop 111), dancing blades L11 and fireball L15; none measured live.
/// - caster line: target, damage;
/// - target line: damage;
/// - room line: target, damage.
///
/// Caller obligations: for self-casts pass `target = Some(caster_name)` and
/// do NOT deliver the Target line to anyone (oracle: `c blur` prints the
/// caster line only); damage spells must pass `damage: Some(_)` or `%d`
/// leaks literally; `damage: Some` without a resolved target mis-binds
/// targeted templates ("You cast blur on 13!").
///
/// `%d` and `%s` both accept the damage integer — message 3242's room line
/// uses `%s` for the number. Returns `None` for a missing or empty line
/// (message 1, the empty message, renders nothing). A `%` not followed by
/// `s`/`d`, or a placeholder beyond the available arguments, passes through
/// unchanged — unlike sibling [`fill_message`], which renders overflow
/// slots EMPTY to mirror the combat path's always-passed trailing `""`
/// args; each policy matches its own decompile evidence. `castmsga` is the empty message on every sampled spell, so
/// callers render `castmsgb` only and flag any spell shipping a non-empty
/// `castmsga`.
pub fn render_cast_line(
    msg: &crate::content::Message,
    audience: CastAudience,
    args: &CastMsgArgs<'_>,
    odd_style: bool,
) -> Option<String> {
    let line = msg.lines.get(audience as usize)?;
    if line.is_empty() {
        return None;
    }
    let damage = args.damage.map(|d| d.to_string());
    let damage = damage.as_deref();
    let order: [Option<&str>; 4] = match (odd_style, audience) {
        (false, CastAudience::Caster) => [Some(args.spell), args.target, damage, None],
        (false, CastAudience::Target) => {
            [Some(args.caster), Some(args.spell), damage, None]
        }
        (false, CastAudience::Room) => {
            [Some(args.caster), Some(args.spell), args.target, damage]
        }
        (true, CastAudience::Caster) | (true, CastAudience::Room) => {
            [args.target, damage, None, None]
        }
        (true, CastAudience::Target) => [damage, None, None, None],
    };
    let mut next_arg = order.into_iter().flatten();
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && matches!(chars.peek(), Some('s' | 'd'))
            && let Some(arg) = next_arg.next()
        {
            chars.next();
            out.push_str(arg);
            continue;
        }
        out.push(c);
    }
    Some(out)
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

/// VERIFIED (oracle): the status prompt. The mana segment appears only
/// while the max pool is non-zero (MEASURED §8.12: the L1 mystic prompt
/// is `[HP=28]:`, KAI from L2; §8.2 mage `[HP=26/MA=12]:`; warrior
/// HP-only) — the same `+0x600 != 0` gate as show_health.
pub fn prompt(hp: i32, max_hp: i32, mana: i32, max_mana: i32, caster_group: i16) -> String {
    // Low HP paints the number bright red (oracle: 3 of ~35 red, full
    // plain; the exact threshold is ORACLE-VERIFY — a quarter is used).
    // The frame paints 0;37 before every "]"/"/" itself (capture shape:
    // "[HP=" + 1;31 number + 0;37 "]:").
    let hp_str = if max_hp > 0 && hp * 4 < max_hp {
        format!("{}{hp}", color::DAMAGE)
    } else {
        hp.to_string()
    };
    let p = color::PLAIN;
    if max_mana == 0 {
        return format!("{p}[HP={hp_str}{p}]:{}", color::RESET);
    }
    let caption = if caster_group == 5 { "KAI" } else { "MA" };
    format!("{p}[HP={hp_str}{p}/{caption}={mana}{p}]:{}", color::RESET)
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
    /// DescMsg line3 of each active duration spell, in slot order —
    /// appended after the MagicRes row (MEASURED §8.11).
    pub active_lines: &'a [String],
    pub mana_current: i32,
    pub mana_max: i32,
    pub caster_group: i16,
}

/// VERIFIED (oracle): the nine-line status sheet, byte-exact to the
/// transcript except the whitelisted Martial Arts WG3-NT/DOS divergence.
/// Three columns at 0/18/39; right column label+value is 18 wide.
/// Non-caster layout + the measured kai row (§8.12); the groups 1-4
/// Mana/Spellcasting line is still ORACLE-VERIFY.
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
    // MEASURED (§8.12): the mystic mana row fills the Traps row's left
    // column (`Kai:      0/1`; 0/0 at L1 — shown regardless of max).
    // ORACLE-VERIFY: the groups 1-4 `Mana:` analog is unmeasured; the
    // non-caster blank is oracle-verified, so only group 5 renders.
    row(
        if d.caster_group == 5 {
            format!("Kai:{:>7}/{}", d.mana_current, d.mana_max)
        } else {
            String::new()
        },
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
    // MEASURED (§8.11): each active duration spell's DescMsg line3
    // ("You are blurred!") appends directly after the MagicRes row and
    // disappears with the slot.
    for line in d.active_lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}
