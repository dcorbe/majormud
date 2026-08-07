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
use crate::events::{Event, RoomView};
use crate::parse::Parser;
use crate::profile::Profile;
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

pub struct Session {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
    events_tx: broadcast::Sender<Correlated>,
    raw_tx: broadcast::Sender<Vec<u8>>,
    state_rx: watch::Receiver<GameState>,
    profile: Profile,
    next_id: AtomicU64,
    /// Current send-pacing interval in ms, shared with the writer task.
    /// See [`Session::set_pace`].
    pace_ms: Arc<AtomicU64>,
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
        let (state_tx, state_rx) = watch::channel(GameState::default());
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();

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
                    }
                }
            });
        }

        // Reader task: telnet -> capture -> raw broadcast -> parser ->
        // events/state; stripped text -> transcript + timing RX lines.
        {
            let shared = Arc::clone(&shared);
            let events_tx = events_tx.clone();
            let raw_tx = raw_tx.clone();
            let cmd_tx = cmd_tx.clone();
            let correlator = Arc::clone(&correlator);
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
                            let cor = guard.on_event(ev, Instant::now());
                            state_tx.send_if_modified(|s| apply_event(s, &cor));
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
                        let cor = guard.on_event(ev, Instant::now());
                        state_tx.send_if_modified(|s| apply_event(s, &cor));
                        let _ = events_tx.send(cor);
                    }
                }
                if let (Some(t), false) = (&timing, rx_line.is_empty()) {
                    t.write("RX", rx_line.trim_end_matches(['\r', '\n']));
                }
                shared.close();
            });
        }

        Ok(Session {
            cmd_tx,
            shared,
            events_tx,
            raw_tx,
            state_rx,
            profile: profile.clone(),
            next_id: AtomicU64::new(1),
            pace_ms,
        })
    }

    /// The profile this session was opened with.
    pub fn profile(&self) -> &Profile {
        &self.profile
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
        let _ = self.cmd_tx.send(Cmd::Line {
            id,
            line: line.trim().to_string(),
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

    pub fn events(&self) -> broadcast::Receiver<Correlated> {
        self.events_tx.subscribe()
    }

    /// Raw post-telnet bytes, for terminal passthrough.
    pub fn raw(&self) -> broadcast::Receiver<Vec<u8>> {
        self.raw_tx.subscribe()
    }

    pub fn state(&self) -> watch::Receiver<GameState> {
        self.state_rx.clone()
    }
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
fn apply_event(state: &mut GameState, cor: &Correlated) -> bool {
    match &cor.event {
        Event::Prompt { hp, mana } => {
            let changed = state.hp != *hp || state.mana != *mana;
            state.hp = *hp;
            state.mana = *mana;
            changed
        }
        Event::RoomSeen(room) => {
            // The block a `look <direction>` answers describes the room
            // NEXT DOOR. `state.room` means the room the character is
            // standing in — it feeds position tracking and the occupant
            // list — so adopting a peek would both walk the client's
            // position and report the neighbour's occupants as present.
            if cor.elsewhere {
                return false;
            }
            state.room = Some(room.clone());
            true
        }
        _ => false,
    }
}
