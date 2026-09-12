//! A window: one character's settings, session and screen, run as a
//! task the front end talks to over channels.
//!
//! The play loop lives here. It is the loop `tui::play` used to be, with
//! three substitutions: keystrokes arrive as parsed outcomes on `msgs`,
//! terminal writes go into the window's [`Screen`], and major events go
//! up to the front end for the lobby's log and the activity list.

use std::sync::{Arc, Mutex};

use crossterm::event::Event as TermEvent;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::events::Status;
use crate::screen::Screen;
use crate::session::{Capture, Session};
use crate::settings::Settings;
use crate::tui::{
    ContentCache, Job, KeyOutcome, LEVEL_POLL, LobbyStep, apply_settings, assist_config_for,
    assist_tick, content_path, describe_loops, finish_locator, follower_bank_arrival_decision,
    handover_actions, help_text, here_or, import_loop, lobby_step, needs_name, new_assist,
    on_realm_entry, pace_on_change, render_status, start_bank, start_farm, start_follower_deposit,
    start_go, start_recover, start_roam, start_where,
};

/// A window's identity. Stable for its life, unlike its number, which
/// moves when a lower window closes.
pub type WindowId = u64;

/// What the front end sends a window.
pub enum WindowMsg {
    /// A keystroke, already parsed by the shared editor.
    Outcome(KeyOutcome),
    /// The terminal changed size. The window's screen is two rows shorter.
    Resize { rows: u16, cols: u16 },
}

/// A major event. The lobby logs it and the bar lights the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Connected,
    Disconnected,
    Died,
    JobEnded(String),
    AssistRefused(String),
    /// A profile with `reconnect` on is dialling the board again.
    /// `attempt` counts from one and resets once a session plays.
    Reconnecting { attempt: u32 },
    /// The character is resting, and why. One line for every rest this
    /// client sends, from whichever job or the assist sent it, and one
    /// each time the prompt shows a recovery, so a rest nobody here
    /// asked for stands out as a prompt line with no send before it.
    Rest(String),
}

/// What a window sends the front end.
#[derive(Debug)]
pub enum FrontMsg {
    /// The screen or the bar changed. Repaint when this is the active
    /// window. `screen` is false when only the bar moved, which lets the
    /// front end keep the snapshot it already has.
    Changed { window: WindowId, screen: bool },
    Event { window: WindowId, kind: EventKind },
    /// The map view is driving the terminal itself. While `on`, the front
    /// end stops painting and forwards raw keys.
    Takeover { window: WindowId, on: bool },
    /// The task ended, because its channel closed.
    Ended { window: WindowId },
}

/// What the front end reads about a window without asking its task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowInfo {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connected: bool,
    pub dirty: bool,
    /// The bar text the window would show, without the number and the
    /// activity list the front end adds.
    pub bar: String,
    /// A job is driving: Ctrl-F and Ctrl-Q are reserved.
    pub farming: bool,
    /// Ctrl-P passthrough, per window. Owned by the front end.
    pub passthrough: bool,
}

/// The connection facts a settings document names, in one place
/// because `spawn` primes a window's info with them and `sync_info`
/// keeps them current.
fn facts(settings: &Settings) -> (String, u16, String, bool) {
    let profile = settings.profile();
    (
        profile.host.clone(),
        profile.port,
        profile.username.clone(),
        settings.dirty(),
    )
}

/// The bar a window shows with no session: where it would connect, and
/// whether its settings are unsaved. A window that has just been hung up
/// on would otherwise keep the last bar its session painted, which reads
/// as a live character on `HP 0`.
fn idle_bar(settings: &Settings) -> String {
    let profile = settings.profile();
    let unsaved = if settings.dirty() { "  unsaved" } else { "" };
    format!("not connected  {}:{}{unsaved}", profile.host, profile.port)
}

pub struct WindowHandle {
    pub id: WindowId,
    pub msgs: UnboundedSender<WindowMsg>,
    /// Raw keys, fed only while the map view has the terminal.
    pub keys: UnboundedSender<TermEvent>,
    pub screen: Arc<Mutex<Screen>>,
    pub info: Arc<Mutex<WindowInfo>>,
}

/// The task's side.
struct Window {
    id: WindowId,
    msgs: UnboundedReceiver<WindowMsg>,
    keys: UnboundedReceiver<TermEvent>,
    screen: Arc<Mutex<Screen>>,
    info: Arc<Mutex<WindowInfo>>,
    front: UnboundedSender<FrontMsg>,
    settings: Settings,
    cache: Arc<Mutex<ContentCache>>,
    capture: Option<Capture>,
    /// Terminal size, for the bar's width and the screen's height.
    rows: u16,
    cols: u16,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    id: WindowId,
    settings: Settings,
    rows: u16,
    cols: u16,
    cache: Arc<Mutex<ContentCache>>,
    front: UnboundedSender<FrontMsg>,
    capture: Option<Capture>,
    first: Option<KeyOutcome>,
) -> WindowHandle {
    let (msgs_tx, msgs_rx) = tokio::sync::mpsc::unbounded_channel();
    let (keys_tx, keys_rx) = tokio::sync::mpsc::unbounded_channel();
    let scrollback = settings.profile().scrollback_lines as usize;
    let screen = Arc::new(Mutex::new(Screen::new(rows.saturating_sub(2), cols, scrollback)));
    // Primed from the settings rather than left blank and synced,
    // because opening a window is not a change to report.
    let (host, port, username, dirty) = facts(&settings);
    let info = Arc::new(Mutex::new(WindowInfo {
        host,
        port,
        username,
        dirty,
        bar: idle_bar(&settings),
        ..WindowInfo::default()
    }));
    let window = Window {
        id,
        msgs: msgs_rx,
        keys: keys_rx,
        screen: screen.clone(),
        info: info.clone(),
        front,
        settings,
        cache,
        capture,
        rows,
        cols,
    };
    tokio::spawn(run(window, first));
    WindowHandle {
        id,
        msgs: msgs_tx,
        keys: keys_tx,
        screen,
        info,
    }
}

/// The notices sink a window gives the jobs it starts.
///
/// A job runs in its own task, so it cannot borrow the window. It gets
/// clones of the three things a note needs instead: the screen to write
/// on, the front end to tell, and which window it is. Built here rather
/// than at each start call so the window's own `note` and a job's
/// notice cannot drift apart.
pub fn notices(
    id: WindowId,
    screen: Arc<Mutex<Screen>>,
    front: UnboundedSender<FrontMsg>,
) -> crate::farm::Notices {
    Arc::new(move |text: &str| {
        screen.lock().expect("screen lock").note(text);
        let _ = front.send(FrontMsg::Changed { window: id, screen: true });
    })
}

impl Window {
    /// Something the front end paints moved. `screen` says whether the
    /// rows moved or only the bar did.
    fn changed(&self, screen: bool) {
        let _ = self.front.send(FrontMsg::Changed {
            window: self.id,
            screen,
        });
    }

    fn note(&self, text: &str) {
        self.screen.lock().expect("screen lock").note(text);
        self.changed(true);
    }

    fn feed(&self, bytes: &[u8]) {
        self.screen.lock().expect("screen lock").feed(bytes);
        self.changed(true);
    }

    fn event(&self, kind: EventKind) {
        let _ = self.front.send(FrontMsg::Event {
            window: self.id,
            kind,
        });
    }

    /// The bar and the job flag. Signals only when the bar text moved,
    /// which is what keeps a countdown that has not ticked from
    /// repainting.
    fn set_bar(&self, bar: String, farming: bool) {
        let mut info = self.info.lock().expect("info lock");
        let moved = info.bar != bar || info.farming != farming;
        info.bar = bar;
        info.farming = farming;
        drop(info);
        if moved {
            self.changed(false);
        }
    }

    /// Connection facts from the settings, for `/windows` and the bar.
    /// Signals when one of them moved, the way `set_bar` does, so a new
    /// host or a dirty flag reaches the front end without waiting for a
    /// note to follow it. Silent when nothing moved, so a command that
    /// changed no connection fact costs no repaint.
    ///
    /// The session, when there is one, is what names the character. It
    /// has the stat sheet the board printed, and the settings only have
    /// the board account. Without one the settings are all there is.
    fn sync_info(&self, session: Option<&Session>) {
        let (host, port, settings_username, dirty) = facts(&self.settings);
        let connected = session.is_some();
        let username = match session {
            Some(s) => s.character_name().unwrap_or_default(),
            None => settings_username,
        };
        let mut info = self.info.lock().expect("info lock");
        let moved = info.host != host
            || info.port != port
            || info.username != username
            || info.connected != connected
            || info.dirty != dirty;
        info.host = host;
        info.port = port;
        info.username = username;
        info.connected = connected;
        info.dirty = dirty;
        drop(info);
        if moved {
            self.changed(false);
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        self.screen
            .lock()
            .expect("screen lock")
            .resize(rows.saturating_sub(2), cols);
        self.changed(true);
    }

    /// The bar for a window with no session. Set on the way into the
    /// disconnected state and after every settings command there, so the
    /// host it would connect to is what the bar names.
    fn set_idle_bar(&self) {
        self.set_bar(idle_bar(&self.settings), false);
    }

    /// The map view drives the real terminal, so the front end has to
    /// stand back while it does.
    fn takeover(&self, on: bool) {
        let _ = self.front.send(FrontMsg::Takeover {
            window: self.id,
            on,
        });
    }
}

/// Why the play loop returned.
enum PlayEnd {
    /// The board closed the line, or the connection failed under the
    /// loop. Nobody asked for it, so this is what a redial answers.
    Closed,
    /// The operator typed `/disconnect`. The line is just as closed and
    /// the tidying is the same, but a window is not dialled back into a
    /// board its operator just left.
    Left,
    /// The front end dropped the handle. The task ends.
    Ended,
}

/// What the disconnected state returned.
enum Idle {
    /// The operator asked for a connection.
    Connect,
    /// The redial timer came due. This one logs in as well, which is
    /// why it is not the same answer as `Connect`.
    Redial,
    Ended,
}

/// How long to wait before redialling, or `None` when this window is
/// staying put. Says on the window's screen why it is staying, when the
/// profile asked for a redial it cannot make.
///
/// A profile that asks for a reconnect without both credentials cannot
/// log back in, and a redial that stopped at the username prompt would
/// look connected while being useless. So it says so and stays.
fn arm_redial(w: &Window) -> Option<std::time::Duration> {
    let profile = w.settings.profile();
    if !profile.reconnect {
        return None;
    }
    if profile.username.is_empty() || profile.password.is_empty() {
        w.note("-- reconnect is on but username or password is empty, staying here --");
        return None;
    }
    Some(std::time::Duration::from_secs(profile.reconnect_delay_seconds))
}

/// Start a recover job for `to` and note the outcome on `w`, whichever
/// key asked for it. `None` means the refusal is already noted and there
/// is nothing for the caller to fold into `job` or `phase_rx`.
fn recover_to(
    w: &Window,
    session: Arc<Session>,
    g: Arc<crate::graph::RoomGraph>,
    here: Option<mud_core::content::RoomId>,
    safe: Option<mud_core::content::RoomId>,
    to: mud_core::content::RoomId,
    notices: crate::farm::Notices,
) -> Option<Job> {
    let name = |id: mud_core::content::RoomId| {
        format!("{} [{}/{}]", g.room(id).map(|r| r.name.clone()).unwrap_or_default(), id.map, id.room)
    };
    let via = match safe {
        Some(safe) => format!(" via the safe room {}", name(safe)),
        None => String::new(),
    };
    let note = format!("-- recovering from {}{via} (Ctrl-F to take over) --", name(to));
    match start_recover(session, g, here, safe, to, notices) {
        Err(e) => {
            w.note(&format!("-- recover: {e} --"));
            None
        }
        Ok(started) => {
            w.note(&note);
            Some(started)
        }
    }
}

async fn run(mut w: Window, first: Option<KeyOutcome>) {
    // A window opened on a profile that already names a host connects
    // without being asked, the way the old lobby did. Only on the way
    // in: a line that closes lands back in the disconnected state and
    // waits there for `/connect`.
    let mut first = first.or_else(|| {
        (!w.settings.profile().host.is_empty()).then_some(KeyOutcome::Connect { target: None })
    });
    // How long to wait before redialling, and which try this is. Armed
    // only by a line that closed on its own, so a `/connect` the
    // operator typed disarms it until the next drop.
    let mut redial: Option<std::time::Duration> = None;
    let mut attempt = 1u32;
    loop {
        let relogin = match disconnected(&mut w, first.take(), redial).await {
            // The operator dialled, so this stretch's wait is spent and
            // the count starts over. A dial that fails arms a fresh one
            // below, when the profile asks for it.
            Idle::Connect => {
                redial = None;
                attempt = 1;
                false
            }
            Idle::Redial => true,
            Idle::Ended => break,
        };
        let profile = w.settings.profile().clone();
        if relogin {
            w.event(EventKind::Reconnecting { attempt });
        }
        // A capture names two files and creating them truncates, so it
        // records the first connection only. Cloned rather than taken:
        // a connect that fails opened nothing, so the next attempt still
        // has somewhere to record.
        let session = match Session::connect(&profile, w.capture.clone()).await {
            Ok(s) => {
                w.capture = None;
                Arc::new(s)
            }
            Err(e) => {
                w.note(&format!("-- connect {}:{}: {e} --", profile.host, profile.port));
                // Only a redial counts its tries. A `/connect` the
                // operator typed is the first of a new stretch, and one
                // that failed still leaves the profile asking for a
                // reconnect, so the schedule picks it up from here.
                if relogin {
                    attempt += 1;
                } else {
                    redial = arm_redial(&w);
                    if let Some(wait) = redial {
                        w.note(&format!(
                            "-- connect failed, redialling in {} seconds --",
                            wait.as_secs()
                        ));
                    }
                }
                continue;
            }
        };
        // Subscribed before the login, so the bytes the login is served
        // reach the screen. A subscription made inside `play` would
        // start after them and the window would come back blank.
        let raw_rx = session.raw();
        // The redial types the credentials interactive play leaves to
        // the operator. Still at the profile's pace, because the board's
        // flood control watches the login too.
        //
        // A prompt that has not come in three redial delays is a stalled
        // board, and the next redial is the retry, so this gives up
        // rather than holding a dead line for the dialect's default
        // thirty seconds. Floored at three seconds so a short delay
        // still allows for a slow board, and capped at the default so a
        // long one never waits longer than a headless login would.
        if relogin
            && let Err(e) = crate::dialect::login_with_timeout(
                &session,
                &profile,
                std::time::Duration::from_secs(
                    profile.reconnect_delay_seconds.saturating_mul(3).clamp(3, 30),
                ),
            )
            .await
        {
            // `play` is the only other place a session is closed and it
            // is not entered on this path. `Session` has no `Drop`, and
            // its reader task holds a `cmd_tx` clone, so letting the
            // last handle go frees nothing. The socket and both tasks
            // would live until the board hung up, once per attempt.
            session.close();
            w.note(&format!("-- reconnect login: {e} --"));
            attempt += 1;
            continue;
        }
        // Interactive play is not paced. `pace_ms` is flood control,
        // which is for automation. The session keeps the real profile,
        // so `/farm` can put its pace back.
        session.set_pace(std::time::Duration::ZERO);
        w.sync_info(Some(&session));
        w.event(EventKind::Connected);
        let end = play(&mut w, session, raw_rx).await;
        w.sync_info(None);
        w.event(EventKind::Disconnected);
        attempt = 1;
        redial = match end {
            PlayEnd::Closed => {
                w.note("-- disconnected. /connect to go back --");
                arm_redial(&w)
            }
            PlayEnd::Left => {
                w.note("-- disconnected. /connect to go back --");
                None
            }
            PlayEnd::Ended => break,
        };
    }
    let _ = w.front.send(FrontMsg::Ended { window: w.id });
}

/// The window with no session: settings commands, `/connect`, `/help`.
/// `quit_armed` exists for `lobby_step`'s signature. `/quit` never
/// reaches a window, the front end owns it.
///
/// `redial_after` is how long a profile with `reconnect` on waits before
/// it dials by itself. The wait is a deadline rather than a sleep the
/// loop restarts, so everything the operator types while it runs is
/// handled and the redial still comes due when it was always going to.
async fn disconnected(
    w: &mut Window,
    first: Option<KeyOutcome>,
    redial_after: Option<std::time::Duration>,
) -> Idle {
    let mut quit_armed = false;
    let mut pending = first;
    let mut due = redial_after.map(|wait| tokio::time::Instant::now() + wait);
    w.set_idle_bar();
    loop {
        let outcome = match pending.take() {
            Some(o) => o,
            None => {
                let msg = match due {
                    Some(at) => tokio::select! {
                        () = tokio::time::sleep_until(at) => return Idle::Redial,
                        msg = w.msgs.recv() => msg,
                    },
                    None => w.msgs.recv().await,
                };
                match msg {
                    None => return Idle::Ended,
                    Some(WindowMsg::Resize { rows, cols }) => {
                        w.resize(rows, cols);
                        continue;
                    }
                    Some(WindowMsg::Outcome(o)) => o,
                }
            }
        };
        // `/disconnect` with nothing to disconnect from means stop
        // redialling. The lobby's own answer to it says "not connected",
        // which is true and no use here.
        if due.is_some() && matches!(outcome, KeyOutcome::Disconnect) {
            due = None;
            w.note("-- reconnect paused, /connect to dial --");
            continue;
        }
        let (step, text) = lobby_step(outcome, &mut w.settings, &mut quit_armed);
        w.sync_info(None);
        w.set_idle_bar();
        if let Some(text) = text {
            w.note(&text);
        }
        match step {
            LobbyStep::Connect => return Idle::Connect,
            LobbyStep::Quit | LobbyStep::Stay => {}
        }
    }
}

/// A job's ending as an event. A death is its own kind so the lobby
/// and the bar can say so without reading the label.
///
/// `starts_with` rather than an equality, because a farm's `why` carries
/// its kill and lap counts after the reason.
pub fn event_for(ended: &crate::farm::Phase) -> EventKind {
    match ended {
        crate::farm::Phase::Done { why, .. } if why.starts_with(crate::farm::DIED) => {
            EventKind::Died
        }
        other => EventKind::JobEnded(other.label()),
    }
}

/// The status bar's text, without painting it. Same phase and room
/// lookups as the bar the front end draws, so the timer arm can tell
/// whether a fresh repaint would actually change anything before it
/// pays for one.
#[allow(clippy::too_many_arguments)]
fn bar_text(
    state_rx: &tokio::sync::watch::Receiver<crate::session::GameState>,
    job: Option<&Job>,
    here: crate::lost::Fix,
    rates: &Rates,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    party: &crate::party::PartyState,
    cols: u16,
) -> String {
    let phase = job.map(|j| j.phase.borrow().clone());
    // The runner's own belief wins while it drives -- it knows which of
    // two same-named rooms it walked to -- and the client's tracking
    // covers everything else.
    let room_id = match phase.as_ref().and_then(|p| p.room()) {
        Some(at) => crate::lost::Fix::Confirmed(at),
        None => here,
    };
    let state = state_rx.borrow().clone();
    render_status(&state, std::time::Instant::now(), phase.as_ref(), room_id, rates.exp_per_hour(), rates.gold_per_hour(), level, assist, party, cols as usize)
}

/// The session's two rates and the one clock they are measured against.
///
/// One clock, because the two meters have the same contract: a reset
/// that zeroes the total without restarting the clock reads as a spike.
/// Holding the clock here makes that mistake impossible to write.
struct Rates {
    exp: crate::progress::ExpMeter,
    income: crate::progress::IncomeMeter,
    since: std::time::Instant,
}

impl Rates {
    fn new() -> Self {
        Rates {
            exp: Default::default(),
            income: Default::default(),
            since: std::time::Instant::now(),
        }
    }

    /// Feed both meters one line. True when either total moved, which is
    /// when the bar is worth repainting.
    fn observe(&mut self, line: &str) -> bool {
        let before = (self.exp.total(), self.income.total());
        self.exp.observe(line);
        self.income.observe(line);
        before != (self.exp.total(), self.income.total())
    }

    /// Start both rates over from now. A long `/go` or `/farm` would
    /// otherwise dilute the session's rate with the minutes before the
    /// job started.
    fn reset(&mut self) {
        self.exp.reset();
        self.income.reset();
        self.since = std::time::Instant::now();
    }

    fn exp_per_hour(&self) -> Option<i64> {
        self.exp.per_hour(self.since.elapsed())
    }

    fn gold_per_hour(&self) -> Option<crate::purse::Purse> {
        self.income.per_hour(self.since.elapsed())
    }
}

/// One connected session, until the line closes or the front end lets go.
///
/// The board's bytes go into the window's screen verbatim. The two
/// reserved rows are the front end's business now, so this loop only
/// keeps [`WindowInfo::bar`] current and says when something changed.
async fn play(
    w: &mut Window,
    session: Arc<Session>,
    mut raw_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
) -> PlayEnd {
    let mut events = session.events();
    let mut state_rx = session.state();
    let mut rests = session.rests();
    // What the party did, in the session's own words. Printed on this
    // window's screen the way a rest is, and never parked on alone: the
    // channel ends with the session, exactly as `rests` does.
    let mut party_notes = session.party_notes();
    // The last recovery the prompt showed, so each one is logged once.
    let mut recovering: Option<Status> = None;
    // One sink for every job this connection starts, so a runner's
    // startup notices land on this window's screen rather than on the
    // terminal the front end is painting.
    let notices = notices(w.id, w.screen.clone(), w.front.clone());
    let (mut cols, mut rows) = (w.cols, w.rows);
    // Set while the runner drives this session; carries its phase for the
    // status bar and the handle needed to call it off.
    let mut job: Option<Job> = None;
    // A clone of the job's phase channel, kept separate so the select
    // can await it without borrowing job, which the bar needs.
    let mut phase_rx: Option<tokio::sync::watch::Receiver<crate::farm::Phase>> = None;
    // Where the client believes the character is, and how much that
    // belief is worth. A room block is a room block: it says as much when
    // the operator typed the move as when the runner did — but a block
    // the graph cannot name says nothing at all, and that used to be
    // indistinguishable from agreement.
    let mut here = crate::lost::Fix::Unknown;
    // One death, one line in the log.
    let mut deaths = crate::deathlog::DeathWatch::default();
    // The fix the death log writes, and the reason it is not `here`.
    // `here` is moved by the state arm of this same select, off the
    // session's watch. The session folds an event into that watch as it
    // emits it, so a death and the recall block that follows it can both
    // be ready before this task is polled at all, and an unbiased select
    // is free to run the state arm first. `here` would then already name
    // the room the character woke up in. This one moves only in the
    // events arm, from the blocks it has itself seen, so a death is
    // logged against the last room this arm read BEFORE the death.
    let mut death_fix = crate::lost::Fix::Unknown;
    // Experience and income rates, counted from the board's award and
    // pickup lines. Run for the whole session, not just while a farm is
    // attached: a hand-played stretch is worth measuring too.
    let mut rates = Rates::new();
    // `content` is held for the session's lifetime alongside `graph` and
    // `spawns`; `on_realm_entry` is its one reader, for the spellbook
    // probe's class/magictype skip (Task 5 of `one-path-to-content`).
    // Loaded ahead of the assist below, so its first build reads the
    // sheet's heal marks the same as every rebuild after it.
    // Bound once, beside the load it answers for: `/load` can replace the
    // profile mid-session, and a refusal has to name the path that was
    // actually looked at, not the one the settings name now.
    let world_path = content_path(&session.profile());
    let found = w.cache.lock().expect("cache lock").world(&world_path);
    let (graph, nav, spawns, content) = finish_locator(found, &session);
    let durations = content.as_ref().map(|c| crate::views::spell_durations(c)).unwrap_or_default();
    // The room names that are banks. Read on every room block while
    // following, so it is built once rather than per block.
    let party_banks = content.as_ref().map(|c| crate::party::bank_names(c)).unwrap_or_default();
    // The name on the last room block the character was in, which is
    // what makes the next one an arrival rather than a re-render. Names
    // rather than ids: the bank gate reads the printed name too, and
    // both must work with no room database at all.
    let mut last_room: Option<String> = None;
    // The assist: a bot that fights and loots, rests, heals and hides
    // BESIDE the operator while no farm runs. `/bot` toggles it; the
    // profile's `assist_play` starts it on. Rebuilt on every toggle-on
    // so its latches start clean.
    let mut assist: Option<crate::bot::Bot> = None;
    let mut assist_casts: Option<crate::tui::AssistCasts> = None;
    let mut assist_config = assist_config_for(&session.profile());
    // Rests the assist sends and the spells its book was last read to
    // hold. Both live beside the bot and are reset with it, because both
    // describe the bot that is running now.
    let mut assist_watch = crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
    // A length no book can have, so the first tick always reads the sheet
    // and prints its refusals, even for an empty book.
    let mut assist_book_seen = usize::MAX;
    // Refused, not started, when the character has no name: an assist
    // that cannot recognise its own death line is worse than none. The
    // refusal is printed below, before the loop.
    let mut assist_refused = None;
    if assist_config.assist_play {
        match needs_name(&session) {
            Ok(_) => {
                let (bot, casts) = new_assist(&session, &assist_config);
                assist = Some(bot);
                assist_casts = Some(casts);
            }
            Err(why) => assist_refused = Some(why),
        }
    }
    // What the board says the pools are, for a profile that leaves
    // `bot.max_hp` at zero. Asked at the loop head once the character
    // is in the realm, and applied to every rebuild of the config.
    let mut assist_vitals = AssistVitals::Unknown;
    // The recovery's safe room and death room, marked on the map with S
    // and D. Kept here so they outlive the map, and gone with the
    // window: a mark is for the death it was made after.
    let mut recover_marks = crate::recover::Marks::default();
    // `/bot` is one switch over every mode: the assist beside the
    // operator, and every job's own policies through its live settings.
    // The session carries it so a job hears the press.
    session.set_bot_on(assist.is_some());
    // The maintained room model, running in SHADOW: it decides nothing
    // here, it only records where it and the board disagree. The farm
    // builds its own per stop (`farm.rs`), so this one exists for the
    // hand-played sessions a farm never touches — which on a foreign
    // board is where the unparsed wordings actually show up.
    let mut model = crate::world::Here::default();
    // One note per distinct disagreement. A wording the parser cannot
    // read fires every single lap, and the interesting fact is that it
    // happened at all, not how often.
    let mut noted: std::collections::HashSet<String> = std::collections::HashSet::new();
    // The board's own answer to `exp`, refreshed on its own timer. The
    // kill-by-kill rate already jitters; a time-to-level that jittered
    // with it would be unreadable, so this is deliberately a slow tick.
    let mut level: Option<crate::progress::LevelProgress> = None;
    let mut level_tick = tokio::time::interval(LEVEL_POLL);
    // The countdowns in the bar move between events, so the bar text is
    // refreshed on its own short timer as well as on every event.
    let mut tick_paint = tokio::time::interval(std::time::Duration::from_millis(250));
    // A stalled loop must not catch up with a burst of repaints.
    tick_paint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Whether the character is standing in the realm at all. A tokio
    // interval's FIRST tick completes immediately, so without this the
    // very first `exp` went out into the username prompt the instant the
    // socket opened (live, 2026-08-01). Nothing sent on a timer may
    // assume a game is running.
    let mut in_realm = false;

    if let Some(why) = &assist_refused {
        w.note(&format!("-- bot assist not started: {why} --"));
        w.event(EventKind::AssistRefused(why.clone()));
    }
    w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());

    let end = loop {
        // Every percent policy the assist has divides by the pools, and
        // `Bot::hp_percent` refuses to decide on a zero max. A profile
        // that leaves `max_hp` out means "ask the board", and every job
        // does through the same `discover_vitals`. The assist ran on
        // the zero until 2026-09-08: a character at 102/102 with
        // Stealth and `/bot` on never rested, healed, fled or sneaked.
        if in_realm
            && assist.is_some()
            && assist_config.max_hp == 0
            && matches!(assist_vitals, AssistVitals::Unknown)
        {
            let probe = session.clone();
            assist_vitals =
                AssistVitals::Asking(tokio::spawn(async move { crate::farm::discover_vitals(&probe).await }));
        }
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    w.feed(&bytes);
                    w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break PlayEnd::Closed, // disconnected
            },
            ev = events.recv() => {
                // The display comes from raw passthrough, so nothing is
                // rendered here — only the counters and the assist.
                if let Ok(cor) = &ev {
                    // The death log's own position, folded here from the
                    // blocks this arm sees. Same rule as the state arm's
                    // `here`, and a peek at the next room is skipped for
                    // the same reason: it describes the room next door.
                    if let crate::events::Event::RoomSeen(room) = &cor.event
                        && !cor.elsewhere
                        && let Some(nav) = nav.as_ref()
                    {
                        death_fix = crate::lost::refix(nav, death_fix, room);
                    }
                    // The sheet's name, or the profile's username until
                    // a sheet is read, which is what `character_name`
                    // answers. A nameless character is not watched at
                    // all: an empty name matches no death line and
                    // would write a record with an empty field.
                    if let Some(name) = session.character_name()
                        && deaths.on_event(&cor.event, &name)
                    {
                        let namer = |id| {
                            graph
                                .as_ref()
                                .and_then(|g| g.room(id))
                                .map(|r| r.name.clone())
                                .unwrap_or_default()
                        };
                        w.note(
                            &match crate::deathlog::log_death(
                                &name,
                                death_fix,
                                namer,
                                &crate::deathlog::path(),
                            ) {
                                Ok(death) => format!("-- death logged: {} --", death.line()),
                                Err(e) => format!("-- death not logged: {e} --"),
                            },
                        );
                    }
                    if let Some(present) = crate::dialect::realm_presence(&cor.event) {
                        // Entering is worth asking about straight away;
                        // waiting out the full period would leave the bar
                        // blank for the first minute of every session.
                        if present && !in_realm {
                            on_realm_entry(&session, content.clone());
                            // A board that did not answer last time is
                            // asked again on the next entry, not before.
                            if matches!(assist_vitals, AssistVitals::Unanswered) {
                                assist_vitals = AssistVitals::Unknown;
                            }
                        }
                        in_realm = present;
                    }
                    if let crate::events::Event::Line(line) = &cor.event {
                        if let Some(p) = crate::progress::level_progress(line) {
                            level = Some(p);
                        }
                        if rates.observe(line) {
                            w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
                        }
                        // The carried balance is the session's own
                        // answer now (`Session::capabilities`), fed
                        // centrally off whichever `i` anyone sends —
                        // `play` used to keep a second, private
                        // `PurseMeter` fed the same way and never read
                        // it. One tracker for one fact.
                    }
                    // Dragged into a bank while following: deposit and
                    // tell the leader. It takes the job slot, which is
                    // what holds the assist off while it runs, and the
                    // end notice and the handover are the slot's own.
                    //
                    // Room blocks are tested here as well as inside the
                    // decision, so that reading the party and the
                    // profile, both of which copy, costs nothing on the
                    // lines between blocks.
                    if job.is_none()
                        && assist.is_some()
                        && matches!(&cor.event, crate::events::Event::RoomSeen(_))
                        && let Some(content) = content.as_ref()
                        && let Some(bank) = follower_bank_arrival_decision(
                            &session.party(),
                            session.bot_on(),
                            session.profile().bank.auto_deposit,
                            &party_banks,
                            last_room.as_deref(),
                            cor,
                        )
                    {
                        w.note(&format!("-- party: at {bank}, depositing --"));
                        let started =
                            start_follower_deposit(session.clone(), content.clone(), &bank);
                        phase_rx = Some(started.phase.clone());
                        job = Some(started);
                    }
                    if let crate::events::Event::RoomSeen(room) = &cor.event && !cor.elsewhere {
                        last_room = Some(room.name.clone());
                    }
                    // While a farm runs it owns the connection outright;
                    // the assist only drives a hand-played session.
                    if job.is_none()
                        && let (Some(bot), Some(casts)) =
                            (assist.as_mut(), assist_casts.as_mut())
                    {
                        let clock = state_rx.borrow().ticks.round.clone();
                        let refusals = assist_tick(
                            &session,
                            &assist_config,
                            &durations,
                            bot,
                            casts,
                            &mut assist_watch,
                            &mut assist_book_seen,
                            &clock,
                            cor,
                            std::time::Instant::now(),
                        );
                        for why in refusals {
                            w.note(&format!("-- {why} --"));
                        }
                    }
                    // Shadow bookkeeping, farm-free sessions only: while
                    // a farm runs it keeps its own model and counts its
                    // own divergences, and folding here too would double
                    // them.
                    if job.is_none() {
                        let before = model.reconcile.total();
                        model.on_event(cor, std::time::Instant::now());
                        if model.reconcile.total() != before {
                            for d in model.reconcile.recent() {
                                let entry = format!("{} {}", d.kind.label(), d.name);
                                if noted.insert(entry.clone()) {
                                    w.note(&format!("-- room model: {entry} --"));
                                }
                            }
                        }
                    }
                }
            }
            why = rests.recv() => match why {
                Ok(why) => w.event(EventKind::Rest(why)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break PlayEnd::Closed, // disconnected
            },
            note = party_notes.recv() => match note {
                Ok(note) => {
                    w.note(&format!("-- {note} --"));
                    // The board keeps a per-character follow mode and
                    // the client assumes the normal one, where the
                    // leader's move drags the follower. Set on every
                    // began-following note, and only when the profile
                    // asks for it. Not a `/bot` decision: how the
                    // operator follows is not automation.
                    if note.starts_with("party: following ") && session.profile().party.follow_normal {
                        session.send("set follow normal");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break PlayEnd::Closed, // disconnected
            },
            changed = state_rx.changed() => {
                if changed.is_err() { break PlayEnd::Closed; }
                let status = state_rx.borrow().status.clone();
                if status != recovering {
                    if let Some(s @ (Status::Resting | Status::Meditating)) = &status {
                        w.event(EventKind::Rest(format!("the prompt shows ({})", s.word())));
                    }
                    recovering = status;
                }
                if let (Some(nav), Some(room)) = (nav.as_ref(), state_rx.borrow().room.clone()) {
                    here = crate::lost::refix(nav, here, &room);
                    // The shadow model keys identity on the printed name
                    // unless somebody can do better, and here somebody
                    // can: the locator already resolved this block to an
                    // id for the status bar. Without it, walking between
                    // Newhaven's twin Narrow Roads reads as a re-render.
                    if let Some(id) = here.confirmed() {
                        model.note_room(id);
                    }
                }
                w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
            }
            // The bar must follow the runner, not just HP: travelling and
            // fighting can pass without a single point of damage.
            changed = async {
                match phase_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending::<bool>().await,
                }
            } => {
                // A closed watch means the farm task ENDED. Erroring
                // here forever was a busy loop — changed() on a dead
                // sender returns instantly, and the discarded Err
                // repainted the bar in a tight spin (the "flashing"
                // status bar, live 2026-08-01). Retire the run: say why
                // it ended, give the keyboard and the assist back.
                if !changed {
                    // A job that ended KNOWING where it stands is the
                    // best position evidence there is — better than the
                    // room blocks `track` sees, which is exactly why
                    // `/where` exists: it resolves rooms no single block
                    // can. Adopt it before the channel is dropped.
                    let ended = phase_rx
                        .take()
                        .map(|rx| rx.borrow().clone())
                        .unwrap_or(crate::farm::Phase::Done { why: "done".into(), at: None });
                    if let Some(at) = ended.room() {
                        here = crate::lost::Fix::Confirmed(at);
                        model.note_room(at);
                    }
                    let what = job.take().map(|j| j.what).unwrap_or("job");
                    session.set_pace(std::time::Duration::ZERO);
                    // Latches from the stretch the job just drove describe
                    // fights that are over, so the assist starts clean for
                    // the same reason `/bot` rebuilds it on every toggle-on.
                    if assist.is_some() {
                        let (bot, mut casts) = new_assist(&session, &assist_config);
                        // The follower's deposit gate is not a fight
                        // latch: it carries how long ago the leader was
                        // asked for a bank, and a job ending must not
                        // let the follower ask again inside the five
                        // minutes. The deposit job is the one job that
                        // clears it, because the coins are in the bank.
                        // The wait handshake is not a latch either: a
                        // job that ran while the character was sitting
                        // down must not swallow the `@ok` its `@wait`
                        // owes the leader.
                        if let Some(old) = assist_casts.take() {
                            casts.follower = old.follower;
                            casts.wait = old.wait;
                        }
                        if what == crate::tui::PARTY_DEPOSIT {
                            casts.follower.deposited();
                        }
                        assist = Some(bot);
                        assist_casts = Some(casts);
                        assist_watch =
                            crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
                        assist_book_seen = usize::MAX;
                    }
                    for cmd in handover_actions(&ended, assist.is_some()) {
                        session.send(&cmd);
                    }
                    w.note(&format!("-- {what} ended: {} --", ended.label()));
                    w.event(event_for(&ended));
                }
                w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
            }
            learned = async {
                match &mut assist_vitals {
                    AssistVitals::Asking(probe) => probe.await.ok().flatten(),
                    _ => std::future::pending().await,
                }
            } => {
                assist_vitals = match learned {
                    Some(vitals) => AssistVitals::Known(vitals),
                    None => AssistVitals::Unanswered,
                };
                match learned {
                    Some(vitals) => {
                        assist_config = with_vitals(assist_config, &assist_vitals);
                        // Reconfigured, not rebuilt, for the reason the
                        // settings arm gives: the memory stays.
                        if let Some(bot) = assist.as_mut() {
                            bot.reconfigure(assist_config.clone());
                        }
                        assist_watch.reconfigure(&assist_config, &crate::farm::FarmConfig::default());
                        let mana = if vitals.max_mana > 0 {
                            format!(", {} mana", vitals.max_mana)
                        } else {
                            String::new()
                        };
                        w.note(&format!("-- assist: the board says {} hits{mana} --", vitals.max_hp));
                    }
                    None => w.note(
                        "-- assist: no answer to health, so the rest, heal and flee marks are off until /set bot.max_hp --",
                    ),
                }
            }
            _ = tick_paint.tick() => {
                w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
            }
            _ = level_tick.tick() => {
                // Only while a game is actually running — see `in_realm`.
                // Unattributed by design: the correlator has no reply
                // grammar for `exp` (it is Opaque), so it simply expires.
                // The answer is recognised by its own wording, whoever
                // asked, which also picks up an `exp` the operator types.
                if in_realm {
                    session.send("exp");
                }
            }
            msg = w.msgs.recv() => {
                let Some(msg) = msg else { break PlayEnd::Ended };
                match msg {
                    WindowMsg::Resize { rows: r, cols: c } => {
                        cols = c;
                        rows = r;
                        w.resize(r, c);
                        w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
                    }
                    WindowMsg::Outcome(outcome) => {
                        if let Some(applied) = apply_settings(&outcome, &mut w.settings) {
                            w.note(&applied.note);
                            // The session first. It is what `sync_info`
                            // reads the character's name from, so a
                            // `/set username` has to reach it before the
                            // window info is rebuilt.
                            if applied.profile_changed {
                                session.set_profile(w.settings.profile().clone());
                                if let Some(pace) = pace_on_change(job.is_some(), w.settings.profile()) {
                                    session.set_pace(pace);
                                }
                            }
                            w.sync_info(Some(&session));
                            if applied.bot_changed {
                                assist_config = with_vitals(assist_config_for(w.settings.profile()), &assist_vitals);
                                if assist.is_some() {
                                    let (bot, casts) = new_assist(&session, &assist_config);
                                    assist = Some(bot);
                                    assist_casts = Some(casts);
                                    // Reconfigured, not rebuilt: a fresh
                                    // watch starts at "not watching" and
                                    // would drop the rest in flight, and
                                    // with it the retry that ends a rest
                                    // the board refused.
                                    assist_watch.reconfigure(&assist_config, &crate::farm::FarmConfig::default());
                                    assist_book_seen = usize::MAX;
                                    w.note("-- bot assist rebuilt with the new settings --");
                                }
                            }
                        }
                        match outcome {
                            KeyOutcome::Send(line) => {
                                session.send(&line);
                            }
                            KeyOutcome::Raw(bytes) => session.send_raw(&bytes),
                            KeyOutcome::Note(text) => w.note(&text),
                            // The shutdown itself is after the loop:
                            // the board can close the line too, and all
                            // three ways it ends need the same tidying.
                            KeyOutcome::Disconnect => break PlayEnd::Left,
                            KeyOutcome::Connect { .. } => {
                                w.note("-- already connected: /disconnect first --");
                            }
                            KeyOutcome::SetList { .. }
                            | KeyOutcome::Set { .. }
                            | KeyOutcome::Unset { .. }
                            | KeyOutcome::Save { .. }
                            | KeyOutcome::Load { .. } => {}
                            KeyOutcome::Loops { name } => {
                                w.note(&describe_loops(graph.as_deref(), name.as_deref()).join("\n"));
                            }
                            KeyOutcome::ImportLoop { file } => {
                                w.note(&match graph.as_ref() {
                                    None => vec![format!(
                                        "-- loop: no room database at {} --",
                                        world_path.display()
                                    )],
                                    Some(g) => import_loop(g, std::path::Path::new(&file)),
                                }.join("\n"));
                            }
                            KeyOutcome::StartFarm { loop_name } => {
                                if let Some(j) = job.as_ref() {
                                    // Reachable mid-run now that the editor
                                    // works while farming: one job only.
                                    w.note(&format!("-- {} already running (Ctrl-F to take over) --", j.what));
                                } else {
                                    match start_farm(session.clone(), loop_name.as_deref(), notices.clone()) {
                                        Ok(started) => {
                                            // Fresh figures for a fresh
                                            // job, measured from its own
                                            // start rather than the
                                            // session's.
                                            rates.reset();
                                            w.note(&match &loop_name {
                                                Some(n) => format!("-- farming loop {n:?} (Ctrl-F to take over) --"),
                                                None => "-- farm running (Ctrl-F to take over) --".to_string(),
                                            });
                                            phase_rx = Some(started.phase.clone());
                                            job = Some(started);
                                        }
                                        Err(e) => w.note(&format!("-- {e} --")),
                                    }
                                }
                            }
                            KeyOutcome::Go { target } => {
                                if let Some(j) = job.as_ref() {
                                    w.note(&format!("-- {} already running (Ctrl-F to take over) --", j.what));
                                } else {
                                    match graph.as_ref() {
                                        // Naming the path is the whole
                                        // point: the default is relative,
                                        // so the commonest cause of this
                                        // is a working directory, and
                                        // "unknown room" would send the
                                        // operator hunting the wrong bug.
                                        None => w.note(&format!(
                                            "-- go: no room database at {} --",
                                            world_path.display()
                                        )),
                                        Some(g) => match crate::go::resolve(g, here.confirmed(), &target) {
                                            Err(refusal) => w.note(&refusal.lines("go").join("\n")),
                                            Ok(to) => {
                                                let name = g.room(to).map(|r| r.name.clone()).unwrap_or_default();
                                                let steps = here
                                                    .confirmed()
                                                    .and_then(|f| g.route(f, to))
                                                    .map(|r| format!(", {} steps", r.len()))
                                                    .unwrap_or_default();
                                                // The same test the job makes at its start.
                                                let table = session.profile().farm.clone().unwrap_or_default();
                                                let fights = session.bot_on()
                                                    && crate::farm::fights_on_the_way(&assist_config, &table);
                                                let how = if fights { "walking" } else { "running" };
                                                match start_go(
                                                    session.clone(),
                                                    g.clone(),
                                                    here.confirmed(),
                                                    to,
                                                    assist_config.clone(),
                                                    notices.clone(),
                                                ) {
                                                    Err(e) => w.note(&format!("-- {e} --")),
                                                    Ok(started) => {
                                                        // Fresh figures for a
                                                        // fresh job.
                                                        rates.reset();
                                                        w.note(&format!(
                                                            "-- {how} to {name} [{}/{}]{steps} (Ctrl-F to take over) --",
                                                            to.map, to.room
                                                        ));
                                                        phase_rx = Some(started.phase.clone());
                                                        job = Some(started);
                                                    }
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                            KeyOutcome::Recover { target } => {
                                if let Some(j) = job.as_ref() {
                                    w.note(&format!("-- {} already running (Ctrl-F to take over) --", j.what));
                                } else {
                                    match graph.as_ref() {
                                        None => w.note(&format!(
                                            "-- recover: no room database at {} --",
                                            world_path.display()
                                        )),
                                        Some(g) => {
                                            let to = crate::recover::target_of(
                                                g,
                                                here.confirmed(),
                                                target.as_deref(),
                                                recover_marks.death,
                                                &session.character_name().unwrap_or_default(),
                                                &crate::deathlog::path(),
                                            );
                                            match to {
                                                Err(why) => w.note(&why.join("\n")),
                                                Ok(to) => {
                                                    if let Some(started) = recover_to(
                                                        w,
                                                        session.clone(),
                                                        g.clone(),
                                                        here.confirmed(),
                                                        recover_marks.safe,
                                                        to,
                                                        notices.clone(),
                                                    ) {
                                                        rates.reset();
                                                        phase_rx = Some(started.phase.clone());
                                                        job = Some(started);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            KeyOutcome::Bank => {
                                if let Some(j) = job.as_ref() {
                                    w.note(&format!("-- {} already running (Ctrl-F to take over) --", j.what));
                                } else {
                                    match graph.as_ref() {
                                        None => w.note(&format!(
                                            "-- bank: no room database at {} --",
                                            world_path.display()
                                        )),
                                        Some(g) => match start_bank(
                                            session.clone(),
                                            g.clone(),
                                            here.confirmed(),
                                            assist_config.clone(),
                                            notices.clone(),
                                        ) {
                                            Err(e) => w.note(&format!("-- {e} --")),
                                            Ok(started) => {
                                                // Fresh figures for a fresh job.
                                                rates.reset();
                                                w.note("-- banking (Ctrl-F to take over) --");
                                                phase_rx = Some(started.phase.clone());
                                                job = Some(started);
                                            }
                                        },
                                    }
                                }
                            }
                            KeyOutcome::Where => {
                                match graph.as_ref() {
                                    None => w.note(&format!(
                                        "-- where: no room database at {} --",
                                        world_path.display()
                                    )),
                                    Some(g) => match start_where(session.clone(), g.clone(), here.confirmed()) {
                                        Err(e) => w.note(&format!("-- {e} --")),
                                        Ok(started) => {
                                            w.note("-- working out where you are (Ctrl-F to take over) --");
                                            phase_rx = Some(started.phase.clone());
                                            job = Some(started);
                                        }
                                    },
                                }
                            }
                            KeyOutcome::Room { target } => {
                                match (graph.as_ref(), spawns.as_ref()) {
                                    (Some(g), Some(s)) => match here_or(g, here.last_known(), &target) {
                                        Err(refusal) => w.note(&refusal.lines("room").join("\n")),
                                        Ok(id) => match crate::spawn::Dossier::of(g, s, id) {
                                            None => w.note(&format!("-- room: no room {}/{} --", id.map, id.room)),
                                            Some(d) => w.note(&d.lines().join("\n")),
                                        },
                                    },
                                    _ => w.note(&format!(
                                        "-- room: no room database at {} --",
                                        world_path.display()
                                    )),
                                }
                            }
                            KeyOutcome::Map { target } => {
                                match (graph.as_ref(), spawns.as_ref()) {
                                    (Some(g), Some(s)) => match here_or(g, here.last_known(), &target) {
                                        Err(refusal) => w.note(&refusal.lines("map").join("\n")),
                                        Ok(id) => {
                                            let mut view = crate::mapview::MapView::new(
                                                g.clone(),
                                                s.clone(),
                                                id,
                                                here,
                                                crate::map::PaintCtx {
                                                    max_hp: assist_config.max_hp.into(),
                                                    ..Default::default()
                                                },
                                                (cols as usize, rows as usize),
                                            )
                                            .with_recover_marks(recover_marks);
                                            // What the assist refused while the
                                            // map had the terminal. Collected
                                            // rather than printed. The screen
                                            // is the map's until it returns, and
                                            // a note written over it would be
                                            // painted on rows the map owns.
                                            let mut refused_under_map: Vec<String> = Vec::new();
                                            // Scoped so the borrows the map
                                            // needs are gone before the exit
                                            // is acted on.
                                            let ran = {
                                                // The session keeps running
                                                // behind the map: going blind
                                                // to plan a route must not
                                                // also stop the assist
                                                // fighting for you.
                                                // The round clock is snapshotted here
                                                // because the closure cannot hold a borrow
                                                // of the state watch across the view's
                                                // whole run. A map session is short, and
                                                // the worst a stale snapshot costs is a
                                                // cast held back one round.
                                                let assist_clock = state_rx.borrow().ticks.round.clone();
                                                let mut on_event = |cor: &crate::correlate::Correlated| {
                                                    if let crate::events::Event::Line(line) = &cor.event {
                                                        rates.observe(line);
                                                    }
                                                    if job.is_none()
                                                        && let (Some(bot), Some(casts)) =
                                                            (assist.as_mut(), assist_casts.as_mut())
                                                    {
                                                        let refusals = assist_tick(
                                                            &session,
                                                            &assist_config,
                                                            &durations,
                                                            bot,
                                                            casts,
                                                            &mut assist_watch,
                                                            &mut assist_book_seen,
                                                            &assist_clock,
                                                            cor,
                                                            std::time::Instant::now(),
                                                        );
                                                        refused_under_map.extend(refusals);
                                                    }
                                                };
                                                // The map draws on the real terminal, so
                                                // the front end stands back and hands it
                                                // the raw keys until it is done.
                                                w.takeover(true);
                                                crate::mapview::run(
                                                    &session,
                                                    &mut view,
                                                    &mut w.keys,
                                                    &mut raw_rx,
                                                    &mut events,
                                                    nav.as_ref(),
                                                    &mut on_event,
                                                )
                                                .await
                                            };
                                            // A key typed between here and the front
                                            // end reading the takeover below lands in
                                            // the raw channel with nothing to read it.
                                            // Dropped now, or it replays as a map
                                            // keystroke the next time the map opens.
                                            while w.keys.try_recv().is_ok() {}
                                            w.takeover(false);
                                            // Whatever was marked while the map
                                            // was open, for the next /recover and
                                            // the next map.
                                            recover_marks = view.recover_marks();
                                            // The screen is the window's again.
                                            for why in refused_under_map {
                                                w.note(&format!("-- {why} --"));
                                            }
                                            match ran {
                                                Err(e) => w.note(&format!("-- map: {e} --")),
                                                Ok(exit) => {
                                                    // Replay everything the board said
                                                    // while it was not being watched.
                                                    w.feed(&exit.buffered);
                                                    // `play`'s own tracking arm did
                                                    // not run while the view owned
                                                    // the screen; adopt what it
                                                    // learned instead of forgetting
                                                    // every step taken with the map
                                                    // open.
                                                    here = exit.here;
                                                    // The view folded the blocks it saw
                                                    // into that fix as they arrived, which
                                                    // is what the death log wants too.
                                                    death_fix = exit.here;
                                                    if let Some(why) = &exit.interrupted {
                                                        w.note(&format!("-- {why} --"));
                                                    }
                                                    // The map consumed the events while
                                                    // it owned the screen, so a death it
                                                    // closed on is logged here from the
                                                    // fix it handed back, and the watch is
                                                    // told so the prompt that follows does
                                                    // not log it twice.
                                                    if exit.interrupted.as_deref() == Some(crate::mapview::DIED_CLOSED) {
                                                        deaths.mark_dead();
                                                        if let Some(name) = session.character_name() {
                                                            let namer = |id| {
                                                                g.room(id).map(|r| r.name.clone()).unwrap_or_default()
                                                            };
                                                            w.note(&match crate::deathlog::log_death(
                                                                &name,
                                                                death_fix,
                                                                namer,
                                                                &crate::deathlog::path(),
                                                            ) {
                                                                Ok(death) => format!("-- death logged: {} --", death.line()),
                                                                Err(e) => format!("-- death not logged: {e} --"),
                                                            });
                                                        }
                                                    }
                                                    match exit.action {
                                                        // The map's quit ends the program,
                                                        // and a window cannot do that. The
                                                        // front end's Ctrl-Q is how the
                                                        // client exits, so this closes the
                                                        // map and nothing more.
                                                        crate::mapview::ViewAction::Quit => {}
                                                        crate::mapview::ViewAction::Save(l) => {
                                                            let dir = crate::loops::dir();
                                                            w.note(&match l.save(&dir) {
                                                                Ok(path) => format!(
                                                                    "-- saved {} stops as {:?} in {} (/farm {} to walk it) --",
                                                                    l.stops.len(), l.name, path.display(), l.name
                                                                ),
                                                                Err(e) => format!("-- loop: {e} --"),
                                                            });
                                                        }
                                                        crate::mapview::ViewAction::Roam(_) if job.is_some() => {
                                                            w.note("-- something is already driving (Ctrl-F to take over) --");
                                                        }
                                                        crate::mapview::ViewAction::Roam(walls) => {
                                                            let fenced = walls.len();
                                                            match start_roam(session.clone(), walls, here, notices.clone()) {
                                                                Ok(started) => {
                                                                    w.note(&format!(
                                                                        "-- roaming, fenced out of {fenced} rooms (Ctrl-F to take over) --"
                                                                    ));
                                                                    phase_rx = Some(started.phase.clone());
                                                                    job = Some(started);
                                                                }
                                                                Err(e) => w.note(&format!("-- roam: {e} --")),
                                                            }
                                                        }
                                                        crate::mapview::ViewAction::Go(to) if job.is_some() => {
                                                            w.note("-- something is already driving (Ctrl-F to take over) --");
                                                            let _ = to;
                                                        }
                                                        crate::mapview::ViewAction::Go(to) => {
                                                            let name = g.room(to).map(|r| r.name.clone()).unwrap_or_default();
                                                            match start_go(
                                                                session.clone(),
                                                                g.clone(),
                                                                here.confirmed(),
                                                                to,
                                                                assist_config.clone(),
                                                                notices.clone(),
                                                            ) {
                                                                Err(e) => w.note(&format!("-- {e} --")),
                                                                Ok(started) => {
                                                                    w.note(&format!(
                                                                        "-- walking to {name} [{}/{}] (Ctrl-F to take over) --",
                                                                        to.map, to.room
                                                                    ));
                                                                    phase_rx = Some(started.phase.clone());
                                                                    job = Some(started);
                                                                }
                                                            }
                                                        }
                                                        crate::mapview::ViewAction::Recover(to) if job.is_some() => {
                                                            w.note("-- something is already driving (Ctrl-F to take over) --");
                                                            let _ = to;
                                                        }
                                                        crate::mapview::ViewAction::Recover(to) => {
                                                            if let Some(started) = recover_to(
                                                                w,
                                                                session.clone(),
                                                                g.clone(),
                                                                here.confirmed(),
                                                                recover_marks.safe,
                                                                to,
                                                                notices.clone(),
                                                            ) {
                                                                phase_rx = Some(started.phase.clone());
                                                                job = Some(started);
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                    },
                                    _ => w.note(&format!(
                                        "-- map: no room database at {} --",
                                        world_path.display()
                                    )),
                                }
                            }
                            KeyOutcome::Refuse(why) => w.note(&format!("-- {why} --")),
                            KeyOutcome::Help => w.note(help_text()),
                            KeyOutcome::TakeOver => {
                                if let Some(j) = job.take() {
                                    j.handle.abort();
                                    phase_rx = None;
                                    // The keyboard is a person again:
                                    // flood control back off.
                                    session.set_pace(std::time::Duration::ZERO);
                                    w.note(&format!("-- {} stopped; you have the keyboard --", j.what));
                                }
                            }
                            KeyOutcome::ToggleAssist => {
                                // Only on the way ON: an assist already
                                // running has to be allowed to stop.
                                if assist.is_none()
                                    && let Err(why) = needs_name(&session)
                                {
                                    w.note(&format!("-- {why} --"));
                                    w.event(EventKind::AssistRefused(why));
                                } else {
                                    let on = assist.take().is_none();
                                    if on {
                                        // Fresh on every start: latches from
                                        // an earlier stretch describe fights
                                        // that are over.
                                        let (bot, casts) = new_assist(&session, &assist_config);
                                        assist = Some(bot);
                                        assist_casts = Some(casts);
                                    } else {
                                        assist_casts = None;
                                    }
                                    assist_watch =
                                        crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
                                    assist_book_seen = usize::MAX;
                                    // A running job owns the connection and
                                    // never hears the assist, so the press
                                    // reaches it through the session's
                                    // switch, which its live settings
                                    // follow: off turns every policy off at
                                    // its next decision, on restores the
                                    // profile's. A walk already stopped to
                                    // defend finishes that defence either
                                    // way.
                                    session.set_bot_on(on);
                                    match (&job, on) {
                                        (Some(j), true) => {
                                            w.note(&format!("-- bot on: the {} runs the profile's policies from here (/bot to stop) --", j.what));
                                        }
                                        (Some(j), false) => {
                                            w.note(&format!("-- bot off: the {} does nothing automatic from here --", j.what));
                                        }
                                        (None, true) => w.note("-- bot on: fighting and looting beside you (/bot to stop) --"),
                                        (None, false) => w.note("-- bot off --"),
                                    }
                                }
                            }
                            // The front end handles these and never sends
                            // them here: quitting and the window list are
                            // its business, not one window's.
                            KeyOutcome::Quit
                            | KeyOutcome::QuitNow
                            | KeyOutcome::NewWindow { .. }
                            | KeyOutcome::CloseWindow
                            | KeyOutcome::Windows
                            | KeyOutcome::Switch(_) => {}
                            KeyOutcome::Continue => {}
                        }
                        w.set_bar(bar_text(&state_rx, job.as_ref(), here, &rates, level, assist.is_some(), &session.party(), cols), job.is_some());
                    }
                }
            }
        }
    };

    // A closed line is closed whoever closed it: `/disconnect`, the
    // board hanging up, the state watch going away, or the front end
    // letting go of this window. Dropping a `Job` only DETACHES its
    // task, so a runner would otherwise go on sending into a dead
    // socket after the window was back in its disconnected state.
    if let Some(j) = job.take() {
        j.handle.abort();
    }
    session.close();
    end
}

/// What the assist knows about the pools when the profile leaves
/// `bot.max_hp` at zero. Asked of the board with the same `health` a
/// job sends, once per realm entry, and applied to every rebuild of the
/// assist's config: the rule [`crate::live::Live::learned_vitals`]
/// gives a job.
enum AssistVitals {
    Unknown,
    Asking(tokio::task::JoinHandle<Option<crate::farm::Vitals>>),
    Known(crate::farm::Vitals),
    /// The board did not answer in time. Not asked again until the
    /// character enters the realm again.
    Unanswered,
}

/// The profile's table with the learned pools filled in where the
/// profile left them at zero. A profile that names `max_hp` is
/// believed over the board, as it is for a job.
fn with_vitals(mut cfg: crate::bot::BotConfig, vitals: &AssistVitals) -> crate::bot::BotConfig {
    if cfg.max_hp == 0
        && let AssistVitals::Known(v) = vitals
    {
        cfg.max_hp = v.max_hp;
        cfg.max_mana = v.max_mana;
    }
    cfg
}
