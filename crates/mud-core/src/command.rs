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
    /// `powers` — the kai listing (spellcasting.md §8.12); a non-kai
    /// class gets the redirect refusal.
    Powers,
    /// `invoke [power [target]]` — the kai cast verb (§8.12). No
    /// abbreviation: `in`/`inv` fall through to say (MEASURED).
    Invoke(String),
    Help,
    /// `top [n] [gangs]` — the ranking listings (gangs.md §5.1). The
    /// gangs arm ships in M7; the player arm is a stub.
    Top(String),
    Train,
    /// `attack [target]` — empty target means auto-pick.
    Attack(String),
    /// `punch [target]` — mode-1 fists of fury (cmd_punch 0x51e37);
    /// without the Punch ability the input is unconsumed (say).
    Punch(String),
    /// `kick [target]` — mode-2 lightning feet (cmd_kick 0x51df2).
    Kick(String),
    /// `jumpkick [target]` — mode-3 flying feet (cmd_jumpkick 0x51dad).
    JumpKick(String),
    /// `ansi` — the per-user colour toggle. OURS (divergence): the real
    /// board keys ANSI on the MBBS account, outside the DLL.
    Ansi,
    /// `set <option>` — cmd_set 0x458b60; only EVIL ships today.
    Set(String),
    /// `rob [target]` — cmd_rob 0x4528fb; bare form prints the syntax.
    Rob(String),
    /// `picklock <direction>` — cmd_picklock 0x454856.
    Picklock(String),
    /// `search [direction]` — cmd_search 0x454fc9.
    Search(String),
    /// `disarm trap <direction>` — cmd_disarm 0x468bed.
    Disarm(String),
    /// `forgive <player>` — refund a live pair timer (cmd_forgive).
    Forgive(String),
    /// `ask <monster> [question]` — the keyword dialogue (cmd_ask
    /// 0x458306; quests.md §3). Bare `ask` and an unresolved monster
    /// fall through to say.
    Ask(String),
    /// `sneak` — arm stealth for the next move (cmd_sneak 0x454641).
    Sneak,
    /// `backstab [target]` — cmd_backstab 0x51573: mode 4 when
    /// hidden/sneaking (weapon must carry BSAccu 0x74), else a plain
    /// attack.
    Backstab(String),
    /// `hide` — hide self; item/coin stash forms arrive with the room
    /// hidden-storage work (cmd_hide 0x466b1f).
    Hide(String),
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
    /// `light [<item>]` — bare reports the level; with a target, the
    /// `_CMD_LIGHT` refusal ladder (darkness plan).
    Light(String),
    /// `broadgang [message]` — args = the gang broadcast (cmd_broadgang
    /// 0x585cc, "%s gangpaths: %s" MEASURED slice8_gang1b.raw), bare =
    /// the §1.6 roster (ORACLE-VERIFY: bare form unprobed live; the
    /// roster display functions have no other player-facing trigger
    /// found — gang/guild themselves are NOT verbs in 1.11p-WG).
    Gang(String),
    /// `create gang|guild <name>` (cmd_create 0x57d4a); other forms hit
    /// the DLL's stubbed house-build path (gangs.md §3.2).
    Create(String),
    /// `join gang <name>` (cmd_join 0x541fb); bare prints the syntax
    /// line. Party/group joins are unported (M8) — they fall through.
    Join(String),
    /// `invite <name>` — leader/lieutenant only (gangs.md §1.2).
    Invite(String),
    /// `uninvite <name>` — remove a member, online or offline (§1.4).
    Uninvite(String),
    /// `promote <name>` / `demote <name>` — lieutenant rank (§1.5).
    Promote(String),
    Demote(String),
    /// `disband gang` — leader only (§1.4).
    Disband(String),
    /// `leave gang` — non-leader departure (§1.4). Bare/other forms
    /// fall through.
    Leave(String),
    /// Gang stock-shop verbs (cmd_stock 0x52b1e / cmd_unstock 0x5326b /
    /// cmd_markup 0x53715, gangs.md §3.1 — behavior lands with the
    /// guild-house task).
    Stock(String),
    Unstock(String),
    Markup(String),
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
const ALIASES: [(&str, Command); 13] = [
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
    // `i` is inventory (oracle) but `in`/`inv` are SAY (MEASURED §8.12,
    // oracle_kai_mystic.raw) — an exact alias, not a min-1 prefix.
    ("i", Command::Inventory),
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
const VERBS: [(&str, usize, Verb); 68] = [
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
    // MEASURED (slice8_abbrevs.raw): p says, pu punches.
    ("punch", 2, Verb::WithArgs(Command::Punch)),
    // MEASURED (slice8_abbrevs.raw): b/ba/bac/back all say; backs is
    // the shortest accepted form.
    ("backstab", 5, Verb::WithArgs(Command::Backstab)),
    // MEASURED (slice8_abbrevs.raw): k/ki say, kic kicks.
    ("kick", 3, Verb::WithArgs(Command::Kick)),
    // MEASURED (slice8_abbrevs.raw): j says, ju jumpkicks.
    ("jumpkick", 2, Verb::WithArgs(Command::JumpKick)),
    // Min 1 like attack, so `c` and `c args` both cast (MEASURED §8.9).
    // ORACLE-VERIFY: only c/cast measured; ca/cas assumed by prefix model.
    ("cast", 1, Verb::WithArgs(Command::Cast)),
    // MEASURED (§8.12 + slice8_abbrevs.raw): in/inv say, invo invokes —
    // minimum 4, not the no-abbreviation model shipped through M7.
    ("invoke", 4, Verb::WithArgs(Command::Invoke)),
    ("aid", 2, Verb::WithArgs(Command::Aid)),       // ORACLE: ai
    ("get", 1, Verb::WithArgs(Command::Get)),       // ORACLE: g/ge/get
    ("drop", 2, Verb::WithArgs(Command::Drop)),
    // `i` moved to the exact aliases: `in`/`inv` are SAY (MEASURED
    // §8.12). ORACLE-VERIFY: the 4+ prefixes (inve...) are unmeasured.
    ("inventory", 4, Verb::Plain(|| Command::Inventory)),
    ("arm", 2, Verb::WithArgs(Command::Arm)),       // ORACLE: ar arms
    ("wield", 3, Verb::WithArgs(Command::Arm)),     // ORACLE: wi says, wie arms
    ("equip", 2, Verb::WithArgs(Command::Arm)),     // ORACLE: eq
    ("wear", 3, Verb::WithArgs(Command::Wear)),     // min 3: "we" says (oracle)
    ("remove", 3, Verb::WithArgs(Command::Remove)),
    // ORACLE-VERIFY min abbrev: unmeasured; "u" is the up alias, so 2.
    ("use", 2, Verb::WithArgs(Command::Use)),
    // MEASURED (slice8_abbrevs.raw): r/re say and "rea" belongs to the
    // UNSHIPPED READY verb ("Syntax: READY {weapon name}") — read's own
    // shortest form is the full word. KNOWN-DIVERGENCE: our "rea" says
    // instead of printing READY's syntax line; ready lands with its
    // system (M8 candidate).
    ("read", 4, Verb::WithArgs(Command::Read)),
    // ORACLE-OPEN min abbrev: unmeasured; the full word until a capture
    // says otherwise ("li" belongs to list).
    ("light", 5, Verb::WithArgs(Command::Light)),
    ("list", 2, Verb::Plain(|| Command::List)),
    ("buy", 2, Verb::WithArgs(Command::Buy)),
    ("sell", 3, Verb::WithArgs(Command::Sell)),
    ("deposit", 3, Verb::WithArgs(Command::Deposit)),
    ("withdraw", 4, Verb::WithArgs(Command::Withdraw)),
    ("balance", 3, Verb::Plain(|| Command::Balance)),
    ("look", 2, Verb::Plain(|| Command::Look)),     // ORACLE: lo
    // MEASURED (slice8_abbrevs2.raw): "as healer hello" asks — 2 with a
    // target. Bare ask/as at ANY length says (the tree matches the
    // pattern, not the bare word; quests.md §3).
    ("ask", 2, Verb::WithArgs(Command::Ask)),
    ("exits", 3, Verb::Plain(|| Command::Exits)),   // ORACLE: exi (ex says)
    ("experience", 3, Verb::Plain(|| Command::Experience)), // ORACLE: exp
    ("status", 2, Verb::Plain(|| Command::Status)), // ORACLE: st/sta/stat
    ("help", 3, Verb::Plain(|| Command::Help)),     // ORACLE: hel (before health)
    ("health", 2, Verb::Plain(|| Command::Health)), // ORACLE: he
    // ORACLE-VERIFY min abbrev: unmeasured; 2 is unambiguous ("s" is the
    // south alias, "st" hits status first).
    ("spells", 2, Verb::Plain(|| Command::Spells)),
    // ORACLE-VERIFY min abbrev: unmeasured; 2 is unambiguous.
    ("powers", 2, Verb::Plain(|| Command::Powers)),
    ("top", 2, Verb::WithArgs(Command::Top)),       // ORACLE: to (t says)
    ("train", 4, Verb::Plain(|| Command::Train)),   // ORACLE: trai (tra says)
    ("quit", 1, Verb::Plain(|| Command::Quit)),     // ORACLE: q
    // OURS (divergence): no DLL surface exists — full word only.
    ("ansi", 4, Verb::Plain(|| Command::Ansi)),
    // ORACLE-VERIFY min abbrev: unmeasured ("se" cannot shadow sell's 3
    // — "sel" is not a prefix of "set"; keep 3 to be safe).
    ("set", 3, Verb::WithArgs(Command::Set)),
    // ORACLE-VERIFY min abbrevs: unmeasured for both.
    ("sneak", 2, Verb::Plain(|| Command::Sneak)),
    ("hide", 3, Verb::WithArgs(Command::Hide)),
    // MEASURED (slice8_abbrevs.raw): ro robs ("Syntax: ROB
    // {user/monster}").
    ("rob", 2, Verb::WithArgs(Command::Rob)),
    // MEASURED (slice8_abbrevs2.raw): "pi n" picks at 2 — WITH a
    // direction; every bare prefix through the full word says (the tree
    // matches the pattern). "pick lock n" also reaches it. Our bare
    // "pi" prints the syntax line instead of saying — KNOWN-DIVERGENCE,
    // same family as ask's pattern-gated parse.
    ("picklock", 2, Verb::WithArgs(Command::Picklock)),
    // MEASURED (slice8_abbrevs.raw): "sea" is the shortest form that
    // searches — se belongs to southeast (our alias handles it; the 3
    // keeps the table honest if the alias ever moves).
    ("search", 3, Verb::WithArgs(Command::Search)),
    // MEASURED (slice8_abbrevs.raw/2): di/dis/disarm all silently no-op
    // in an empty room (no say) — 2 is live; 3 kept "di" free before,
    // but the live tree owns di.
    ("disarm", 2, Verb::WithArgs(Command::Disarm)),
    ("forgive", 4, Verb::WithArgs(Command::Forgive)),
    // --- M7 slice 7: gangs (gangs.md §1, §5). Minimums MEASURED at the
    // slice-8 sweep (slice8_abbrevs.raw, slice8_abbrevs2.raw,
    // slice8_gang1b.raw) unless noted. ---
    // MEASURED: gang/guild are NOT verbs in 1.11p-WG (they say even for
    // members, args or not) — the broadcast verb is BROADGANG, minimum
    // 6, rendered "%s gangpaths: %s". Bare broadgang keeps the roster
    // arm (ORACLE-VERIFY: the bare form was not probed live; the §1.6
    // roster display functions exist in the DLL with no other
    // player-facing trigger found).
    ("broadgang", 6, Verb::WithArgs(Command::Gang)),
    // MEASURED: "cr gang X" refused on the exp gate — 2, pattern-gated
    // (bare create says).
    ("create", 2, Verb::WithArgs(Command::Create)),
    // `j` stays jumpkick's single letter.
    ("join", 2, Verb::WithArgs(Command::Join)),
    // `in`/`inv` are MEASURED say (§8.12) — invite cannot start below 4
    // ("invi" vs inventory's "inve" and invoke's "invo").
    ("invite", 4, Verb::WithArgs(Command::Invite)),
    // MEASURED: un says, uni uninvites; uns unstocks.
    ("uninvite", 3, Verb::WithArgs(Command::Uninvite)),
    ("unstock", 3, Verb::WithArgs(Command::Unstock)),
    // MEASURED: pr/pro belong to an UNSHIPPED session-info verb live
    // ("Life for this CHAR ..."); prom promotes. KNOWN-DIVERGENCE: our
    // pr/pro say instead of that panel.
    ("promote", 4, Verb::WithArgs(Command::Promote)),
    // MEASURED: de says, dem demotes.
    ("demote", 3, Verb::WithArgs(Command::Demote)),
    // "dis" stays disarm's; disband starts at 4.
    ("disband", 4, Verb::WithArgs(Command::Disband)),
    ("leave", 2, Verb::WithArgs(Command::Leave)),
    // MEASURED: sto stocks ("st"/"sta" stay status's).
    ("stock", 3, Verb::WithArgs(Command::Stock)),
    // MEASURED: ma belongs to the UNSHIPPED MAP verb live (the town
    // ANSI map); mar markups. KNOWN-DIVERGENCE: our ma says.
    ("markup", 3, Verb::WithArgs(Command::Markup)),
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
