//! The session engine: one TCP connection to a board, shared by the
//! TUI, scripts, and bot. A tokio actor owns the socket; consumers see
//! a paced `send` returning a [`CmdId`], expect-style waits over the
//! cleaned transcript, a broadcast of attributed [`Correlated`] events,
//! raw bytes for terminal passthrough, and a `watch` of the accumulated
//! [`GameState`].
//!
//! Correlation lives HERE and nowhere else: the writer task feeds the
//! registry in wire order right before each write, the reader task
//! attributes each parsed event as it is born, and `answers` rides
//! inside the broadcast — so a fresh subscriber (every phase handoff
//! makes one) inherits correct attribution instead of a private guess.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};

use crate::correlate::{CmdId, Correlated, Correlator};
use crate::equipment::Equipment;
use crate::events::{Event, RoomView};
use crate::graph::{Capabilities, TollLog};
use crate::parse::Parser;
use crate::profile::Profile;
use crate::purse::PurseMeter;
use crate::sheet::{Casting, Inventory, Spellbook};
use crate::stats::Stats;
use crate::wire::{AnsiStripper, TelnetFilter, cp437_to_string};

/// Send pacing: reserves evenly spaced slots at least `min` apart
/// (live-board flood control drops input above ~15 rapid commands).
pub struct Pacer {
    min: Duration,
    next_slot: Option<Instant>,
}

impl Pacer {
    pub fn new(min: Duration) -> Self {
        Pacer {
            min,
            next_slot: None,
        }
    }

    /// Change the interval mid-stream. One session serves both a person
    /// and a farm — `mmc play` types unpaced, `/farm` hands the same
    /// connection to automation — and only the automation needs flood
    /// control. Already-reserved slots are not served out after a drop
    /// to zero: the operator taking the keyboard is not a burst.
    pub fn set_min(&mut self, min: Duration) {
        self.min = min;
    }

    /// Delay to wait before sending at `now`; reserves the slot.
    pub fn delay_for(&mut self, now: Instant) -> Duration {
        if self.min.is_zero() {
            return Duration::ZERO;
        }
        let slot = match self.next_slot {
            None => now,
            Some(s) => s.max(now),
        };
        self.next_slot = Some(slot + self.min);
        slot.saturating_duration_since(now)
    }
}

/// Capture destinations: `.raw` gets raw socket bytes (mudlib.py
/// compatible); the timing log gets `EPOCH.mmm TAG payload` lines.
#[derive(Debug, Clone)]
pub struct Capture {
    pub raw: PathBuf,
    pub timing: Option<PathBuf>,
}

/// Rolling view of the character, fed from parsed events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameState {
    pub hp: i32,
    pub mana: Option<i32>,
    pub room: Option<RoomView>,
    /// The word the last prompt painted: resting, meditating, or
    /// nothing. Tracked on every prompt, because the board has no
    /// wording for the end of a rest. The prompt is the only signal.
    pub status: Option<crate::events::Status>,
    /// The board's round and regen cycles, inferred from what arrives.
    /// See [`crate::world::TickClock`].
    pub ticks: crate::world::TickClock,
}

#[derive(Debug)]
pub enum ExpectError {
    Timeout { needle: String, tail: String },
    Closed { tail: String },
}

impl std::fmt::Display for ExpectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpectError::Timeout { needle, tail } => {
                write!(f, "timed out waiting for {needle:?}; tail:\n{tail}")
            }
            ExpectError::Closed { tail } => write!(f, "connection closed; tail:\n{tail}"),
        }
    }
}

impl std::error::Error for ExpectError {}

/// Cleaned transcript (CP437-decoded, ANSI-stripped, backspaces
/// applied) plus the expect-consumption cursor.
struct Transcript {
    text: String,
    cursor: usize,
    closed: bool,
}

struct Shared {
    transcript: Mutex<Transcript>,
    /// Bumped on every transcript change (and on close) to wake expects.
    version: watch::Sender<u64>,
}

impl Shared {
    fn append(&self, chunk: &str) {
        let mut tr = self.transcript.lock().expect("transcript lock");
        for c in chunk.chars() {
            if c == '\x08' {
                match tr.text.chars().last() {
                    Some('\n') | Some('\r') | None => {}
                    Some(_) => {
                        tr.text.pop();
                    }
                }
            } else {
                tr.text.push(c);
            }
        }
        let len = tr.text.len();
        tr.cursor = tr.cursor.min(len);
        drop(tr);
        self.version.send_modify(|v| *v += 1);
    }

    fn close(&self) {
        self.transcript.lock().expect("transcript lock").closed = true;
        self.version.send_modify(|v| *v += 1);
    }

    fn tail(&self) -> String {
        let tr = self.transcript.lock().expect("transcript lock");
        let start = tr.text.len().saturating_sub(2000);
        // Snap to a char boundary.
        let start = (start..tr.text.len())
            .find(|&i| tr.text.is_char_boundary(i))
            .unwrap_or(tr.text.len());
        tr.text[start..].to_string()
    }
}

enum Cmd {
    /// Paced game line (CRLF appended), tagged with its correlation id.
    /// `moves` marks a line that walks a command exit — correlated as a
    /// move whatever its wording. See [`Session::send_move`].
    Line {
        id: CmdId,
        line: String,
        moves: bool,
    },
    /// Unpaced raw bytes (telnet negotiation replies, FSD keystrokes).
    /// Excluded from correlation: not line-shaped, never echoed as one.
    Raw(Vec<u8>),
    /// Shut the socket's write side and stop the writer.
    Close,
}

/// How long the correlator waits on an unanswered command before its
/// late answers read as unsolicited. Sliding with queue progress; the
/// absolute cap is 4x this (see `correlate::Correlator`). Generous
/// against the measured worst case (~3s of round-timer wait per queued
/// command), and deliberately LONGER than the navigator's 15s step
/// timeout: with TTL below it, any answer landing in the (TTL,
/// step_timeout] band was guaranteed to arrive unattributed — a
/// systematic loss class, not a tail case.
const CORRELATE_TTL: Duration = Duration::from_secs(20);

struct TimingLog {
    file: Mutex<File>,
}

impl TimingLog {
    fn write(&self, tag: &str, payload: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch");
        let mut f = self.file.lock().expect("timing log lock");
        let _ = writeln!(f, "{}.{:03} {tag} {payload}", now.as_secs(), now.subsec_millis());
        let _ = f.flush();
    }
}

/// The session's own [`PurseMeter`], plus the bookkeeping needed to feed
/// it centrally instead of per-consumer.
///
/// `pending` names the [`CmdId`]s of our own `i` sends, each stamped with
/// when it went out, so the reader task can tell "this echo is the one
/// owed a reply" from "this line just happens to read `i`" — the same
/// distinction [`crate::nav::Navigator`] and `tui.rs` each track locally
/// today, done once here so every caller shares one answer instead of
/// running its own copy that goes stale the moment its `Navigator` is
/// dropped.
///
/// An `i` that never echoes (a dropped connection, a send that landed on
/// a menu that does not know the word) would otherwise sit in here
/// forever — a slow, session-lifetime leak on a long-lived connection.
/// [`feed_purse`] sweeps entries older than [`CORRELATE_TTL`] on every
/// call: that is the same cutoff the correlator itself already uses to
/// decide an unanswered command's late answers are unsolicited, so a
/// pending `i` id and the correlator's own entry for it go stale on
/// exactly the same clock, never two different opinions about when to
/// give up on the same send.
struct PurseTracker {
    meter: PurseMeter,
    pending: HashMap<CmdId, Instant>,
}

/// The session's own [`Stats`], plus the bookkeeping needed to
/// accumulate its multi-line reply centrally instead of per-consumer —
/// [`PurseTracker`]'s pattern, extended for a reply that spans many
/// lines instead of one.
///
/// `pending` names the [`CmdId`]s of our own `stat`/`st` sends, exactly
/// as `PurseTracker::pending` does for `i`: the reader task needs to
/// tell "this echo is the one that arms the accumulator" from "this
/// line just happens to read `stat`". Swept on the same
/// [`CORRELATE_TTL`] clock as `PurseTracker`, for the same reason — a
/// send that never echoes must not leak for the life of the session.
struct StatTracker {
    current: Stats,
    pending: HashMap<CmdId, Instant>,
    /// `Some` from the moment our echo is attributed until a prompt
    /// closes a reply that actually parsed as a sheet; `None` means no
    /// reply is in flight. Every [`Event::Line`] in between is appended
    /// verbatim, interleaved traffic included: [`Stats::parse`] finds
    /// each field by searching the whole text for its own `Label:`, so
    /// a stray line no pattern matches just adds noise — it can never
    /// truncate the sheet or knock a later field out of the parse.
    ///
    /// Not every prompt is ours: when sends pipeline, BOTH receipt
    /// echoes precede the first reply, so the first prompt after arming
    /// can be an earlier command's terminator (measured live 2026-08-26:
    /// the realm-entry `i` ping's reply+prompt landed inside the `stat`
    /// window and the real sheet was discarded — picklocks read 0 on a
    /// Ninja with 28). A prompt that closes a buffer parsing to nothing
    /// clears the foreign reply and keeps waiting; the [`Instant`] is
    /// the arm time, bounding that wait by [`CORRELATE_TTL`].
    buffer: Option<(String, Instant)>,
}

/// The session's own carried-contents reading, kept current by the
/// reader task off whichever `i` reply arrives -- ours or a caller's.
/// [`PurseTracker`]'s pattern, extended for a reply that spans several
/// wrapped lines instead of one: buffered from the echo through the
/// board's own terminator, the `Encumbrance:` line `show_inventory`
/// always prints last (`crates/mud-core/src/game.rs`).
///
/// Distinct from [`PurseTracker`] on purpose. The purse is authoritative
/// -- a `Purse` is a single number the meter can trust completely once
/// it has seen one accepted reply. Contents are not: loot, sales and
/// consumables drift the carried list constantly between asks, so this
/// is deliberately allowed to be stale between refreshes rather than
/// tracked incrementally (`2026-08-22-inventory-and-backstab-design.md`
/// "Architecture — three layers, different truth models").
struct ContentsTracker {
    current: Inventory,
    pending: HashMap<CmdId, Instant>,
    /// `Some` from the echo until the `Encumbrance:` terminator; `None`
    /// means no reply is in flight. Same shape as [`StatTracker::buffer`]
    /// and the same reasoning: interleaved traffic just adds noise that
    /// [`Inventory::parse`] will not recognise as any of its own fields.
    buffer: Option<String>,
}

/// The character's inventory and spellbook, read once at realm entry —
/// see [`crate::farm::probe_sheet`] and [`Session::set_sheet`] — and
/// held here so `/go`, a farm start, and the dark-finish walk stop
/// asking the board the same two questions on every call
/// (`crate::farm::sheet_from` reads it back).
///
/// Character-intrinsic data ONLY: never a caller's bot config (which
/// heal marks or buffs to keep up), which still varies per farm profile
/// and gets applied fresh, in memory, by whoever calls `sheet_from` — a
/// session-wide cache of the FULL `Sheet` would have frozen that choice
/// to whichever caller happened to probe first.
#[derive(Debug, Clone, Default)]
struct RawSheet {
    inventory: Inventory,
    book: Spellbook,
    casting: Casting,
    /// Whether [`Session::set_sheet`] has stored a reading yet. `false`
    /// until the realm entry probe lands, so [`Session::book_len`] can
    /// tell an unread sheet apart from a genuinely empty book.
    read: bool,
}

/// A yes/no shared between the terminal and a job running on the same
/// session, read by the job at each decision rather than copied once
/// at launch. Cloning shares the value.
#[derive(Clone, Debug, Default)]
pub struct Switch(Arc<std::sync::atomic::AtomicBool>);

impl Switch {
    pub fn new(on: bool) -> Self {
        Switch(Arc::new(std::sync::atomic::AtomicBool::new(on)))
    }

    pub fn get(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set(&self, on: bool) {
        self.0.store(on, std::sync::atomic::Ordering::Relaxed)
    }
}

pub struct Session {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
    /// `None` once `close` has dropped it. Held behind a lock rather
    /// than as a bare `Sender` because a broadcast channel only reports
    /// `Closed` to its receivers once every sender clone is gone. The
    /// reader task's own clone dies with it on abort. This one is kept
    /// so `events` can mint fresh subscribers for the whole life of the
    /// session, and would otherwise outlive `close` and keep the stream
    /// open.
    events_tx: Mutex<Option<broadcast::Sender<Correlated>>>,
    /// Same reasoning as `events_tx`.
    raw_tx: Mutex<Option<broadcast::Sender<Vec<u8>>>>,
    /// Why this client rested, one line per rest it sent. Jobs and the
    /// assist explain here, and the window logs the line in the lobby.
    /// Same reasoning as `events_tx`.
    rests_tx: Mutex<Option<broadcast::Sender<String>>>,
    state_rx: watch::Receiver<GameState>,
    /// The profile this session runs under. A watch channel, because a
    /// `/set` replacing it is the event a running job reloads on.
    profile: watch::Sender<Profile>,
    /// The reader task, so `close` can stop it. `TcpStream::into_split`
    /// gives each half its own handle to the same socket. The fd is only
    /// actually released once both halves have dropped, so the reader
    /// must stop and drop its read half too, not just the writer.
    reader: tokio::task::AbortHandle,
    next_id: AtomicU64,
    /// Current send-pacing interval in ms, shared with the writer task.
    /// See [`Session::set_pace`].
    pace_ms: Arc<AtomicU64>,
    /// Whether a walk on this session stops to fight what it meets.
    /// Seeded by the runner from its config when a job starts and
    /// flipped by the `/bot` toggle while it runs. See
    /// [`Session::travel_fights`].
    travel_fights: Switch,
    /// The master switch over every automatic policy, `/bot`. See
    /// [`Session::bot_switch`].
    bot: watch::Sender<bool>,
    /// The session's own running balance, kept current by the reader
    /// task off whichever `i` reply arrives -- ours or a caller's.
    purse: Arc<Mutex<PurseTracker>>,
    /// The session's own `stat`-sheet reading, kept current by the reader
    /// task off whichever `stat`/`st` reply arrives -- ours or a
    /// caller's. See [`StatTracker`].
    stats: Arc<Mutex<StatTracker>>,
    /// Which `(room, direction)` toll crossings this session has
    /// personally measured free. `Arc` so every [`Navigator`] built over
    /// this session's lifetime shares the same memory: a fact learned on
    /// one leg (a farm stop, a `/go`) must still be known on the next.
    ///
    /// [`Navigator`]: crate::nav::Navigator
    toll_log: Arc<TollLog>,
    /// The session's own inventory and spellbook. NOT the `stat` sheet
    /// above — a different "sheet" ([`crate::farm::Sheet`]), read once at
    /// realm entry rather than fed continuously. See [`RawSheet`].
    sheet: Arc<Mutex<RawSheet>>,
    /// The session's own carried-contents reading, kept current by the
    /// reader task off whichever `i` reply arrives -- ours or a caller's.
    /// See [`ContentsTracker`]. Unlike `sheet` above, this is refreshed
    /// on EVERY `i`, not read once.
    contents: Arc<Mutex<ContentsTracker>>,
    /// The session's own wielded-weapon model. Updated two ways: any
    /// `"You are now holding <new>."` line, wherever it lands in the
    /// stream (see [`feed_equipment`]), and a one-time seed off the
    /// first `i`/`inventory` listing this session sees -- see
    /// [`Equipment::seed`], called from both [`feed_contents`] (the
    /// lowercase `i` a caller sends) and [`Session::set_sheet`]
    /// (`crate::farm::probe_sheet`'s `inventory`, the realm-entry
    /// primer every caller -- TUI and headless `mmc farm`/`mmc go`
    /// alike -- actually runs). `Arc` for the same reason `toll_log`
    /// is: every [`Navigator`] built over this session's lifetime needs
    /// the same, current belief.
    ///
    /// [`Navigator`]: crate::nav::Navigator
    equipment: Arc<Mutex<Equipment>>,
    /// The character's pack by item id, refreshed off every `i` reply
    /// once a caller has handed over the item table with
    /// [`Session::set_content`]. `None` before that: the session cannot
    /// resolve names on its own, and it does not load the world
    /// database on its own account.
    pack: Arc<Mutex<Option<crate::pack::PackHandle>>>,
}

impl Session {
    pub async fn connect(profile: &Profile, capture: Option<Capture>) -> std::io::Result<Session> {
        let stream = TcpStream::connect((profile.host.as_str(), profile.port)).await?;
        let (mut read_half, mut write_half) = stream.into_split();

        let shared = Arc::new(Shared {
            transcript: Mutex::new(Transcript {
                text: String::new(),
                cursor: 0,
                closed: false,
            }),
            version: watch::Sender::new(0),
        });
        let (events_tx, _) = broadcast::channel(8192);
        let (raw_tx, _) = broadcast::channel(8192);
        let (rests_tx, _) = broadcast::channel(64);
        let (state_tx, state_rx) = watch::channel(GameState::default());
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();
        let purse = Arc::new(Mutex::new(PurseTracker {
            meter: PurseMeter::default(),
            pending: HashMap::new(),
        }));
        let stats = Arc::new(Mutex::new(StatTracker {
            current: Stats::default(),
            pending: HashMap::new(),
            buffer: None,
        }));
        let toll_log = Arc::new(TollLog::default());
        let sheet = Arc::new(Mutex::new(RawSheet::default()));
        let contents = Arc::new(Mutex::new(ContentsTracker {
            current: Inventory::default(),
            pending: HashMap::new(),
            buffer: None,
        }));
        let equipment = Arc::new(Mutex::new(Equipment::new()));
        let pack = Arc::new(Mutex::new(None));

        let mut raw_file = match &capture {
            Some(c) => Some(File::create(&c.raw)?),
            None => None,
        };
        let timing = match capture.as_ref().and_then(|c| c.timing.as_ref()) {
            Some(p) => {
                let log = Arc::new(TimingLog {
                    file: Mutex::new(File::create(p)?),
                });
                log.write("!!", "session connected");
                Some(log)
            }
            None => None,
        };

        let correlator = Arc::new(Mutex::new(Correlator::new(CORRELATE_TTL)));

        // Writer task: paced lines + unpaced negotiation replies.
        let pace_ms = Arc::new(AtomicU64::new(profile.pace().as_millis() as u64));
        {
            let pace_ms = Arc::clone(&pace_ms);
            let timing = timing.clone();
            let correlator = Arc::clone(&correlator);
            tokio::spawn(async move {
                let mut pacer = Pacer::new(Duration::ZERO);
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Cmd::Line { id, line, moves } => {
                            // Re-read per send: `set_pace` retunes a live
                            // session when `/farm` takes it over or hands
                            // it back.
                            pacer.set_min(Duration::from_millis(
                                pace_ms.load(std::sync::atomic::Ordering::Relaxed),
                            ));
                            let delay = pacer.delay_for(Instant::now());
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            // Register BEFORE the bytes hit the wire:
                            // registry order provably equals wire order
                            // (this task is the only writer), and the
                            // entry exists before its echo can possibly
                            // arrive.
                            {
                                let mut cor = correlator.lock().expect("correlator lock");
                                match moves {
                                    true => cor.sent_move(id, &line, Instant::now()),
                                    false => cor.sent(id, &line, Instant::now()),
                                }
                            }
                            let mut bytes = line.clone().into_bytes();
                            bytes.extend_from_slice(b"\r\n");
                            if write_half.write_all(&bytes).await.is_err() {
                                break;
                            }
                            let _ = write_half.flush().await;
                            if let Some(t) = &timing {
                                t.write("TX", &line);
                            }
                        }
                        Cmd::Raw(bytes) => {
                            if write_half.write_all(&bytes).await.is_err() {
                                break;
                            }
                            let _ = write_half.flush().await;
                        }
                        Cmd::Close => {
                            use tokio::io::AsyncWriteExt as _;
                            let _ = write_half.shutdown().await;
                            break;
                        }
                    }
                }
            });
        }

        // Reader task: telnet -> capture -> raw broadcast -> parser ->
        // events/state; stripped text -> transcript + timing RX lines.
        let reader = {
            let shared = Arc::clone(&shared);
            let events_tx = events_tx.clone();
            let raw_tx = raw_tx.clone();
            let cmd_tx = cmd_tx.clone();
            let correlator = Arc::clone(&correlator);
            let purse = Arc::clone(&purse);
            let stats = Arc::clone(&stats);
            let contents = Arc::clone(&contents);
            let equipment = Arc::clone(&equipment);
            let pack = Arc::clone(&pack);
            tokio::spawn(async move {
                let mut filter = TelnetFilter::new();
                let mut stripper = AnsiStripper::new();
                let mut parser = Parser::new();
                let mut rx_line = String::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = match read_half.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if let Some(f) = &mut raw_file {
                        let _ = f.write_all(&buf[..n]);
                        let _ = f.flush();
                    }
                    let out = filter.push(&buf[..n]);
                    if !out.replies.is_empty() {
                        let _ = cmd_tx.send(Cmd::Raw(out.replies));
                    }
                    if out.data.is_empty() {
                        continue;
                    }
                    let _ = raw_tx.send(out.data.clone());
                    let decoded = cp437_to_string(&out.data);
                    let batch = parser.push(&decoded);
                    if !batch.is_empty() {
                        // One lock for the whole chunk: with per-event
                        // acquisitions the writer could register a
                        // same-text command BETWEEN an execution echo
                        // and its block, and the echo would falsely
                        // accept the brand-new entry. Nothing in here
                        // awaits.
                        let mut guard = correlator.lock().expect("correlator lock");
                        for ev in batch {
                            let now = Instant::now();
                            let cor = guard.on_event(ev, now);
                            state_tx.send_if_modified(|s| apply_event(s, &cor, now));
                            feed_purse(&purse, &cor);
                            feed_stats(&stats, &cor);
                            feed_contents(&contents, &equipment, &pack, &cor);
                            feed_equipment(&equipment, &cor);
                            let _ = events_tx.send(cor);
                        }
                    }
                    let stripped = stripper.push(&decoded);
                    if let Some(t) = &timing {
                        rx_line.push_str(&stripped);
                        while let Some(nl) = rx_line.find('\n') {
                            let line: String = rx_line.drain(..=nl).collect();
                            t.write("RX", line.trim_end_matches(['\r', '\n']));
                        }
                    }
                    shared.append(&stripped);
                }
                let tail = parser.finish();
                if !tail.is_empty() {
                    let mut guard = correlator.lock().expect("correlator lock");
                    for ev in tail {
                        let now = Instant::now();
                        let cor = guard.on_event(ev, now);
                        state_tx.send_if_modified(|s| apply_event(s, &cor, now));
                        feed_purse(&purse, &cor);
                        feed_stats(&stats, &cor);
                        feed_contents(&contents, &equipment, &pack, &cor);
                        feed_equipment(&equipment, &cor);
                        let _ = events_tx.send(cor);
                    }
                }
                if let (Some(t), false) = (&timing, rx_line.is_empty()) {
                    t.write("RX", rx_line.trim_end_matches(['\r', '\n']));
                }
                shared.close();
            })
        };

        Ok(Session {
            cmd_tx,
            shared,
            events_tx: Mutex::new(Some(events_tx)),
            raw_tx: Mutex::new(Some(raw_tx)),
            rests_tx: Mutex::new(Some(rests_tx)),
            state_rx,
            profile: watch::Sender::new(profile.clone()),
            reader: reader.abort_handle(),
            next_id: AtomicU64::new(1),
            pace_ms,
            travel_fights: Switch::default(),
            bot: watch::Sender::new(true),
            purse,
            stats,
            toll_log,
            sheet,
            contents,
            equipment,
            pack,
        })
    }

    /// The profile this session runs under. A clone: `/set` replaces it
    /// with [`Session::set_profile`], and a job reads it when it starts
    /// and again through [`Session::profile_changes`] while it runs.
    pub fn profile(&self) -> Profile {
        self.profile.borrow().clone()
    }

    /// Replace the profile. Every job holding a receiver from
    /// [`Session::profile_changes`] wakes.
    pub fn set_profile(&self, profile: Profile) {
        self.profile.send_replace(profile);
    }

    /// The event a job reloads its settings on. The receiver reports
    /// the current profile on `borrow` and wakes on every replacement.
    pub fn profile_changes(&self) -> watch::Receiver<Profile> {
        self.profile.subscribe()
    }

    /// Close the line: the writer shuts the socket's write side, the
    /// reader stops so the read half drops, and every pending expect
    /// wakes with the closed error. Idempotent.
    ///
    /// Also drops this session's own `events_tx`/`raw_tx` clones, not
    /// just the reader task's. A broadcast channel only reports `Closed`
    /// to its subscribers once every sender is gone, and these two
    /// outlive the reader by design. See their doc on [`Session`] for why.
    ///
    /// Returns before the reader task has actually stopped: the abort is
    /// only requested here, not waited on. A caller sees the streams end
    /// on its next await rather than immediately after this call returns.
    pub fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
        self.reader.abort();
        self.events_tx.lock().expect("events_tx lock").take();
        self.raw_tx.lock().expect("raw_tx lock").take();
        self.rests_tx.lock().expect("rests_tx lock").take();
        self.shared.close();
    }

    /// Retune send pacing on a live session. Takes effect from the next
    /// queued line. Pacing is flood control, which is for automation:
    /// `mmc play` runs a person's keystrokes unpaced, and `/farm` puts
    /// the same connection back under the profile's pace while the
    /// runner drives — the unpaced TUI farm is what cycled the gate at
    /// loopback echo speed (~40 commands in 400ms, run4 2026-08-01).
    pub fn set_pace(&self, pace: Duration) {
        self.pace_ms
            .store(pace.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// The live "fight while travelling" switch. A walk's travel guard
    /// reads it at every sighting, entry and blow. The jobs set it from
    /// their live settings at entry and on every reload
    /// ([`crate::farm::fights_on_the_way`]), so a `/set` or a `/bot`
    /// changes a walk already in progress.
    pub fn travel_fights(&self) -> &Switch {
        &self.travel_fights
    }

    /// Whether `/bot` is on. Off means nothing automatic runs in any
    /// mode: no fighting, healing, resting, looting, sneaking, fleeing
    /// or buffing, and no health gate on a walk's departures. On means
    /// what the profile's `[bot]` table turned on runs. On until the
    /// window says otherwise, so the headless commands are unaffected.
    pub fn bot_on(&self) -> bool {
        *self.bot.borrow()
    }

    /// Flip `/bot`. Every job on the session hears it through
    /// [`Session::bot_switch`] at its next decision.
    pub fn set_bot_on(&self, on: bool) {
        self.bot.send_replace(on);
    }

    /// A receiver on the `/bot` switch, for [`crate::live::Live`].
    pub fn bot_switch(&self) -> watch::Receiver<bool> {
        self.bot.subscribe()
    }

    /// Queue a line for sending (CRLF appended); pacing applies. The
    /// returned id names this send in every event that answers it —
    /// match it against [`Correlated::answers`] by EQUALITY only: id
    /// numeric order equals wire order per sender, not across tasks.
    ///
    /// Two contract notes for waiters: the id returns immediately but
    /// the bytes leave after the pacing backlog, so a wait on `answers`
    /// must budget for pacing it cannot observe; and a `SlowDown` resend
    /// (the Gate re-queues its in-flight command) mints a NEW id — the
    /// flushed original will never answer.
    ///
    /// Trimmed at this boundary: the TUI and scripts hand over raw
    /// lines, and a trailing space must not defeat echo matching.
    pub fn send(&self, line: &str) -> CmdId {
        let id = CmdId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let trimmed = line.trim();
        // Name this id as an inventory ask BEFORE it goes out, mirroring
        // `sent`'s own registration-before-write rule: the reader task
        // must never be able to see the echo before it knows to expect
        // one. `i` is the board's own inventory verb — see
        // `PurseMeter::expect_reply` for why the echo, not the reply, is
        // what arms the meter.
        if trimmed.eq_ignore_ascii_case("i") {
            self.purse
                .lock()
                .expect("purse lock")
                .pending
                .insert(id, Instant::now());
            self.contents
                .lock()
                .expect("contents lock")
                .pending
                .insert(id, Instant::now());
        }
        // Same rule, same reason, for the character sheet: `stat` and
        // its minimum abbreviation `st` (`mud-core`'s `ALIASES` table)
        // both resolve to `Command::Status`. See [`StatTracker`].
        if trimmed.eq_ignore_ascii_case("stat") || trimmed.eq_ignore_ascii_case("st") {
            self.stats
                .lock()
                .expect("stats lock")
                .pending
                .insert(id, Instant::now());
        }
        let _ = self.cmd_tx.send(Cmd::Line {
            id,
            line: trimmed.to_string(),
            moves: false,
        });
        id
    }

    /// Send a line that moves the character even though it is not a
    /// direction word — the phrase that walks a **command exit**
    /// (`borrow skiff`, `go manhole`, `climb tree`).
    ///
    /// Identical to [`Session::send`] on the wire; the only difference
    /// is that the correlator files it as a move, so the arriving room
    /// block answers it. See [`crate::correlate::Correlator::sent_move`]
    /// for why this is the caller's assertion and not a string test.
    pub fn send_move(&self, line: &str) -> CmdId {
        let id = CmdId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let _ = self.cmd_tx.send(Cmd::Line {
            id,
            line: line.trim().to_string(),
            moves: true,
        });
        id
    }

    /// Send bytes verbatim: no CRLF, no pacing, no timing-log line.
    ///
    /// For driving the board's full-screen data-entry screens, which are
    /// keystroke-oriented rather than line-oriented — an FSD room reads
    /// `\x1b[A` to move a field, and a line send would append CRLF and be
    /// read as an extra key.
    pub fn send_raw(&self, bytes: &[u8]) {
        let _ = self.cmd_tx.send(Cmd::Raw(bytes.to_vec()));
    }

    /// Wait until `needle` appears in the cleaned transcript past the
    /// expect cursor; consumes through the end of the match.
    pub async fn expect(&self, needle: &str, timeout: Duration) -> Result<(), ExpectError> {
        self.expect_any(&[needle], timeout).await.map(|_| ())
    }

    /// Wait for the first of `needles` (earliest match position wins);
    /// returns its index and consumes through the end of that match.
    pub async fn expect_any(
        &self,
        needles: &[&str],
        timeout: Duration,
    ) -> Result<usize, ExpectError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut version = self.shared.version.subscribe();
        loop {
            {
                let mut tr = self.shared.transcript.lock().expect("transcript lock");
                let hay = &tr.text[tr.cursor..];
                let best = needles
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| hay.find(n).map(|pos| (pos, pos + n.len(), i)))
                    .min();
                if let Some((_, end, idx)) = best {
                    tr.cursor += end;
                    return Ok(idx);
                }
                if tr.closed {
                    drop(tr);
                    return Err(ExpectError::Closed {
                        tail: self.shared.tail(),
                    });
                }
            }
            match tokio::time::timeout_at(deadline, version.changed()).await {
                Err(_) => {
                    return Err(ExpectError::Timeout {
                        needle: needles.join("|"),
                        tail: self.shared.tail(),
                    });
                }
                Ok(Err(_)) => {
                    return Err(ExpectError::Closed {
                        tail: self.shared.tail(),
                    });
                }
                Ok(Ok(())) => {}
            }
        }
    }

    /// Current end of the cleaned transcript.
    pub fn mark(&self) -> usize {
        self.shared.transcript.lock().expect("transcript lock").text.len()
    }

    /// Cleaned transcript text captured since `mark`.
    pub fn since(&self, mark: usize) -> String {
        let tr = self.shared.transcript.lock().expect("transcript lock");
        let start = mark.min(tr.text.len());
        let start = (start..tr.text.len())
            .find(|&i| tr.text.is_char_boundary(i))
            .unwrap_or(tr.text.len());
        tr.text[start..].to_string()
    }

    /// A fresh subscriber. After `close`, `events_tx` is gone: this
    /// hands back a receiver over a channel whose sender was dropped on
    /// the spot, so it reads as already closed rather than panicking.
    pub fn events(&self) -> broadcast::Receiver<Correlated> {
        self.events_tx
            .lock()
            .expect("events_tx lock")
            .as_ref()
            .map(|tx| tx.subscribe())
            .unwrap_or_else(|| broadcast::channel(1).1)
    }

    /// Raw post-telnet bytes, for terminal passthrough. Same closed-after-
    /// `close` behaviour as [`Session::events`].
    pub fn raw(&self) -> broadcast::Receiver<Vec<u8>> {
        self.raw_tx
            .lock()
            .expect("raw_tx lock")
            .as_ref()
            .map(|tx| tx.subscribe())
            .unwrap_or_else(|| broadcast::channel(1).1)
    }

    /// Why the client rested, one line per rest. Same closed-after-
    /// `close` behaviour as [`Session::events`].
    pub fn rests(&self) -> broadcast::Receiver<String> {
        self.rests_tx
            .lock()
            .expect("rests_tx lock")
            .as_ref()
            .map(|tx| tx.subscribe())
            .unwrap_or_else(|| broadcast::channel(1).1)
    }

    /// Explain a rest this client is sending. Every path that sends a
    /// rest or a meditate says why here, in the same breath as the
    /// send, so the lobby's log can answer "why is it resting". Nobody
    /// listening is fine: a rest needs no witness.
    pub fn report_rest(&self, why: String) {
        if let Some(tx) = self.rests_tx.lock().expect("rests_tx lock").as_ref() {
            let _ = tx.send(why);
        }
    }

    pub fn state(&self) -> watch::Receiver<GameState> {
        self.state_rx.clone()
    }

    /// What the walker can currently bring to bear, as this session knows it.
    ///
    /// The toll log is shared by Arc deliberately: a fact learned on one
    /// leg must outlive the `Navigator` that learned it, or the walk
    /// relearns (and re-pays) the same toll on every crossing. The purse
    /// is a snapshot of this session's own [`PurseMeter`], kept current
    /// off the event stream by whichever `i` a caller (or this session's
    /// own navigators) has already sent — never sent on `capabilities`'
    /// own account, so asking costs nothing and can go stale between real
    /// inventory checks. `picklocks` is the same idea off `stats()`:
    /// whether this character can pick a lock is a fact read off the
    /// board's own `stat` sheet, not a setting an operator manages.
    /// `stealth` follows the identical pattern for the Stealth skill.
    /// The pack is the shared handle from `set_content`, or `None` until
    /// then. `bash_doors` is off, the navigator that walks for this
    /// session sets it from its own config. `every_item` is off, this
    /// is one character rather than the map.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            purse: self.purse.lock().expect("purse lock").meter.current(),
            tolls_known_free: Arc::clone(&self.toll_log),
            picklocks: self.stats().picklocks.unwrap_or(0),
            stealth: self.stats().stealth.unwrap_or(0),
            pack: self.pack_handle(),
            bash_doors: false,
            every_item: false,
            level: self.stats().level,
            route: crate::graph::RouteMode::Short,
        }
    }

    /// The session's own [`Stats`], as of the most recent `stat`/`st`
    /// reply this session (or any caller sharing it) has read off the
    /// board. `Stats::default()` — every field `None` — until the first
    /// one arrives; nothing here sends anything on its own account, same
    /// as [`Session::capabilities`]'s purse.
    pub fn stats(&self) -> Stats {
        self.stats.lock().expect("stats lock").current.clone()
    }

    /// The character's name as the board printed it on the stat sheet, or
    /// the profile's `username` before any sheet was read. `None` when
    /// neither is known, which is the one state the automation cannot run
    /// in. Nothing would recognise the character's own death line.
    ///
    /// The sheet wins. A board account and the character on it are
    /// different names, and only the character dies.
    pub fn character_name(&self) -> Option<String> {
        if let Some(name) = self.stats().name.filter(|n| !n.is_empty()) {
            return Some(name);
        }
        let username = self.profile().username;
        if username.is_empty() {
            return None;
        }
        Some(username)
    }

    /// The session's own carried-contents reading, as of the most recent
    /// `i` reply this session (or any caller sharing it) has read off the
    /// board. [`Inventory::default`] -- an empty pack -- until the first
    /// one arrives; nothing here sends anything on its own account, same
    /// as [`Session::stats`]. Best-effort by design: loot, sales and
    /// consumables drift the real contents between asks, and this is
    /// only ever as fresh as the last `i` anyone on this session sent.
    pub fn contents(&self) -> Inventory {
        self.contents.lock().expect("contents lock").current.clone()
    }

    /// Hand the session the item table so it can keep a pack by id.
    ///
    /// Called once by whoever loads the world database for this
    /// session: `mmc play` at realm entry, `run_go` and `run_farm` at
    /// their start. Resolves the reading the session already holds, so
    /// a table handed over after the first `i` still gives a full pack.
    /// A second call swaps the table and re-resolves into the SAME
    /// shared pack (`PackHandle::with_table`), so every earlier holder
    /// -- a bot, a navigator -- sees it too, rather than being left
    /// watching a pack nothing refreshes again.
    pub fn set_content(&self, content: Arc<mud_core::content::Content>) {
        let handle = match self.pack.lock().expect("pack lock").as_ref() {
            Some(h) => h.with_table(content),
            None => crate::pack::PackHandle::new(content),
        };
        handle.refresh(&self.contents());
        *self.pack.lock().expect("pack lock") = Some(handle);
    }

    /// The shared pack, or `None` until [`Session::set_content`].
    pub fn pack_handle(&self) -> Option<crate::pack::PackHandle> {
        self.pack.lock().expect("pack lock").clone()
    }

    /// The content database the realm entry probe stored, if it did.
    /// The pack holds it because the pack was its first reader. Spell
    /// discovery is its second.
    pub fn content(&self) -> Option<Arc<mud_core::content::Content>> {
        self.pack
            .lock()
            .expect("pack lock")
            .as_ref()
            .map(|h| Arc::clone(h.content()))
    }

    /// The session's own wielded-weapon model, as this session has
    /// confirmed (`"You are now holding <new>."`) or seeded it (the
    /// first `i`/`inventory` listing read) -- see [`Equipment`]. `None`
    /// until one of those has happened, or the character is genuinely
    /// unarmed. Best-effort in the same sense [`Session::stats`] and
    /// [`Session::contents`] are: nothing here sends anything on its own
    /// account.
    pub fn wielded(&self) -> Option<String> {
        self.equipment.lock().expect("equipment lock").weapon().map(str::to_string)
    }

    /// Record the character's inventory and spellbook, as read off the
    /// board by [`crate::farm::probe_sheet`]. Called once, at realm
    /// entry; overwrites whatever was cached before — the same "always
    /// an assignment" rule [`PurseMeter`] and [`StatTracker`] already
    /// follow, so a `Session` never holds two different opinions about
    /// what was last actually asked.
    pub fn set_sheet(&self, inventory: Inventory, book: Spellbook, casting: Casting) {
        // The universal realm-entry primer (`crate::farm::probe_sheet`,
        // run by both the TUI and headless `mmc farm`/`mmc go`) is the
        // one call site every caller actually reaches, so it seeds the
        // wielded-weapon model too -- see `Equipment::seed`'s doc for
        // why this only ever takes the first time.
        self.equipment.lock().expect("equipment lock").seed(&inventory.items);
        let mut s = self.sheet.lock().expect("sheet lock");
        s.inventory = inventory;
        s.book = book;
        s.casting = casting;
        s.read = true;
    }

    /// The session's own cached inventory, spellbook, and casting
    /// vocabulary — `Default`s (empty inventory, empty book) until
    /// [`Session::set_sheet`] has been called once. Cloned out rather
    /// than borrowed so a caller can build a [`crate::farm::Sheet`] with
    /// its own bot config (via [`crate::farm::sheet_from`]) without
    /// holding the lock across that work.
    pub fn raw_sheet(&self) -> (Inventory, Spellbook, Casting) {
        let s = self.sheet.lock().expect("sheet lock");
        (s.inventory.clone(), s.book.clone(), s.casting)
    }

    /// How many spells the stored book holds, read under one lock. None
    /// until the realm entry probe has stored a sheet, so a caller can
    /// tell unread from empty.
    pub fn book_len(&self) -> Option<usize> {
        let s = self.sheet.lock().expect("sheet lock");
        s.read.then(|| s.book.spells.len())
    }
}

/// Feed one correlated event to the session's own purse meter: arm it on
/// the echo of one of OUR `i` sends (named in `pending` before the bytes
/// left, same as [`Correlator::sent`]), and otherwise offer the line as
/// the reply owed to whichever ask is still open. [`PurseMeter::observe`]
/// itself decides whether a line offered this way is actually shaped
/// like an inventory reply -- an unattributed line is always a no-op
/// here, and so is an attributed one that does not look like the real
/// answer; either way the expectation stays open for the next line
/// rather than getting consumed by whatever interleaved traffic (a
/// shout, another player's action) happened to land in the gap.
fn feed_purse(purse: &Mutex<PurseTracker>, cor: &Correlated) {
    let Event::Line(line) = &cor.event else {
        return;
    };
    let mut tracker = purse.lock().expect("purse lock");
    // An `i` that never echoed (dropped connection, sent into a menu
    // that never accepted it) is exactly as stale as the correlator's
    // own entry for the same send would be by now — sweep both on the
    // same clock rather than leaking one id per send that never answers.
    let now = Instant::now();
    tracker
        .pending
        .retain(|_, sent_at| now.duration_since(*sent_at) < CORRELATE_TTL);
    let is_our_echo = cor.answers.is_some_and(|id| tracker.pending.remove(&id).is_some());
    if is_our_echo {
        tracker.meter.expect_reply();
    } else {
        tracker.meter.observe(line);
    }
}

/// Feed one correlated event to the session's own [`Stats`]: arm
/// accumulation on the echo of one of OUR `stat`/`st` sends (named in
/// `pending` before the bytes left, same as [`feed_purse`]'s `i`), then
/// append every line that follows into [`StatTracker::buffer`] until the
/// next ordinary game prompt closes the reply out and it gets parsed.
///
/// Unlike [`feed_purse`], this does not need [`Correlated::answers`] to
/// pick out the terminal line — `stat`'s reply has no fixed one (see
/// `Kind::Stat` in `correlate.rs`) — so it watches the raw event shapes
/// instead: any [`Event::Line`] while a reply is in flight is body text
/// (interleaved traffic included — harmless, see [`StatTracker::buffer`]
/// doc). A prompt finalizes only when the buffer parses as a sheet: the
/// first prompt after arming is NOT reliably ours — an earlier queued
/// command's reply and prompt can land first, because receipt echoes
/// for every pipelined send precede the first reply (see
/// [`StatTracker::buffer`]). A prompt over an unparseable buffer closes
/// that foreign reply out of the window and keeps waiting, up to
/// [`CORRELATE_TTL`] from the arm.
fn feed_stats(stats: &Mutex<StatTracker>, cor: &Correlated) {
    let mut tracker = stats.lock().expect("stats lock");
    // Same staleness rule as `feed_purse`, same reason: a `stat` that
    // never echoed must not leak forever.
    let now = Instant::now();
    tracker
        .pending
        .retain(|_, sent_at| now.duration_since(*sent_at) < CORRELATE_TTL);
    match &cor.event {
        Event::Line(line) => {
            let is_our_echo = cor.answers.is_some_and(|id| tracker.pending.remove(&id).is_some());
            if is_our_echo {
                // The echo line itself is not sheet body text.
                tracker.buffer = Some((String::new(), now));
                return;
            }
            if let Some((buf, _)) = &mut tracker.buffer {
                buf.push_str(line);
                buf.push('\n');
            }
        }
        Event::Prompt { .. } => {
            if let Some((buf, armed_at)) = &mut tracker.buffer {
                let parsed = Stats::parse(buf);
                if parsed != Stats::default() {
                    tracker.current = parsed;
                    tracker.buffer = None;
                } else if now.duration_since(*armed_at) >= CORRELATE_TTL {
                    // The sheet never came; stop hoarding traffic.
                    tracker.buffer = None;
                } else {
                    // An earlier command's reply just closed — not ours.
                    buf.clear();
                }
            }
        }
        _ => {}
    }
}

/// Feed one correlated event to the session's own [`Inventory`]: arm
/// accumulation on the echo of one of OUR `i` sends (named in `pending`
/// before the bytes left, same as [`feed_purse`] and [`feed_stats`]),
/// then append every line that follows into [`ContentsTracker::buffer`]
/// until the board's own terminator line closes the reply out.
///
/// Unlike [`feed_stats`], the terminator is not the next prompt: `i`'s
/// reply always ends with `Encumbrance: .../ ...` (`show_inventory`,
/// `crates/mud-core/src/game.rs`), a fixed final line rather than
/// whatever text happens to precede the next `[HP=...]:`. Watching for
/// it directly means a reply is parsed as soon as it is complete rather
/// than waiting on a prompt that might be several lines further out.
fn feed_contents(
    contents: &Mutex<ContentsTracker>,
    equipment: &Mutex<Equipment>,
    pack: &Mutex<Option<crate::pack::PackHandle>>,
    cor: &Correlated,
) {
    let Event::Line(line) = &cor.event else {
        return;
    };
    let mut tracker = contents.lock().expect("contents lock");
    // Same staleness rule as `feed_purse`/`feed_stats`, same reason: an
    // `i` that never echoed must not leak forever.
    let now = Instant::now();
    tracker
        .pending
        .retain(|_, sent_at| now.duration_since(*sent_at) < CORRELATE_TTL);
    let is_our_echo = cor.answers.is_some_and(|id| tracker.pending.remove(&id).is_some());
    if is_our_echo {
        // The echo line itself is not reply body text.
        tracker.buffer = Some(String::new());
        return;
    }
    let Some(buf) = &mut tracker.buffer else {
        return;
    };
    buf.push_str(line);
    buf.push('\n');
    if line.trim_start().starts_with("Encumbrance:") {
        tracker.current = Inventory::parse(&std::mem::take(buf));
        // This is a caller's own `i`, not just ours (see this
        // function's `pending`/echo dance above) -- so it is exactly
        // the "first `i` listing" `Equipment::seed` wants, whichever
        // caller happened to send it.
        equipment.lock().expect("equipment lock").seed(&tracker.current.items);
        // The pack is the same reading by id. Refreshed here, under the
        // contents lock, so a caller that sees the new contents also
        // sees the new pack.
        if let Some(handle) = pack.lock().expect("pack lock").as_ref() {
            handle.refresh(&tracker.current);
        }
        tracker.buffer = None;
    }
}

/// Feed one correlated event to the session's own [`Equipment`]: any
/// [`Event::Line`] is offered to [`Equipment::observe`], which only
/// ever moves off a genuine `"You are now holding <new>."` confirmation
/// -- see that method's own doc for why nothing else can update it.
/// Unlike purse/stats/contents this needs no `pending`/echo dance: an
/// equip confirmation is unambiguous on its own wording, wherever in
/// the stream it lands -- including mid-[`feed_contents`]'s own
/// buffered `i` reply, since a swap issued while a listing is still in
/// flight is exactly what [`crate::nav::Navigator::goto`]'s decide ->
/// swap -> sneak -> move step does.
fn feed_equipment(equipment: &Mutex<Equipment>, cor: &Correlated) {
    let Event::Line(line) = &cor.event else {
        return;
    };
    equipment.lock().expect("equipment lock").observe(line);
}

/// Throw away everything already queued on an event receiver.
///
/// `Lagged` is not `Empty`: tokio drops the oldest messages, reports the
/// overflow once, and then hands back the oldest message still retained.
/// So a drain written `while try_recv().is_ok() {}` stops at the lag with
/// stale events still queued — and a stale room block is exactly what
/// [`crate::nav::Navigator::goto`] drains to prevent, since one can
/// satisfy the next step's arrival check and drift the position silently.
/// Every event is shown to `seen` on its way out. Discarded is not the
/// same as unseen: [`crate::nav::Navigator::goto`] drains between steps,
/// and a death landing in that window is still a death.
pub fn drain(events: &mut broadcast::Receiver<Correlated>, mut seen: impl FnMut(&Correlated)) {
    use broadcast::error::TryRecvError;
    loop {
        match events.try_recv() {
            Ok(ev) => seen(&ev),
            Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty | TryRecvError::Closed) => return,
        }
    }
}

/// Fold an event into the rolling state; returns whether it changed.
fn apply_event(state: &mut GameState, cor: &Correlated, now: Instant) -> bool {
    let ticks_before = state.ticks.clone();
    state.ticks.on_event(cor, now);
    let ticked = state.ticks != ticks_before;
    let changed = match &cor.event {
        Event::Prompt { hp, mana, status } => {
            let changed =
                state.hp != *hp || state.mana != *mana || state.status != *status;
            state.hp = *hp;
            state.mana = *mana;
            state.status = status.clone();
            changed
        }
        Event::RoomSeen(room) => {
            // The block a `look <direction>` answers describes the room
            // NEXT DOOR. `state.room` means the room the character is
            // standing in — it feeds position tracking and the occupant
            // list — so adopting a peek would both walk the client's
            // position and report the neighbour's occupants as present.
            if cor.elsewhere {
                return ticked;
            }
            state.room = Some(room.clone());
            true
        }
        _ => false,
    };
    changed || ticked
}

