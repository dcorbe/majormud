//! Which command does this event answer?
//!
//! The board echoes every command it ACCEPTS, and replies strictly in
//! send order. That is the whole basis of position tracking: the room
//! block that answers `n` is the one that follows `n`'s echo, and a block
//! with no echo is somebody else's render and must never satisfy a
//! pending step (docs/board-correlation.md, measured 94.1% of 6,344
//! corpus blocks; the shortfall is genuinely unsolicited output).
//!
//! Four measured facts shape the rules (stopstate-run5/6/7 live captures,
//! transcribed into tests/correlate.rs):
//!
//! - **A busy board echoes twice.** A receipt echo ~2ms after send, and a
//!   second execution echo right before the reply when commands queue
//!   behind the round timer. Post-parse both are `Line(cmd)`, so a
//!   duplicate must re-confirm the entry it echoes, never advance.
//! - **Replies are FIFO.** With four `n` pipelined, echo text identifies
//!   nothing; only order does.
//! - **The board talks constantly without being asked** — swing
//!   announces, spawn wordings the classifier misses, "You hear
//!   movement...", exp and loot lines. A rule that retires the pending
//!   command on "whatever line came next" converts that din into wrong
//!   associations the moment two commands are in flight: run6 127-131s
//!   replays a room block landing on the WRONG move under exactly that
//!   rule. So retirement needs positive evidence — an event the pending
//!   command's reply grammar EXPECTS. We control the send vocabulary, so
//!   the grammar is small and closed; commands outside it (inventory,
//!   health) simply expire, a missed association their sender's own
//!   timeout absorbs.
//! - **A wrong association is worse than none.** Everything ambiguous —
//!   a never-arriving echo, a worse-than-observed split, an unknown line
//!   — answers nothing and falls to the deadline; the consumer re-asks or
//!   re-localizes.
//!
//! When a reply retires an entry, everything older goes with it: replies
//! are FIFO, so the answered command's juniors-in-waiting are the only
//! ones still owed anything. And any acceptance or retirement refreshes
//! every pending deadline — queue progress is proof the board is grinding
//! the round timer (receipt echo to execution echo was measured at ~3s
//! per queued command), while genuine silence still expires.
//!
//! Honest residuals, all bounded by the hard lifetime cap: a chat or
//! description line quoting a grammar wording verbatim retires — and IS
//! ATTRIBUTED TO — a pending entry of matching kind, a genuinely wrong
//! association (no such line exists in any capture; the corpus has no
//! second player talking, so this is unfalsified, not disproved); if two
//! same-text entries are pending and the older's echo was eaten
//! mid-block, the younger is marked accepted by the older's execution
//! echo and can claim one unsolicited block; a grammar gap (an
//! unmodelled reply wording) leaves its entry lingering until the cap,
//! able to claim one same-kind event meanwhile; multi-line replies to
//! Opaque commands attribute nothing at all — the one deliberate
//! exception is `Kind::Stat` (`stat`/`st`'s character sheet), retired by
//! the ordinary game prompt that follows it rather than by any wording
//! in the body, since the body has no fixed terminal line. Every
//! consumer keeps its own timeout precisely because of this floor.
//!
//! `SlowDown` flushes everything pending: flood control DROPPED input,
//! and whether a dropped command still echoes is unverified.
//!
//! This was nearly built three other ways, all wrong: counting unanswered
//! sends at the session layer (reverted in `7ddfd17` — "a proxy for
//! correlation is not correlation"), per-consumer echo watching (dies at
//! phase handoffs, double-counts the execution echo), and
//! retire-on-first-unknown-line (the run6 wrong association above).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::events::Event;

/// Identity of one sent line, allocated by the session.
///
/// Deliberately NOT `Ord`: numeric order equals wire order only within
/// one sending task (allocation and enqueue race across tasks), so the
/// only sound consumer operation is equality against
/// [`Correlated::answers`] — and a `<` that would compile is a bug that
/// would not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CmdId(pub u64);

/// An event plus the command it answers, if any. `None` means
/// unsolicited: it must never satisfy anyone's pending step, but it still
/// flows — flee detection and re-localization live on unsolicited blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correlated {
    pub event: Event,
    pub answers: Option<CmdId>,
    /// True when `event` is a room block describing a room the character
    /// is NOT standing in — the answer to a `look <direction>`.
    ///
    /// Without this a peek is indistinguishable from an arrival, and
    /// `Navigator::localize` searches the current room PLUS its
    /// neighbours by name, so the neighbour matches and the client's
    /// position walks one room per peek. Consumers that model WHERE the
    /// character is — position tracking, the occupancy model — must
    /// ignore such a block; consumers that model what the world looks
    /// like may still read it.
    pub elsewhere: bool,
}

/// Strip the board's single leading parenthesized status decoration:
/// `(Resting) look` -> `look` (DLL 0xe06f6; other markers are uncaptured,
/// so any single leading `(...)` run strips, spaces inside included).
pub fn strip_decoration(s: &str) -> &str {
    s.strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .map(|(_, after)| after.trim_start())
        .unwrap_or(s)
}

/// THE definition of "this line is the echo of that command".
///
/// After trimming and stripping a status decoration: the exact text, or a
/// proper suffix of it — the split echo, where async output lands
/// mid-echo and `look` reaches the parser as `l` + `ook` (~0.4% live).
/// Floors keep the fragment rule honest: the command must be >= 3 chars
/// and the fragment >= 2, so a stray `n` never reads as the tail of
/// `open n` and one-letter directions only ever match exactly.
pub fn is_echo(line: &str, cmd: &str) -> bool {
    // An empty command matches nothing: board output is full of
    // whitespace-only and decoration-only lines that strip to empty, and
    // a falsely-accepted empty entry would FIFO-drop real pending
    // commands as "eaten".
    if cmd.is_empty() {
        return false;
    }
    let line = strip_decoration(line.trim());
    if line == cmd {
        return true;
    }
    cmd.len() >= 3 && line.len() >= 2 && line.len() < cmd.len() && cmd.ends_with(line)
}

/// The shape of reply a command's answer can take. Derived from the
/// command text — the client controls its own send vocabulary, so this is
/// closed and small. `Opaque` commands (inventory, health, attacks) have
/// no modelled reply: they are retired by their deadline alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Move,
    Look,
    LookDir,
    Open,
    Bash,
    Cast,
    Light,
    Get,
    BuyHealing,
    Search,
    Picklock,
    /// `sneak` (theft.md §11.1). Like `Stat`, its body has no fixed
    /// terminal wording — success is genuinely silent (`mud-core`'s
    /// `sneak_command` returns having printed nothing past "Attempting
    /// to sneak..."), and failure is reported only when a perception
    /// roll passes. So the only thing that reliably closes the reply is
    /// the ordinary game prompt that follows it, same as `Stat` — see
    /// the `Event::Prompt` arm of `completes`. Classified at all so an
    /// unattributed "You may not sneak right now!" or "You don't think
    /// you're sneaking." can never be read as somebody else's line, and
    /// so a caller waiting on the reply is not left to burn its whole
    /// deadline the way the locked door once did (`b19f862d`).
    Sneak,
    /// `stat`/`st` — the multi-line character sheet. Unlike every other
    /// modelled kind, no wording in the BODY completes it (there is no
    /// fixed terminal line — `Traps`/`Picklocks` can be the last row or
    /// not depending on active buffs, see `mud-core`'s `show_sheet`).
    /// What completes it is the ORDINARY game prompt that follows any
    /// command's reply — see the `Event::Prompt` arm of `completes`.
    /// [`crate::session::Session`]'s `StatTracker` accumulates the body
    /// lines itself; this Kind's only job is making sure THAT reply gets
    /// attributed at all, rather than sitting `Opaque` (`completes`
    /// always `false`) until a waiting consumer burns its whole
    /// deadline — the exact class of bug `b19f862d` fixed for the
    /// locked door.
    Stat,
    Opaque,
}

/// Is this command a bare movement? The flee and travel vocabulary —
/// consumers use it to know their own send is about to change the room.
pub fn is_movement(cmd: &str) -> bool {
    matches!(kind_of(cmd), Kind::Move)
}

fn kind_of(cmd: &str) -> Kind {
    const DIRS: [&str; 20] = [
        "n", "s", "e", "w", "ne", "nw", "se", "sw", "u", "d", "north", "south", "east", "west",
        "northeast", "northwest", "southeast", "southwest", "up", "down",
    ];
    if DIRS.contains(&cmd) {
        return Kind::Move;
    }
    if cmd == "look" || cmd == "l" {
        return Kind::Look;
    }
    // The character sheet. `mud-core`'s alias table resolves both to
    // `Command::Status`: `status`'s minimum abbreviation is 2 (`st`),
    // and `stat` matches too since `"status".starts_with("stat")`
    // (`crates/mud-core/src/command.rs`'s `ALIASES`).
    if cmd == "stat" || cmd == "st" {
        return Kind::Stat;
    }
    // `look <direction>` and its `l <direction>` alias answer with a full
    // room block for the NEIGHBOUR (vendor relnotes: "LOOK <dir> will now
    // give the same detail as moving"). Same reply shape as a move, other
    // vantage — so it needs its own kind rather than sharing `Look`,
    // whose block IS where the character stands.
    if let Some(rest) = cmd.strip_prefix("look ").or_else(|| cmd.strip_prefix("l ")) {
        if DIRS.contains(&rest.trim()) {
            return Kind::LookDir;
        }
    }
    if cmd.starts_with("look ") {
        return Kind::Look;
    }
    if cmd.starts_with("open ") {
        return Kind::Open;
    }
    if cmd.starts_with("bash ") {
        return Kind::Bash;
    }
    // The thief's answer to a locked door. The board takes any prefix
    // from `pi`, but only the DIRECTED form is ours: a bare `pick` and
    // `pick up <thing>` are different commands with different replies,
    // and classifying those here would leave an entry lingering to claim
    // somebody else's line.
    if cmd.starts_with("picklock ") {
        return Kind::Picklock;
    }
    if let Some(rest) = cmd.strip_prefix("pick ") {
        if DIRS.contains(&rest.trim()) {
            return Kind::Picklock;
        }
    }
    // Only the DIRECTED form. A bare `search` re-lists the room's items
    // and answers with a wording this grammar does not model; the client
    // never sends one, and classifying it here would leave an entry
    // lingering to claim somebody else's line.
    if cmd.starts_with("search ") {
        return Kind::Search;
    }
    // `invoke` is the mystic's `cast` — same grammar, kai wording
    // (`mud_core::text` §8.12), so it shares the kind rather than growing
    // a parallel one that would have to be kept in step.
    if cmd.starts_with("cast ") || cmd.starts_with("invoke ") {
        return Kind::Cast;
    }
    if cmd.starts_with("light ") {
        return Kind::Light;
    }
    if cmd.starts_with("get ") {
        return Kind::Get;
    }
    if cmd == "buy healing" {
        return Kind::BuyHealing;
    }
    if cmd == "sneak" {
        return Kind::Sneak;
    }
    Kind::Opaque
}

/// The board's answer to seeing (or stepping into) an unlit room. Also in
/// `sheet::TOO_DARK`; duplicated here rather than imported so the grammar
/// reads as one table. Both are pinned against the DLL wording.
const DARK: &str = "you can't see anything";

/// Does this event complete `kind`'s reply? Wordings verified against the
/// live captures and the DLL string table (see nav.rs constants, which
/// classify the same lines for the walk's own purposes).
///
/// The move/look door split matters: "The door is closed!" (with the
/// bang) refuses a MOVE; "The door is closed in that direction!" refuses
/// a LOOK. Conflating them is how a look-refusal used to read as a step
/// being blocked.
fn completes(kind: Kind, ev: &Event) -> bool {
    let line = match ev {
        Event::RoomSeen(_) => {
            // A block answers movement and looking — and a bash that
            // carried the character through the doorway.
            return matches!(kind, Kind::Move | Kind::Look | Kind::LookDir | Kind::Bash);
        }
        // The ordinary game prompt is the ONLY thing that completes a
        // Stat reply — its body has no fixed terminal wording (see
        // `Kind::Stat`'s doc). `Kind::Sneak` shares that shape for the
        // same reason (see its doc): every other kind answers to a
        // RoomSeen or a specific Line wording instead, so a Prompt
        // completes nothing for them:
        // `classified_async_events_answer_nothing_and_retire_nothing`
        // (tests/correlate.rs) pins that down.
        Event::Prompt { .. } => return matches!(kind, Kind::Stat | Kind::Sneak),
        Event::Line(l) => l.to_lowercase(),
        _ => return false,
    };
    let has = |needle: &str| line.contains(needle);
    match kind {
        // The refusal wordings are pinned to their emitters via the
        // ReMUD decompile: _move_user says "There is a closed door in
        // that direction!" (exit-type case 2, beside "There is no
        // exit...", plus 0xbc745 / 0xbc8c4); _cmd_look says "The door is
        // closed in that direction!". Crossing them hands a post-move
        // room to a look consumer, or leaves a refused move lingering to
        // claim the next block.
        // The full _move_user refusal table from the ReMUD decompile
        // (31588-33100), second-person terminal refusals only. NO locked
        // wording — a move at a locked door answers "The door is
        // closed!" (22+ corpus occurrences agree); "locked" replies
        // belong to open alone, and carrying them here let a stale move
        // steal an open's refusal. The bang on "move anywhere!" keeps
        // the hide refusal ("...to move anywhere to hide!") out.
        Kind::Move => {
            has(DARK)
                || has("no exit in that direction")
                // Both terminators: stock's _move_user prints the bang;
                // foreign reimplementations soften it to a period (live,
                // cwgaming 2026-08-01 — the unretired move then claimed
                // the navigator's arrival block and the walk timed out a
                // room behind itself). The period variant cannot collide
                // with the LOOK refusal: that wording continues "...in
                // that direction!" and never carries the period.
                || has("the door is closed!")
                || has("the door is closed.")
                || has("the gate is closed!")
                || has("the gate is closed.")
                || has("closed door in that direction")
                || has("may not enter that room while in combat")
                || has("may not enter that room during a retaliation")
                || has("can't seem to move anywhere!")
                || has("need to cast a spell to go that way")
                || has("not permitted in that room")
                || has("too evil to go through this exit")
                || has("too good to go through this exit")
                || has("too heavy to move")
                || has("too stunned to move anywhere!")
                || has("appropriate item to go that direction")
                || has("progressed too far to go through this exit")
                || has("may not go through this exit")
                || has("may not pass through that exit")
                || has("cover the toll of")
        }
        Kind::Look | Kind::LookDir => {
            has(DARK) || has("door is closed in that direction") || has("there are no exits")
        }
        Kind::Open => {
            has("is now open")
                || has("already open")
                || has("successfully unlocked")
                || has("the door is locked")
                || has("the gate is locked")
        }
        // "Your attempts to bash through fail!" is captured live
        // (stopstate-run1, 170s) — the wording nav never recognized,
        // which is why doors "gave up". The carried-through wording is
        // handled by `confirms`, not here: its block is the arrival.
        // The roll's two outcomes (`re/docs/theft.md` §8.2/§8.3): the
        // lock gives, or the skill check fails and the door stays shut.
        // The failure wording is shared with an unpickable exit type --
        // both mean "this door did not open", which is all the walk
        // needs to decide whether to roll again.
        Kind::Picklock => has("unlocked the door") || has("skill fails you"),
        Kind::Bash => {
            has("bashed the")
                || has("bash through fail")
                || has("must wait before you may do that")
        }
        // "You attempt to cast %s, but fail." — the leading "you
        // attempt" matters: the DLL also ships "%s attempted to cast %s
        // at you, but failed.", routine din from any casting monster,
        // which must never retire OUR cast.
        Kind::Cast => {
            (has("you attempt to cast") && has("but fail"))
                || has("you cast ")
                || has("spell is resisted")
                || has("resists your spell")
                || has("already cast a spell")
                || has("enough mana to cast")
                // The spell is not in the book at all. Terminal, and the
                // only cast outcome worth remembering past the round:
                // `sheet::HealState` retires that source for the run.
                || has("do not know how to cast")
                // A cast of a light spell routes through the light
                // routine and answers with its wordings — success and
                // the already-lit refusal alike.
                || has("you lit the")
                || has("already have something lit")
                // The mystic's half of the same table (`mud_core::text`
                // §8.12, all VERIFIED): kai for mana, power for spell,
                // invoke for cast.
                || has("you invoke ")
                || has("already invoked a power")
                || has("enough kai to invoke")
                || has("do not know how to invoke")
        }
        Kind::Light => {
            has("you lit the")
                || has("already have something lit")
                || has("may not light that item")
        }
        // SEARCH <dir> (theft.md §9). The roll either reveals the exit
        // ("You found an exit to the north!", "...upwards!",
        // "...downwards!") or reports nothing — and "nothing" is also
        // what an ALREADY-found exit says, since the found state falls
        // through the same else. Both terminal; so are the two refusals,
        // which never roll at all.
        //
        // Deliberately no RoomSeen arm above: a search moves nobody, so a
        // block during one is somebody else's render.
        Kind::Search => {
            has("you found an exit")
                || has("you notice nothing different")
                || has("may not search while attacking")
                || has("why would you want to search that")
        }
        Kind::Get => has("you picked up"),
        Kind::BuyHealing => has("wounds are healed"),
        // No line completes it — see the `Event::Prompt` arm above.
        Kind::Stat => false,
        // Same reasoning as `Kind::Stat` — see its doc and this match's
        // `Event::Prompt` arm above.
        Kind::Sneak => false,
        Kind::Opaque => false,
    }
}

/// Wordings that CONFIRM a command without completing it: the reply is
/// still owed. The bash that carries the character through the doorway
/// (DLL 0xd538e, "You bash the door open and walk through") announces
/// itself and then renders the arrival — the block is the answer, so the
/// announce must not retire the entry or every successful bash-through
/// arrival would read unsolicited.
fn confirms(kind: Kind, line: &str) -> bool {
    matches!(kind, Kind::Bash) && line.to_lowercase().contains("walk through")
}

struct Entry {
    id: CmdId,
    text: String,
    kind: Kind,
    /// The board echoed it: accepted, reply owed. Set by the receipt or
    /// execution echo, whichever arrives first.
    echoed: bool,
    /// Past this instant the entry is forgotten and its late answers read
    /// as unsolicited. Refreshed by every acceptance and retirement —
    /// queue progress — so only genuine silence expires anyone.
    deadline: Instant,
    /// Absolute cap, never refreshed: progress-refresh exists for
    /// round-timer waits, not for keeping a command whose reply was eaten
    /// alive under constant traffic to steal a block minutes later.
    lifetime: Instant,
}

/// The one correlator, owned by the session: registry fed in wire order,
/// events attributed in arrival order.
pub struct Correlator {
    queue: VecDeque<Entry>,
    ttl: Duration,
}

impl Correlator {
    pub fn new(ttl: Duration) -> Self {
        Correlator { queue: VecDeque::new(), ttl }
    }

    /// Record a line the writer put on the wire. Must be called in wire
    /// order — echoes arrive in send order and attribution is FIFO. Pass
    /// the command text, not the wire bytes: trailing CR/LF would defeat
    /// every echo match silently.
    pub fn sent(&mut self, id: CmdId, line: &str, now: Instant) {
        self.sent_kind(id, line, false, now)
    }

    /// As [`Correlator::sent`], but the caller asserts this line moves
    /// the character however it is worded.
    ///
    /// For **command exits**, where the board is walked with a phrase
    /// rather than a direction — `borrow skiff`, `go manhole`,
    /// `climb tree`. [`kind_of`] reads direction words and would file
    /// these as `Opaque`, so the arriving room block would answer
    /// nothing and the navigator would wait out its deadline standing in
    /// the room it had already reached.
    ///
    /// Deliberately not solved by teaching `kind_of` to recognise `go `
    /// and `climb `: those are ordinary words a player may type at a
    /// board that says unknown commands out loud, and a wrong `Move`
    /// classification lets an unrelated line retire a real step. The
    /// graph knows which exits are command exits; the string does not.
    pub fn sent_move(&mut self, id: CmdId, line: &str, now: Instant) {
        self.sent_kind(id, line, true, now)
    }

    fn sent_kind(&mut self, id: CmdId, line: &str, force_move: bool, now: Instant) {
        debug_assert_eq!(line, line.trim(), "sent() wants the trimmed command text");
        self.expire(now);
        // A bare Enter is meaningful on the wire (pager advance) but
        // meaningless to correlate — never registered, so it can never
        // be falsely accepted.
        if line.is_empty() {
            return;
        }
        self.queue.push_back(Entry {
            id,
            text: line.to_string(),
            kind: if force_move { Kind::Move } else { kind_of(line) },
            echoed: false,
            deadline: now + self.ttl,
            // Generous: covers the deepest measured pipeline (~3s of
            // round-timer wait per queued command) several times over,
            // while still bounding the eaten-reply residual absolutely.
            lifetime: now + self.ttl * 4,
        });
    }

    /// Forget everything pending (reconnect, phase reset, flood flush).
    pub fn flush(&mut self) {
        self.queue.clear();
    }

    /// Attribute one parsed event.
    pub fn on_event(&mut self, event: Event, now: Instant) -> Correlated {
        self.expire(now);
        let mut elsewhere = false;
        let answers = match &event {
            Event::SlowDown => {
                // Input above this point was DROPPED; whether dropped
                // commands still echo is unverified. Forget everything.
                self.flush();
                None
            }
            Event::Line(l) => {
                let l = l.clone();
                self.on_line(&l, now)
            }
            Event::RoomSeen(_) => match self.retire(&event, now) {
                Some((id, kind)) => {
                    elsewhere = kind == Kind::LookDir;
                    Some(id)
                }
                None => None,
            },
            // The ordinary game prompt: answers nothing for any pending
            // kind except Stat, whose reply has no other terminator (see
            // `Kind::Stat`). `retire` itself decides via `completes`, so
            // this arm only exists to give Stat a path to it at all —
            // every other kind sees the same `None` it always has.
            Event::Prompt { .. } => self.retire(&event, now).map(|(id, _)| id),
            // Classified async traffic: combat, actors. The board emits
            // these freely; they answer nothing.
            Event::CombatHit { .. }
            | Event::CombatMiss { .. }
            | Event::ActorEntered { .. }
            | Event::ActorLeft { .. } => None,
        };
        Correlated { event, answers, elsewhere }
    }

    /// A line is one of four things, checked in order: the echo of the
    /// oldest not-yet-accepted entry (acceptance — and every earlier
    /// never-echoed entry was eaten, echoes arrive in send order); a
    /// duplicate echo of an already-accepted entry (the execution echo —
    /// confirm, never advance); a confirm wording (the reply is
    /// announced but still owed — see `confirms`); or a reply completing
    /// the oldest accepted entry that expects it. Anything else — the
    /// board's constant unsolicited din — answers nothing and retires
    /// nothing.
    fn on_line(&mut self, line: &str, now: Instant) -> Option<CmdId> {
        if let Some(pos) = self
            .queue
            .iter()
            .position(|e| !e.echoed && is_echo(line, &e.text))
        {
            let id = self.queue[pos].id;
            self.queue[pos].echoed = true;
            // Every EARLIER entry that never echoed was eaten — echoes
            // arrive in send order. Earlier accepted entries still owe
            // their replies and stay.
            let mut idx = 0;
            self.queue.retain(|e| {
                let keep = e.echoed || idx >= pos;
                idx += 1;
                keep
            });
            self.refresh(now);
            return Some(id);
        }
        if let Some(dup) = self
            .queue
            .iter()
            .find(|e| e.echoed && is_echo(line, &e.text))
        {
            let id = dup.id;
            self.refresh(now);
            return Some(id);
        }
        if let Some(pos) = self
            .queue
            .iter()
            .position(|e| e.echoed && confirms(e.kind, line))
        {
            // The confirm is FIFO evidence too: this command executing
            // means everything senior was answered or never will be —
            // left in place, a stale senior would steal the confirmed
            // command's arrival block.
            let id = self.queue[pos].id;
            self.queue.drain(..pos);
            self.refresh(now);
            return Some(id);
        }
        self.retire(&Event::Line(line.to_string()), now).map(|(id, _)| id)
    }

    /// The reply completes the oldest accepted entry whose grammar
    /// expects it — accepted entries with unmodelled replies are skipped,
    /// not credited. Retirement takes everything older with it: replies
    /// are FIFO, so anything senior to the answered command was already
    /// answered or never will be.
    fn retire(&mut self, ev: &Event, now: Instant) -> Option<(CmdId, Kind)> {
        let pos = self
            .queue
            .iter()
            .position(|e| e.echoed && completes(e.kind, ev))?;
        let id = self.queue[pos].id;
        let kind = self.queue[pos].kind;
        self.queue.drain(..=pos);
        self.refresh(now);
        Some((id, kind))
    }

    /// Queue progress refreshes every pending deadline: a queued command
    /// hears nothing about itself between its receipt echo and its
    /// execution echo (~3s per queued round measured live), and only
    /// genuine silence should expire it.
    fn refresh(&mut self, now: Instant) {
        for e in &mut self.queue {
            e.deadline = now + self.ttl;
        }
    }

    fn expire(&mut self, now: Instant) {
        self.queue.retain(|e| now <= e.deadline && now <= e.lifetime);
    }
}
