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

use crate::screen::Screen;
use crate::session::{Capture, Session};
use crate::settings::Settings;
use crate::tui::{
    ContentCache, Job, KeyOutcome, LEVEL_POLL, LobbyStep, apply_settings, assist_config_for,
    assist_tick, content_path, describe_loops, finish_locator, handover_actions, help_text,
    here_or, import_loop, lobby_step, needs_username, new_assist, on_realm_entry, render_status,
    start_bank, start_farm, start_go, start_roam, start_where,
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
}

/// What a window sends the front end.
#[derive(Debug)]
pub enum FrontMsg {
    /// The screen or the bar changed. Repaint when this is the active window.
    Changed { window: WindowId },
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
    let info = Arc::new(Mutex::new(WindowInfo::default()));
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
    window.sync_info(false);
    tokio::spawn(run(window, first));
    WindowHandle {
        id,
        msgs: msgs_tx,
        keys: keys_tx,
        screen,
        info,
    }
}

impl Window {
    fn changed(&self) {
        let _ = self.front.send(FrontMsg::Changed { window: self.id });
    }

    fn note(&self, text: &str) {
        self.screen.lock().expect("screen lock").note(text);
        self.changed();
    }

    fn feed(&self, bytes: &[u8]) {
        self.screen.lock().expect("screen lock").feed(bytes);
        self.changed();
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
            self.changed();
        }
    }

    /// Connection facts from the settings, for `/windows` and the bar.
    fn sync_info(&self, connected: bool) {
        let profile = self.settings.profile();
        let mut info = self.info.lock().expect("info lock");
        info.host = profile.host.clone();
        info.port = profile.port;
        info.username = profile.username.clone();
        info.connected = connected;
        info.dirty = self.settings.dirty();
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        self.screen
            .lock()
            .expect("screen lock")
            .resize(rows.saturating_sub(2), cols);
        self.changed();
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
    /// The line closed, by the board or by `/disconnect`.
    Closed,
    /// The front end dropped the handle. The task ends.
    Ended,
}

/// What the disconnected state returned.
enum Idle {
    Connect,
    Ended,
}

async fn run(mut w: Window, first: Option<KeyOutcome>) {
    // A window opened on a profile that already names a host connects
    // without being asked, the way the old lobby did. Only on the way
    // in: a line that closes lands back in the disconnected state and
    // waits there for `/connect`.
    let mut first = first.or_else(|| {
        (!w.settings.profile().host.is_empty()).then_some(KeyOutcome::Connect { target: None })
    });
    loop {
        match disconnected(&mut w, first.take()).await {
            Idle::Connect => {}
            Idle::Ended => break,
        }
        let profile = w.settings.profile().clone();
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
                continue;
            }
        };
        // Interactive play is not paced. `pace_ms` is flood control,
        // which is for automation. The session keeps the real profile,
        // so `/farm` can put its pace back.
        session.set_pace(std::time::Duration::ZERO);
        w.sync_info(true);
        w.event(EventKind::Connected);
        let end = play(&mut w, session).await;
        w.sync_info(false);
        w.event(EventKind::Disconnected);
        match end {
            PlayEnd::Closed => w.note("-- disconnected. /connect to go back --"),
            PlayEnd::Ended => break,
        }
    }
    let _ = w.front.send(FrontMsg::Ended { window: w.id });
}

/// The window with no session: settings commands, `/connect`, `/help`.
/// `quit_armed` exists for `lobby_step`'s signature. `/quit` never
/// reaches a window, the front end owns it.
async fn disconnected(w: &mut Window, first: Option<KeyOutcome>) -> Idle {
    let mut quit_armed = false;
    let mut pending = first;
    loop {
        let outcome = match pending.take() {
            Some(o) => o,
            None => match w.msgs.recv().await {
                None => return Idle::Ended,
                Some(WindowMsg::Resize { rows, cols }) => {
                    w.resize(rows, cols);
                    continue;
                }
                Some(WindowMsg::Outcome(o)) => o,
            },
        };
        let (step, text) = lobby_step(outcome, &mut w.settings, &mut quit_armed);
        w.sync_info(false);
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
    target: &str,
    job: Option<&Job>,
    here: crate::lost::Fix,
    exp_per_hour: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
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
    render_status(&state, std::time::Instant::now(), target, phase.as_ref(), room_id, exp_per_hour, level, assist, cols as usize)
}

/// One connected session, until the line closes or the front end lets go.
///
/// The board's bytes go into the window's screen verbatim. The two
/// reserved rows are the front end's business now, so this loop only
/// keeps [`WindowInfo::bar`] current and says when something changed.
async fn play(w: &mut Window, session: Arc<Session>) -> PlayEnd {
    let mut raw_rx = session.raw();
    let mut events = session.events();
    let mut state_rx = session.state();
    // Bound once because it is a connection-time fact: this session was
    // opened against that board and goes on speaking its dialect. A
    // `/set target` mid-session applies at the next `/connect`, and the
    // bar goes on naming the board actually on the other end.
    let target = match session.profile().target {
        crate::dialect::Target::MbbsEmu => "mbbs",
        crate::dialect::Target::RustServer => "rust",
    };

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
    // Experience rate, counted from the board's award lines. Runs for the
    // whole session, not just while a farm is attached: a hand-played
    // stretch is worth measuring too.
    let mut exp = crate::progress::ExpMeter::default();
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
    // The assist: a bot that fights and loots, rests, heals and hides
    // BESIDE the operator while no farm runs. `/bot` toggles it; the
    // profile's `assist_play` starts it on. Rebuilt on every toggle-on
    // so its latches start clean.
    let mut assist: Option<crate::bot::Bot> = None;
    let mut assist_heal_state: Option<crate::sheet::HealState> = None;
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
        match needs_username(&session.profile()) {
            Ok(()) => {
                let (bot, heal) = new_assist(&session, &assist_config);
                assist = Some(bot);
                assist_heal_state = Some(heal);
            }
            Err(why) => assist_refused = Some(why),
        }
    }
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
    // Named apart from the job-handle `started` bound inside the Farm
    // and Go match arms below (`Ok(started) => { job = Some(started); }`)
    // on purpose: a reset written as `started = Instant::now()` inside
    // those arms would silently assign the SHADOWED job binding instead
    // of this clock, compile cleanly, and reset nothing.
    let mut exp_since = std::time::Instant::now();

    if let Some(why) = &assist_refused {
        w.note(&format!("-- bot assist not started: {why} --"));
        w.event(EventKind::AssistRefused(why.clone()));
    }
    w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());

    let end = loop {
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    w.feed(&bytes);
                    w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break PlayEnd::Closed, // disconnected
            },
            ev = events.recv() => {
                // The display comes from raw passthrough, so nothing is
                // rendered here — only the counters and the assist.
                if let Ok(cor) = &ev {
                    if let Some(present) = crate::dialect::realm_presence(&cor.event) {
                        // Entering is worth asking about straight away;
                        // waiting out the full period would leave the bar
                        // blank for the first minute of every session.
                        if present && !in_realm {
                            on_realm_entry(&session, content.clone());
                        }
                        in_realm = present;
                    }
                    if let crate::events::Event::Line(line) = &cor.event {
                        if let Some(p) = crate::progress::level_progress(line) {
                            level = Some(p);
                        }
                        let before = exp.total();
                        exp.observe(line);
                        if exp.total() != before {
                            w.set_bar(bar_text(&state_rx, target, job.as_ref(), here,
                                    exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
                        }
                        // The carried balance is the session's own
                        // answer now (`Session::capabilities`), fed
                        // centrally off whichever `i` anyone sends —
                        // `play` used to keep a second, private
                        // `PurseMeter` fed the same way and never read
                        // it. One tracker for one fact.
                    }
                    // While a farm runs it owns the connection outright;
                    // the assist only drives a hand-played session.
                    if job.is_none()
                        && let (Some(bot), Some(heal)) =
                            (assist.as_mut(), assist_heal_state.as_mut())
                    {
                        let clock = state_rx.borrow().ticks.round.clone();
                        let refusals = assist_tick(
                            &session,
                            &assist_config,
                            &durations,
                            bot,
                            heal,
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
            changed = state_rx.changed() => {
                if changed.is_err() { break PlayEnd::Closed; }
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
                w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
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
                        let (bot, heal) = new_assist(&session, &assist_config);
                        assist = Some(bot);
                        assist_heal_state = Some(heal);
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
                w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
            }
            _ = tick_paint.tick() => {
                w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
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
                        w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
                    }
                    WindowMsg::Outcome(outcome) => {
                        if let Some(applied) = apply_settings(&outcome, &mut w.settings) {
                            w.note(&applied.note);
                            w.sync_info(true);
                            if applied.profile_changed {
                                session.set_profile(w.settings.profile().clone());
                            }
                            if applied.bot_changed {
                                assist_config = assist_config_for(w.settings.profile());
                                if assist.is_some() {
                                    let (bot, heal) = new_assist(&session, &assist_config);
                                    assist = Some(bot);
                                    assist_heal_state = Some(heal);
                                    assist_watch = crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
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
                            KeyOutcome::Disconnect => break PlayEnd::Closed,
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
                                    match start_farm(session.clone(), loop_name.as_deref()) {
                                        Ok(started) => {
                                            // Fresh figures for a fresh
                                            // job: the total and the
                                            // clock it is divided by
                                            // reset together, or the
                                            // rate reads as a spike or a
                                            // sink instead of the truth.
                                            exp.reset();
                                            exp_since = std::time::Instant::now();
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
                                            Err(refusal) => w.note(&refusal.lines().join("\n")),
                                            Ok(to) => {
                                                let name = g.room(to).map(|r| r.name.clone()).unwrap_or_default();
                                                let steps = here
                                                    .confirmed()
                                                    .and_then(|f| g.route(f, to))
                                                    .map(|r| format!(", {} steps", r.len()))
                                                    .unwrap_or_default();
                                                let how = if assist.is_some() { "walking" } else { "running" };
                                                match start_go(
                                                    session.clone(),
                                                    g.clone(),
                                                    here.confirmed(),
                                                    to,
                                                    assist_config.clone(),
                                                    assist.is_some(),
                                                ) {
                                                    Err(e) => w.note(&format!("-- {e} --")),
                                                    Ok(started) => {
                                                        // See the StartFarm arm
                                                        // above: total and clock
                                                        // reset together.
                                                        exp.reset();
                                                        exp_since = std::time::Instant::now();
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
                                            assist.is_some(),
                                        ) {
                                            Err(e) => w.note(&format!("-- {e} --")),
                                            Ok(started) => {
                                                // See the StartFarm arm above:
                                                // total and clock reset together.
                                                exp.reset();
                                                exp_since = std::time::Instant::now();
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
                                        Err(refusal) => w.note(&refusal.lines().join("\n")),
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
                                        Err(refusal) => w.note(&refusal.lines().join("\n")),
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
                                            );
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
                                                        exp.observe(line);
                                                    }
                                                    if job.is_none()
                                                        && let (Some(bot), Some(heal)) =
                                                            (assist.as_mut(), assist_heal_state.as_mut())
                                                    {
                                                        let refusals = assist_tick(
                                                            &session,
                                                            &assist_config,
                                                            &durations,
                                                            bot,
                                                            heal,
                                                            &mut assist_watch,
                                                            &mut assist_book_seen,
                                                            &assist_clock,
                                                            cor,
                                                            std::time::Instant::now(),
                                                        );
                                                        // The map owns the screen, so there
                                                        // is no `note` to print through.
                                                        for why in refusals {
                                                            eprintln!("-- {why} --");
                                                        }
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
                                            w.takeover(false);
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
                                                    if let Some(why) = &exit.interrupted {
                                                        w.note(&format!("-- {why} --"));
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
                                                            match start_roam(session.clone(), walls, here) {
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
                                                                assist.is_some(),
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
                                    && let Err(why) = needs_username(&session.profile())
                                {
                                    w.note(&format!("-- {why} --"));
                                } else {
                                    let on = assist.take().is_none();
                                    if on {
                                        // Fresh on every start: latches from
                                        // an earlier stretch describe fights
                                        // that are over.
                                        let (bot, heal) = new_assist(&session, &assist_config);
                                        assist = Some(bot);
                                        assist_heal_state = Some(heal);
                                    } else {
                                        assist_heal_state = None;
                                    }
                                    assist_watch =
                                        crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
                                    assist_book_seen = usize::MAX;
                                    // A running job owns the connection and
                                    // never hears the assist, so the toggle
                                    // reaches it the only way it can: the
                                    // session's fight switch, which its
                                    // travel guard reads at every decision.
                                    // A walk already stopped to defend
                                    // finishes that defence either way.
                                    match (&job, on) {
                                        (Some(j), true) => {
                                            session.travel_fights().set(true);
                                            w.note(&format!("-- bot assist on: the {} fights what it meets from here (/bot to stop) --", j.what));
                                        }
                                        (Some(j), false) => {
                                            session.travel_fights().set(false);
                                            w.note(&format!("-- bot assist off: the {} walks past fights from here --", j.what));
                                        }
                                        (None, true) => w.note("-- bot assist on: fighting and looting beside you (/bot to stop) --"),
                                        (None, false) => w.note("-- bot assist off --"),
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
                        w.set_bar(bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols), job.is_some());
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

