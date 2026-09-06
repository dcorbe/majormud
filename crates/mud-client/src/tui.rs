//! Interactive terminal client: raw ANSI passthrough into a DECSTBM
//! scroll region, with a status bar and a local-editing input line on
//! the two reserved bottom rows.

use std::io::Write as _;
use std::sync::Arc;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::session::{GameState, Session};

/// How often the client asks the board where it stands against the next
/// level. Slow on purpose: the answer changes by one kill at a time, and
/// the command costs a round-trip on a connection a farm may be driving.
const LEVEL_POLL: std::time::Duration = std::time::Duration::from_secs(60);

/// Local line editor with history. Pure logic; the terminal loop feeds
/// it key events and repaints from `line()`/`cursor()`.
pub struct InputEditor {
    /// Current buffer as characters (cursor math is per-char).
    chars: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    /// None = editing the draft; Some(i) = viewing history[i].
    history_pos: Option<usize>,
    /// The draft stashed while browsing history.
    draft: Vec<char>,
}

impl InputEditor {
    pub fn new() -> Self {
        InputEditor {
            chars: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            draft: Vec::new(),
        }
    }

    pub fn line(&self) -> String {
        self.chars.iter().collect()
    }

    /// Cursor position in characters from the start of the line.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// Replace the characters `start..end` with `text` and park the
    /// cursor after it. Completion's one edit.
    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        let end = end.min(self.chars.len());
        let start = start.min(end);
        self.chars.splice(start..end, text.chars());
        self.cursor = start + text.chars().count();
    }

    /// Submit: returns the line, pushes it to history, clears the
    /// buffer and history cursor.
    pub fn take_line(&mut self) -> String {
        let line: String = std::mem::take(&mut self.chars).iter().collect();
        self.cursor = 0;
        self.history_pos = None;
        self.draft.clear();
        if !line.is_empty() {
            self.history.push(line.clone());
        }
        line
    }

    /// Recall older history (draft preserved on first step).
    pub fn history_prev(&mut self) {
        let next_pos = match self.history_pos {
            None if self.history.is_empty() => return,
            None => {
                self.draft = std::mem::take(&mut self.chars);
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_pos = Some(next_pos);
        self.chars = self.history[next_pos].chars().collect();
        self.cursor = self.chars.len();
    }

    /// Walk back toward the draft.
    pub fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.history_pos = Some(i + 1);
                self.chars = self.history[i + 1].chars().collect();
                self.cursor = self.chars.len();
            }
            Some(_) => {
                self.history_pos = None;
                self.chars = std::mem::take(&mut self.draft);
                self.cursor = self.chars.len();
            }
        }
    }
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// Why `play` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayEnd {
    /// The operator quit. The program ends.
    Quit,
    /// The line closed, by the board or by `/disconnect`. Back to the
    /// lobby with the settings intact.
    Closed,
}

/// The lobby's answer to one submitted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyStep {
    Stay,
    Connect,
    Quit,
}

/// The unsaved-settings refusal, in one place because the lobby and
/// play both ask it and a reworded copy would drift. The note comes
/// back the first time `/quit` is typed with settings unsaved, and arms
/// `armed` so that the next `/quit` goes through. `None` means the quit
/// may proceed. Disarming on any other command stays with the callers,
/// which are the only things that see the other commands.
pub fn quit_refusal(
    settings: &crate::settings::Settings,
    armed: &mut bool,
) -> Option<&'static str> {
    if settings.dirty() && !*armed {
        *armed = true;
        return Some("-- unsaved settings: /save, or /quit again --");
    }
    None
}

/// One outcome against the lobby: settings commands, `/connect`, `/help`
/// and `/quit`. Everything that needs a board says so. `quit_armed` is
/// the unsaved-settings refusal: set by a refused `/quit`, cleared by
/// any other command, so two `/quit` in a row exit.
pub fn lobby_step(
    outcome: KeyOutcome,
    settings: &mut crate::settings::Settings,
    quit_armed: &mut bool,
) -> (LobbyStep, Option<String>) {
    if !matches!(outcome, KeyOutcome::Quit | KeyOutcome::Continue) {
        *quit_armed = false;
    }
    if let Some(applied) = apply_settings(&outcome, settings) {
        return (LobbyStep::Stay, Some(applied.note));
    }
    match outcome {
        KeyOutcome::Continue => (LobbyStep::Stay, None),
        KeyOutcome::QuitNow => (LobbyStep::Quit, None),
        KeyOutcome::Quit => match quit_refusal(settings, quit_armed) {
            Some(why) => (LobbyStep::Stay, Some(why.to_string())),
            None => (LobbyStep::Quit, None),
        },
        KeyOutcome::Connect { target } => {
            if let Some(target) = target {
                let (host, port) = match connect_target(&target) {
                    Ok(t) => t,
                    Err(e) => return (LobbyStep::Stay, Some(format!("-- {e} --"))),
                };
                // Written as TOML, not with Rust's `Debug`: the two
                // spell an escape differently, and only one of them is
                // a string the parser will take back.
                let host = toml_edit::Value::from(host.as_str()).to_string();
                if let Err(e) = settings
                    .set("host", &host)
                    .and_then(|()| settings.set("port", &port.to_string()))
                {
                    return (LobbyStep::Stay, Some(format!("-- connect: {e} --")));
                }
            }
            if settings.profile().host.is_empty() {
                return (LobbyStep::Stay, Some("-- connect: no host: /connect host[:port] --".into()));
            }
            (LobbyStep::Connect, None)
        }
        KeyOutcome::Help => (LobbyStep::Stay, Some(help_text().to_string())),
        KeyOutcome::Note(text) => (LobbyStep::Stay, Some(text)),
        // Dashed the way play dashes it, so the same refusal reads the
        // same on both sides of a connection.
        KeyOutcome::Refuse(text) => (LobbyStep::Stay, Some(format!("-- {text} --"))),
        KeyOutcome::Send(_)
        | KeyOutcome::Raw(_)
        | KeyOutcome::Disconnect
        | KeyOutcome::StartFarm { .. }
        | KeyOutcome::Loops { .. }
        | KeyOutcome::ImportLoop { .. }
        | KeyOutcome::TakeOver
        | KeyOutcome::ToggleAssist
        | KeyOutcome::Go { .. }
        | KeyOutcome::Bank
        | KeyOutcome::Where
        | KeyOutcome::Room { .. }
        | KeyOutcome::Map { .. } => (
            LobbyStep::Stay,
            Some("-- not connected: /connect host[:port] --".into()),
        ),
        KeyOutcome::SetList { .. }
        | KeyOutcome::Set { .. }
        | KeyOutcome::Unset { .. }
        | KeyOutcome::Save { .. }
        | KeyOutcome::Load { .. } => (LobbyStep::Stay, None),
    }
}

/// How the lobby ended. `LobbyStep::Stay` has no place here: the lobby
/// only returns when it is done, so the caller has two cases and no
/// unreachable third.
enum LobbyEnd {
    Connect,
    Quit,
}

impl LobbyEnd {
    /// `None` while the lobby stays open.
    fn of(step: LobbyStep) -> Option<LobbyEnd> {
        match step {
            LobbyStep::Stay => None,
            LobbyStep::Connect => Some(LobbyEnd::Connect),
            LobbyStep::Quit => Some(LobbyEnd::Quit),
        }
    }
}

/// Run the client until the operator quits. A loop of two states: the
/// lobby, which has no session, and `play`, which has one. A profile
/// with a host connects at once. A line that closes lands in the lobby
/// with the settings intact.
///
/// The terminal is set up once, here, and put back once, here. Nothing
/// between those two points may return early, which is why the whole of
/// it lives in [`run_inner`]: an `?` there still comes back through this
/// function, and a client that exits on a write error must not leave the
/// terminal in raw mode with a scroll region set.
pub async fn run(
    settings: crate::settings::Settings,
    capture: Option<crate::session::Capture>,
) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    let result = run_inner(&mut out, settings, capture).await;
    let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let _ = out.write_all(b"\x1b[r");
    let _ = out.write_all(format!("\x1b[{rows};1H\r\n").as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
    result
}

/// The lobby-and-play loop itself. Every fallible call in here returns
/// through [`run`], which restores the terminal whatever the answer was.
async fn run_inner(
    out: &mut std::io::Stdout,
    mut settings: crate::settings::Settings,
    mut capture: Option<crate::session::Capture>,
) -> std::io::Result<()> {
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });
    let (_, rows) = crossterm::terminal::size()?;
    setup_region(out, rows)?;
    for warning in settings.warnings() {
        note(out, &format!("-- {warning} --"))?;
    }
    let mut cache = ContentCache::default();
    let mut connect_now = !settings.profile().host.is_empty();
    loop {
        if !connect_now {
            match lobby(out, &mut key_rx, &mut settings).await? {
                LobbyEnd::Connect => {}
                LobbyEnd::Quit => return Ok(()),
            }
        }
        connect_now = false;
        let profile = settings.profile().clone();
        // A capture names two files and creating them truncates, so it
        // records the first connection only. Cloned rather than taken:
        // a connect that fails opened nothing, so the next attempt still
        // has somewhere to record.
        let session = match Session::connect(&profile, capture.clone()).await {
            Ok(s) => {
                capture = None;
                Arc::new(s)
            }
            Err(e) => {
                note(out, &format!("-- connect {}:{}: {e} --", profile.host, profile.port))?;
                continue;
            }
        };
        // Interactive play is not paced. `pace_ms` is flood control,
        // which is for automation. The session keeps the real profile,
        // so `/farm` can put its pace back.
        session.set_pace(std::time::Duration::ZERO);
        match play(session, &mut settings, &mut cache, &mut key_rx, out).await? {
            PlayEnd::Quit => return Ok(()),
            PlayEnd::Closed => note(out, "-- disconnected. /connect to go back --")?,
        }
    }
}

/// The client with no session: an input line that takes the settings
/// commands, `/connect`, `/help` and `/quit`.
async fn lobby(
    out: &mut std::io::Stdout,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<TermEvent>,
    settings: &mut crate::settings::Settings,
) -> std::io::Result<LobbyEnd> {
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    setup_region(out, rows)?;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    let mut quit_armed = false;
    lobby_paint(out, settings, &editor, cols, rows)?;
    loop {
        let Some(ev) = key_rx.recv().await else {
            return Ok(LobbyEnd::Quit);
        };
        match ev {
            TermEvent::Resize(w, h) => {
                cols = w;
                rows = h;
                setup_region(out, rows)?;
            }
            TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                let outcome = handle_key(&key, &mut editor, &mut passthrough, false);
                // There is no board to pass keys through to.
                passthrough = false;
                let (step, text) = lobby_step(outcome, settings, &mut quit_armed);
                if let Some(text) = text {
                    note(out, &text)?;
                }
                if let Some(end) = LobbyEnd::of(step) {
                    return Ok(end);
                }
            }
            _ => {}
        }
        lobby_paint(out, settings, &editor, cols, rows)?;
    }
}

/// The lobby's bar and input line. The same two rows `play` paints,
/// with nothing to report but where `/connect` would go.
fn lobby_paint(
    out: &mut std::io::Stdout,
    settings: &crate::settings::Settings,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let profile = settings.profile();
    let mut status = format!("not connected  {}:{}", profile.host, profile.port);
    if settings.dirty() {
        status.push_str("  unsaved");
    }
    paint_bottom(out, &fit(&status, cols as usize), editor, rows)
}

/// One connected session, until it closes or the operator quits.
///
/// Layout: rows 1..h-2 are a DECSTBM scroll region receiving the raw
/// server stream verbatim; row h-1 is the status bar; row h is the
/// input line. The server-side cursor position is kept with DECSC/DECRC
/// around every passthrough write.
async fn play(
    session: Arc<Session>,
    settings: &mut crate::settings::Settings,
    cache: &mut ContentCache,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<TermEvent>,
    // Reborrowed all through the loop, hence the `mut` binding:
    // `&mut out` has to name a fresh borrow each time.
    mut out: &mut std::io::Stdout,
) -> std::io::Result<PlayEnd> {
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

    let (mut cols, mut rows) = crossterm::terminal::size()?;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    // The unsaved-settings refusal, exactly as the lobby makes it: set
    // by a refused `/quit`, cleared by any other command.
    let mut quit_armed = false;
    // Set while the runner drives this session; carries its phase for the
    // status bar and the handle needed to call it off.
    let mut job: Option<Job> = None;
    // A clone of the job's phase channel, kept separate so the select
    // can await it without borrowing `job` (which the repaint needs).
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
    let (graph, nav, spawns, content) = finish_locator(cache.world(&world_path), &session);
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
    // refusal is printed once the region is set up, below.
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
    // The countdowns in the bar move between events, so the bar is
    // repainted on its own short timer as well as on every event.
    let mut tick_paint = tokio::time::interval(std::time::Duration::from_millis(250));
    // A stalled loop must not catch up with a burst of repaints.
    tick_paint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The bar text the timer arm last painted, so a countdown that has
    // not moved a full tenth of a second does not repaint at all.
    let mut last_bar = String::new();
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

    setup_region(&mut out, rows)?;
    if let Some(why) = &assist_refused {
        note(&mut out, &format!("-- bot assist not started: {why} --"))?;
    }
    repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;

    let end = loop {
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    // Into the scroll region: restore server cursor,
                    // write verbatim, save it again.
                    out.write_all(b"\x1b8")?;
                    out.write_all(&bytes)?;
                    out.write_all(b"\x1b7")?;
                    repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break Ok(PlayEnd::Closed), // disconnected
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
                            repaint(&mut out, &state_rx, target, job.as_ref(), here,
                                    exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
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
                            note(&mut out, &format!("-- {why} --"))?;
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
                                    note(&mut out, &format!("-- room model: {entry} --"))?;
                                }
                            }
                        }
                    }
                }
            }
            changed = state_rx.changed() => {
                if changed.is_err() { break Ok(PlayEnd::Closed); }
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
                repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
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
                    note(&mut out, &format!("-- {what} ended: {} --", ended.label()))?;
                }
                repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
            }
            _ = tick_paint.tick() => {
                let bar = bar_text(&state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), cols);
                if bar != last_bar {
                    last_bar = bar;
                    repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                }
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
            ev = key_rx.recv() => {
                let Some(ev) = ev else { break Ok(PlayEnd::Quit) };
                match ev {
                    TermEvent::Resize(w, h) => {
                        cols = w;
                        rows = h;
                        setup_region(&mut out, rows)?;
                        repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                    }
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        let was = passthrough;
                        let outcome =
                            handle_key(&key, &mut editor, &mut passthrough, job.is_some());
                        if !matches!(outcome, KeyOutcome::Quit | KeyOutcome::Continue) {
                            quit_armed = false;
                        }
                        if let Some(applied) = apply_settings(&outcome, settings) {
                            note(&mut out, &applied.note)?;
                            if applied.profile_changed {
                                session.set_profile(settings.profile().clone());
                            }
                            if applied.bot_changed {
                                assist_config = assist_config_for(settings.profile());
                                if assist.is_some() {
                                    let (bot, heal) = new_assist(&session, &assist_config);
                                    assist = Some(bot);
                                    assist_heal_state = Some(heal);
                                    assist_watch = crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
                                    assist_book_seen = usize::MAX;
                                    note(&mut out, "-- bot assist rebuilt with the new settings --")?;
                                }
                            }
                        }
                        match outcome {
                            KeyOutcome::Quit => match quit_refusal(settings, &mut quit_armed) {
                                Some(why) => note(&mut out, why)?,
                                None => break Ok(PlayEnd::Quit),
                            },
                            KeyOutcome::QuitNow => break Ok(PlayEnd::Quit),
                            KeyOutcome::Send(line) => {
                                session.send(&line);
                            }
                            KeyOutcome::Raw(bytes) => session.send_raw(&bytes),
                            KeyOutcome::Note(text) => note(&mut out, &text)?,
                            // The shutdown itself is after the loop:
                            // the board can close the line too, and all
                            // three ways it ends need the same tidying.
                            KeyOutcome::Disconnect => break Ok(PlayEnd::Closed),
                            KeyOutcome::Connect { .. } => {
                                note(&mut out, "-- already connected: /disconnect first --")?;
                            }
                            KeyOutcome::SetList { .. }
                            | KeyOutcome::Set { .. }
                            | KeyOutcome::Unset { .. }
                            | KeyOutcome::Save { .. }
                            | KeyOutcome::Load { .. } => {}
                            KeyOutcome::Loops { name } => {
                                note(&mut out, &describe_loops(graph.as_deref(), name.as_deref()).join("\n"))?;
                            }
                            KeyOutcome::ImportLoop { file } => {
                                note(&mut out, &match graph.as_ref() {
                                    None => vec![format!(
                                        "-- loop: no room database at {} --",
                                        world_path.display()
                                    )],
                                    Some(g) => import_loop(g, std::path::Path::new(&file)),
                                }.join("\n"))?;
                            }
                            KeyOutcome::StartFarm { loop_name } => {
                                if let Some(j) = job.as_ref() {
                                    // Reachable mid-run now that the editor
                                    // works while farming: one job only.
                                    note(&mut out, &format!("-- {} already running (Ctrl-F to take over) --", j.what))?;
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
                                            note(&mut out, &match &loop_name {
                                                Some(n) => format!("-- farming loop {n:?} (Ctrl-F to take over) --"),
                                                None => "-- farm running (Ctrl-F to take over) --".to_string(),
                                            })?;
                                            phase_rx = Some(started.phase.clone());
                                            job = Some(started);
                                        }
                                        Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                    }
                                }
                            }
                            KeyOutcome::Go { target } => {
                                if let Some(j) = job.as_ref() {
                                    note(&mut out, &format!("-- {} already running (Ctrl-F to take over) --", j.what))?;
                                } else {
                                    match graph.as_ref() {
                                        // Naming the path is the whole
                                        // point: the default is relative,
                                        // so the commonest cause of this
                                        // is a working directory, and
                                        // "unknown room" would send the
                                        // operator hunting the wrong bug.
                                        None => note(&mut out, &format!(
                                            "-- go: no room database at {} --",
                                            world_path.display()
                                        ))?,
                                        Some(g) => match crate::go::resolve(g, here.confirmed(), &target) {
                                            Err(refusal) => note(&mut out, &refusal.lines().join("\n"))?,
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
                                                    Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                                    Ok(started) => {
                                                        // See the StartFarm arm
                                                        // above: total and clock
                                                        // reset together.
                                                        exp.reset();
                                                        exp_since = std::time::Instant::now();
                                                        note(&mut out, &format!(
                                                            "-- {how} to {name} [{}/{}]{steps} (Ctrl-F to take over) --",
                                                            to.map, to.room
                                                        ))?;
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
                                    note(&mut out, &format!("-- {} already running (Ctrl-F to take over) --", j.what))?;
                                } else {
                                    match graph.as_ref() {
                                        None => note(&mut out, &format!(
                                            "-- bank: no room database at {} --",
                                            world_path.display()
                                        ))?,
                                        Some(g) => match start_bank(
                                            session.clone(),
                                            g.clone(),
                                            here.confirmed(),
                                            assist_config.clone(),
                                            assist.is_some(),
                                        ) {
                                            Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                            Ok(started) => {
                                                // See the StartFarm arm above:
                                                // total and clock reset together.
                                                exp.reset();
                                                exp_since = std::time::Instant::now();
                                                note(&mut out, "-- banking (Ctrl-F to take over) --")?;
                                                phase_rx = Some(started.phase.clone());
                                                job = Some(started);
                                            }
                                        },
                                    }
                                }
                            }
                            KeyOutcome::Where => {
                                match graph.as_ref() {
                                    None => note(&mut out, &format!(
                                        "-- where: no room database at {} --",
                                        world_path.display()
                                    ))?,
                                    Some(g) => match start_where(session.clone(), g.clone(), here.confirmed()) {
                                        Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                        Ok(started) => {
                                            note(&mut out, "-- working out where you are (Ctrl-F to take over) --")?;
                                            phase_rx = Some(started.phase.clone());
                                            job = Some(started);
                                        }
                                    },
                                }
                            }
                            KeyOutcome::Room { target } => {
                                match (graph.as_ref(), spawns.as_ref()) {
                                    (Some(g), Some(s)) => match here_or(g, here.last_known(), &target) {
                                        Err(refusal) => note(&mut out, &refusal.lines().join("\n"))?,
                                        Ok(id) => match crate::spawn::Dossier::of(g, s, id) {
                                            None => note(&mut out, &format!("-- room: no room {}/{} --", id.map, id.room))?,
                                            Some(d) => note(&mut out, &d.lines().join("\n"))?,
                                        },
                                    },
                                    _ => note(&mut out, &format!(
                                        "-- room: no room database at {} --",
                                        world_path.display()
                                    ))?,
                                }
                            }
                            KeyOutcome::Map { target } => {
                                match (graph.as_ref(), spawns.as_ref()) {
                                    (Some(g), Some(s)) => match here_or(g, here.last_known(), &target) {
                                        Err(refusal) => note(&mut out, &refusal.lines().join("\n"))?,
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
                                            let exit = {
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
                                                crate::mapview::run(
                                                    &session,
                                                    &mut view,
                                                    key_rx,
                                                    &mut raw_rx,
                                                    &mut events,
                                                    nav.as_ref(),
                                                    &mut on_event,
                                                )
                                                .await?
                                            };
                                            // Hand the terminal back exactly
                                            // as `play` set it up, then replay
                                            // everything the board said while
                                            // it was not being watched.
                                            setup_region(&mut out, rows)?;
                                            out.write_all(b"\x1b8")?;
                                            out.write_all(&exit.buffered)?;
                                            out.write_all(b"\x1b7")?;
                                            // `play`'s own tracking arm did
                                            // not run while the view owned
                                            // the screen; adopt what it
                                            // learned instead of forgetting
                                            // every step taken with the map
                                            // open.
                                            here = exit.here;
                                            if let Some(why) = &exit.interrupted {
                                                note(&mut out, &format!("-- {why} --"))?;
                                            }
                                            match exit.action {
                                                crate::mapview::ViewAction::Quit => break Ok(PlayEnd::Quit),
                                                crate::mapview::ViewAction::Save(l) => {
                                                    let dir = crate::loops::dir();
                                                    note(&mut out, &match l.save(&dir) {
                                                        Ok(path) => format!(
                                                            "-- saved {} stops as {:?} in {} (/farm {} to walk it) --",
                                                            l.stops.len(), l.name, path.display(), l.name
                                                        ),
                                                        Err(e) => format!("-- loop: {e} --"),
                                                    })?;
                                                }
                                                crate::mapview::ViewAction::Roam(_) if job.is_some() => {
                                                    note(&mut out, "-- something is already driving (Ctrl-F to take over) --")?;
                                                }
                                                crate::mapview::ViewAction::Roam(walls) => {
                                                    let fenced = walls.len();
                                                    match start_roam(session.clone(), walls, here) {
                                                        Ok(started) => {
                                                            note(&mut out, &format!(
                                                                "-- roaming, fenced out of {fenced} rooms (Ctrl-F to take over) --"
                                                            ))?;
                                                            phase_rx = Some(started.phase.clone());
                                                            job = Some(started);
                                                        }
                                                        Err(e) => note(&mut out, &format!("-- roam: {e} --"))?,
                                                    }
                                                }
                                                crate::mapview::ViewAction::Go(to) if job.is_some() => {
                                                    note(&mut out, "-- something is already driving (Ctrl-F to take over) --")?;
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
                                                        Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                                        Ok(started) => {
                                                            note(&mut out, &format!(
                                                                "-- walking to {name} [{}/{}] (Ctrl-F to take over) --",
                                                                to.map, to.room
                                                            ))?;
                                                            phase_rx = Some(started.phase.clone());
                                                            job = Some(started);
                                                        }
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    },
                                    _ => note(&mut out, &format!(
                                        "-- map: no room database at {} --",
                                        world_path.display()
                                    ))?,
                                }
                            }
                            KeyOutcome::Refuse(why) => note(&mut out, &format!("-- {why} --"))?,
                            KeyOutcome::Help => note(&mut out, help_text())?,
                            KeyOutcome::TakeOver => {
                                if let Some(j) = job.take() {
                                    j.handle.abort();
                                    phase_rx = None;
                                    // The keyboard is a person again:
                                    // flood control back off.
                                    session.set_pace(std::time::Duration::ZERO);
                                    note(&mut out, &format!("-- {} stopped; you have the keyboard --", j.what))?;
                                }
                            }
                            KeyOutcome::ToggleAssist => {
                                // Only on the way ON: an assist already
                                // running has to be allowed to stop.
                                if assist.is_none()
                                    && let Err(why) = needs_username(&session.profile())
                                {
                                    note(&mut out, &format!("-- {why} --"))?;
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
                                            note(&mut out, &format!("-- bot assist on: the {} fights what it meets from here (/bot to stop) --", j.what))?;
                                        }
                                        (Some(j), false) => {
                                            session.travel_fights().set(false);
                                            note(&mut out, &format!("-- bot assist off: the {} walks past fights from here --", j.what))?;
                                        }
                                        (None, true) => note(&mut out, "-- bot assist on: fighting and looting beside you (/bot to stop) --")?,
                                        (None, false) => note(&mut out, "-- bot assist off --")?,
                                    }
                                }
                            }
                            KeyOutcome::Continue => {}
                        }
                        if passthrough != was {
                            let note = if passthrough {
                                "\r\n-- keys passed through to the board (Ctrl-P to return) --\r\n"
                            } else {
                                "\r\n-- back to the line editor --\r\n"
                            };
                            out.write_all(b"\x1b8")?;
                            out.write_all(note.as_bytes())?;
                            out.write_all(b"\x1b7")?;
                        }
                        repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_hour(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                    }
                    _ => {}
                }
            }
        }
    };

    // A closed line is closed whoever closed it: `/disconnect`, the
    // board hanging up, or the state watch going away. Dropping a `Job`
    // only DETACHES its task, so a runner would otherwise go on sending
    // into a dead socket after the operator was back in the lobby.
    if matches!(end, Ok(PlayEnd::Closed)) {
        if let Some(j) = job.take() {
            j.handle.abort();
        }
        session.close();
    }
    end
}

/// Returns true when the user asked to quit.
/// The bytes a key sends to the board in passthrough mode, or `None`
/// when the key is ours rather than the board's.
///
/// The board's full-screen screens (`train stats` opens one) are driven
/// by ANSI cursor sequences, which the line editor otherwise swallows for
/// its own cursor and history.
pub fn key_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    // The mode toggle is never forwarded, or leaving passthrough would
    // put a stray byte in whatever field is selected.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('q'))
    {
        return None;
    }
    Some(match key.code {
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        // A bare CR: an FSD screen is not line-oriented, and the LF of a
        // CRLF would be read as a second keystroke.
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => b"\x08".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-A..Ctrl-Z fold to 1..26.
            let up = c.to_ascii_uppercase();
            if up.is_ascii_uppercase() {
                vec![up as u8 - b'A' + 1]
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        _ => return None,
    })
}

/// Every verb `slash` claims, for completion. A verb here that `slash`
/// does not claim, or that `help_text` does not list, fails the test
/// `every_verb_in_the_completion_list_is_claimed_and_in_help`.
pub const VERBS: &[&str] = &[
    "/quit", "/farm", "/loop", "/bot", "/go", "/bank", "/where", "/room", "/map", "/help",
    "/set", "/unset", "/save", "/load", "/connect", "/disconnect",
];

/// What a keystroke asked the client to do. Returned rather than acted
/// on, because starting a farm needs to spawn a task and own its handle —
/// which is `play`'s business, not the key handler's.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Continue,
    Quit,
    /// A line for the board. The caller sends it: the handler has no
    /// session, so the lobby and play share it.
    Send(String),
    /// Passthrough bytes for the board, likewise.
    Raw(Vec<u8>),
    /// Print this and send nothing. Completion candidates, mostly.
    Note(String),
    /// Ctrl-Q: exit without the unsaved-settings check `/quit` makes.
    QuitNow,
    /// `/set` with a pattern or nothing: list.
    SetList {
        pattern: String,
    },
    /// `/set key value`: write.
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    Save {
        file: Option<String>,
    },
    Load {
        file: String,
    },
    /// `/connect [host[:port]]`. Unparsed here, `connect_target` parses.
    Connect {
        target: Option<String>,
    },
    Disconnect,
    /// Start the runner. `Some(name)` walks a loop from the library
    /// instead of the profile's own `[farm].circuit`.
    StartFarm {
        loop_name: Option<String>,
    },
    /// List the loop library, or show one loop's stops.
    Loops {
        name: Option<String>,
    },
    /// Read a MegaMud `.mp` path into the library.
    ImportLoop {
        file: String,
    },
    /// Take the keyboard back from whatever the client is driving.
    TakeOver,
    ToggleAssist,
    /// Walk to a room the operator named. Unparsed here on purpose: the
    /// graph decides what a name means, and the key handler has none.
    Go {
        target: String,
    },
    /// Walk to the bank and deposit above the keep floor, now.
    Bank,
    /// Print what the world database knows about a room. `None` means the
    /// one the character is standing in — unlike `/go`, a bare `/room` is
    /// the commonest form rather than a mistake.
    Room {
        target: Option<String>,
    },
    /// Work out which room the character is standing in, walking to
    /// narrow the candidates when the look alone cannot say.
    Where,
    /// Draw the plane around a room. `None` means the one the character
    /// is standing in, as for [`KeyOutcome::Room`].
    Map {
        target: Option<String>,
    },
    /// One of ours, got wrong. Print this and send nothing.
    Refuse(String),
    /// List the client's own slash commands and keybindings.
    Help,
}

/// What a submitted line asks the CLIENT to do, or `None` when it is the
/// board's business.
///
/// Only the known verbs are claimed. An unrecognised `/x` still goes to
/// the board, exactly as it did before this existed: the board says
/// unknown commands out loud rather than erroring, which is a survivable
/// answer, and swallowing every slash-prefixed line would silently eat
/// board syntax nobody has audited.
pub fn slash(line: &str) -> Option<KeyOutcome> {
    let line = line.trim();
    let (verb, rest) = match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim()),
        None => (line, ""),
    };
    match verb {
        "/quit" => Some(KeyOutcome::Quit),
        "/farm" => Some(KeyOutcome::StartFarm {
            loop_name: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/loop" => match rest.split_once(char::is_whitespace) {
            Some(("import", file)) if !file.trim().is_empty() => Some(KeyOutcome::ImportLoop {
                file: file.trim().to_string(),
            }),
            _ if rest == "import" => Some(KeyOutcome::Refuse(
                "loop: import what? try `/loop import paths/rocsloop.mp`".into(),
            )),
            _ => Some(KeyOutcome::Loops {
                name: (!rest.is_empty()).then(|| rest.to_string()),
            }),
        },
        "/bot" => Some(KeyOutcome::ToggleAssist),
        "/go" if rest.is_empty() => Some(KeyOutcome::Refuse(
            "go: where? try `/go 1/2324` or `/go Grungy Shop`".into(),
        )),
        "/go" => Some(KeyOutcome::Go {
            target: rest.to_string(),
        }),
        "/bank" => Some(KeyOutcome::Bank),
        "/where" => Some(KeyOutcome::Where),
        "/room" => Some(KeyOutcome::Room {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/map" => Some(KeyOutcome::Map {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/help" | "/?" => Some(KeyOutcome::Help),
        "/set" => match rest.split_once(char::is_whitespace) {
            Some((key, _)) if key.contains('*') => Some(KeyOutcome::Refuse(format!(
                "set: {key} is a pattern and lists. To change one key, name it without a star"
            ))),
            Some((key, value)) => Some(KeyOutcome::Set {
                key: key.to_string(),
                value: value.trim().to_string(),
            }),
            None => Some(KeyOutcome::SetList {
                pattern: rest.to_string(),
            }),
        },
        "/unset" if rest.is_empty() => Some(KeyOutcome::Refuse(
            "unset: which key? try `/unset bank.at`".into(),
        )),
        "/unset" => Some(KeyOutcome::Unset {
            key: rest.to_string(),
        }),
        "/save" => Some(KeyOutcome::Save {
            file: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/load" if rest.is_empty() => Some(KeyOutcome::Refuse(
            "load: which file? try `/load chars/dan.toml`".into(),
        )),
        "/load" => Some(KeyOutcome::Load {
            file: rest.to_string(),
        }),
        "/connect" => Some(KeyOutcome::Connect {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/disconnect" => Some(KeyOutcome::Disconnect),
        _ => None,
    }
}

/// Text for `/help`. A function rather than a `const` so it reads next
/// to `slash`, the thing it has to stay in sync with.
pub fn help_text() -> &'static str {
    "/quit                disconnect and exit
/farm [loop]         patrol the profile's circuit, or a named loop from the library
/loop [name]         list the loop library, or show one loop's stops
/loop import <file>  read a MegaMud .mp path into the library
/bot                 toggle the fight/loot assist (walk vs. run for /go)
/go <room>           walk to a room, by id (1/2324) or name
/bank                walk to the nearest bank and deposit the purse
/where               work out which room you're standing in
/room [target]       what the world database knows about a room (default: here)
/map [target]        draw the plane around a room (default: here)
/help, /?            this list
/set [pattern]       list settings, all or those the glob matches: bot, bot.rest*, *heal*
/set <key> <value>   change a setting now, as TOML, where a bare word is a string
/unset <key>         remove a setting so its default applies
/save [file]         write the settings and remember the file
/load <file>         read settings from a file
/connect [host[:port]]  connect, setting host and port when given
/disconnect          close the line and return to the lobby
Tab                  complete a slash verb or a setting key

Ctrl-F  take the keyboard back from a running farm/go/where/roam
Ctrl-P  toggle passthrough, for full-screen board screens (train stats)
Ctrl-Q  quit"
}

/// What a settings command did, for `play` and the lobby to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub note: String,
    /// The session's copy needs replacing.
    pub profile_changed: bool,
    /// A `bot.*` key moved, or a whole file came in: rebuild the assist.
    pub bot_changed: bool,
}

/// What one edit prints. Unsaved work is a fact about the settings and
/// not about the command, so a `/set` that wrote the value a key already
/// carried says nothing about saving: there is nothing to save.
fn edit_note(settings: &crate::settings::Settings, key: &str) -> String {
    let value = settings.value(key).unwrap_or_default();
    if settings.dirty() {
        format!("-- {key} = {value}, unsaved until /save --")
    } else {
        format!("-- {key} = {value} --")
    }
}

/// Run one settings command against the settings. `None` when the
/// outcome is not one. The note is ready to print.
pub fn apply_settings(outcome: &KeyOutcome, settings: &mut crate::settings::Settings) -> Option<Applied> {
    let said = |note: String| {
        Some(Applied {
            note,
            profile_changed: false,
            bot_changed: false,
        })
    };
    match outcome {
        KeyOutcome::SetList { pattern } => {
            let rows = settings.list(pattern);
            if rows.is_empty() {
                return said(format!("-- no setting matches {pattern} --"));
            }
            let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            let text = rows
                .iter()
                .map(|(k, v)| format!("{k:width$} = {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            said(text)
        }
        KeyOutcome::Set { key, value } => match settings.set(key, value) {
            Ok(()) => Some(Applied {
                note: edit_note(settings, key),
                profile_changed: true,
                bot_changed: key.starts_with("bot."),
            }),
            Err(e) => said(format!("-- set: {e} --")),
        },
        KeyOutcome::Unset { key } => match settings.unset(key) {
            Ok(()) => Some(Applied {
                note: edit_note(settings, key),
                profile_changed: true,
                bot_changed: key.starts_with("bot."),
            }),
            Err(e) => said(format!("-- unset: {e} --")),
        },
        KeyOutcome::Save { file } => {
            match settings.save(file.as_deref().map(std::path::Path::new)) {
                Ok(path) => said(format!("-- saved {} --", path.display())),
                Err(e) => said(format!("-- save: {e} --")),
            }
        }
        KeyOutcome::Load { file } => {
            match crate::settings::Settings::load(std::path::Path::new(file)) {
                Ok(loaded) => {
                    let mut note = format!("-- loaded {file}; connection keys apply at the next /connect --");
                    for warning in loaded.warnings() {
                        note.push_str(&format!("\n-- {warning} --"));
                    }
                    *settings = loaded;
                    Some(Applied {
                        note,
                        profile_changed: true,
                        bot_changed: true,
                    })
                }
                Err(e) => said(format!("-- load: {e} --")),
            }
        }
        _ => None,
    }
}

/// The assist's config: the profile's bot table, or attack and loot for
/// a profile without one, because attack and loot are the whole point of
/// asking for an assist.
pub fn assist_config_for(profile: &crate::profile::Profile) -> crate::bot::BotConfig {
    profile.bot.clone().unwrap_or(crate::bot::BotConfig {
        auto_combat: true,
        auto_get: true,
        ..Default::default()
    })
}

/// A job or the assist matches the character's own death line against
/// the username. Empty, they would never know the character died.
pub fn needs_username(profile: &crate::profile::Profile) -> Result<(), String> {
    if profile.username.is_empty() {
        return Err(
            "username is empty. /set username <name> so the runner can see your own death line"
                .into(),
        );
    }
    Ok(())
}

/// `host` or `host:port`. The port defaults to telnet's 23. An IPv6
/// literal is out of scope and is not parsed: a hostname or an IPv4
/// address is what this expects.
pub fn connect_target(arg: &str) -> Result<(String, u16), String> {
    let (host, port) = match arg.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("connect: {port} is not a port"))?;
            (host, port)
        }
        None => (arg, 23),
    };
    if host.is_empty() {
        return Err("connect: no host: /connect host[:port]".into());
    }
    Ok((host.to_string(), port))
}

/// One keystroke against the editor. Pure: what to send comes back as
/// `Send` or `Raw` and the caller sends it, so the lobby, which has
/// nothing to send to, and play share this. Public for the keyboard
/// contract tests.
pub fn handle_key(
    key: &KeyEvent,
    editor: &mut InputEditor,
    passthrough: &mut bool,
    farming: bool,
) -> KeyOutcome {
    // While the runner drives, the keyboard still works: composing costs
    // nothing (the editor is local), and a line sent on Enter is safe
    // beside the runner because ATTRIBUTION is — the typed command's
    // echo anchors its own answer, so a farm step's block can never be
    // claimed by it nor it by a step. The runner keeps sending while the
    // operator types; the two interleave FIFO on the one paced writer.
    // What stays reserved: Ctrl-F takes the keyboard back outright, and
    // Ctrl-P passthrough is refused — raw keys have no echo to anchor,
    // and an FSD screen mid-run would fight the runner for the parser.
    // Typing a MOVEMENT command is the operator desyncing the navigator
    // on purpose; recovery handles it like any other flee.
    if farming && key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('f') => KeyOutcome::TakeOver,
            KeyCode::Char('q') => KeyOutcome::QuitNow,
            _ => KeyOutcome::Continue,
        };
    }
    // Ctrl-P swaps between typing commands and driving a full-screen
    // board screen. Both are needed: the line editor wants the arrows for
    // its cursor and history, an FSD room wants them as cursor keys, and
    // nothing can satisfy both at once.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
        *passthrough = !*passthrough;
        return KeyOutcome::Continue;
    }
    if *passthrough {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return KeyOutcome::QuitNow;
        }
        return match key_bytes(key) {
            Some(bytes) => KeyOutcome::Raw(bytes),
            None => KeyOutcome::Continue,
        };
    }
    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyOutcome::QuitNow;
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => editor.insert(c),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Up => editor.history_prev(),
        KeyCode::Down => editor.history_next(),
        KeyCode::Tab => {
            if let Some(c) = crate::settings::complete(&editor.line(), editor.cursor(), VERBS) {
                editor.replace(c.start, c.end, &c.text);
                if !c.list.is_empty() {
                    return KeyOutcome::Note(c.list.join("  "));
                }
            }
        }
        KeyCode::Enter => {
            let line = editor.take_line();
            return match slash(&line) {
                Some(outcome) => outcome,
                None => KeyOutcome::Send(line),
            };
        }
        _ => {}
    }
    KeyOutcome::Continue
}

/// Build the assist and its heal state together, so every rebuild reads
/// the same way. The rebuilds are the profile's own start, the `/bot`
/// toggle, and a job handing the character back.
///
/// Both come out blank. The heal state has no sources and the bot is
/// told whatever `Session::capabilities` says about Stealth right now,
/// which on the first build of a session is nothing at all, because the
/// realm entry probe has not read the stat sheet or the spellbook yet.
/// [`assist_tick`] reads the sheet on its first tick after the probe has
/// stored one, and refreshes the hide flag on every tick.
fn new_assist(session: &Session, cfg: &crate::bot::BotConfig) -> (crate::bot::Bot, crate::sheet::HealState) {
    let bot = crate::bot::Bot::new(cfg.clone())
        .with_hide(session.capabilities().stealth > 0)
        .with_pack(session.pack_handle());
    (bot, crate::sheet::HealState::new(Vec::new()))
}

/// One correlated event for the assist while no job runs: keep its
/// sheet current, feed the heal state, cast when a heal is due, rearm a
/// rest that plainly failed, and let the bot decide the rest.
///
/// Returns any heal refusals discovered on a rebuild, for the caller to
/// print. Everything else it decides it sends itself.
///
/// The sheet is re-read on every tick because the assist is built
/// before the realm entry probe answers. An assist started by
/// `assist_play` used to keep the empty book and the missing Stealth it
/// was built with for the whole session, so it never cast a heal and
/// never hid.
#[allow(clippy::too_many_arguments)]
pub fn assist_tick(
    session: &Session,
    cfg: &crate::bot::BotConfig,
    durations: &std::collections::BTreeMap<String, u32>,
    bot: &mut crate::bot::Bot,
    heal: &mut crate::sheet::HealState,
    watch: &mut crate::farm::HealWatch,
    book_seen: &mut usize,
    clock: &crate::world::RoundClock,
    cor: &crate::correlate::Correlated,
    now: std::time::Instant,
) -> Vec<String> {
    let Some(spells) = session.book_len() else { return Vec::new() };
    bot.set_hide(session.capabilities().stealth > 0);
    // The assist is built before realm entry calls `set_content` and
    // hands the pack over, the same reason the hide flag is refreshed
    // here rather than trusted from the build.
    bot.set_pack(session.pack_handle());
    let mut refusals = Vec::new();
    if spells != *book_seen {
        let sheet = crate::farm::sheet_from(session, cfg, durations);
        *heal = crate::sheet::HealState::new(sheet.heals.0);
        *book_seen = spells;
        refusals = sheet.heals.1;
    }
    heal.on_event(cor, now);
    if watch.on_event(&cor.event) {
        bot.rearm();
    }
    if let crate::events::Event::Prompt { hp, .. } = &cor.event
        && let Some(cmd) = assist_heal(cfg, bot, heal, clock, *hp, now)
    {
        let id = session.send(&cmd);
        heal.on_sent(&cmd, id);
        watch.on_sent(&cmd);
    }
    for cmd in assist_actions(bot, cor) {
        session.send(&cmd);
        watch.on_sent(&cmd);
    }
    refusals
}

/// The assist's reply to one correlated event: the bot's own decisions,
/// plus the re-look the farm's pump would have made for it.
///
/// Room blocks are believed under the runner's own rule (farm.rs
/// `bot_sees`): only a block that answers a command — the operator's
/// look or step included — reaches the bot, so a stale or foreign
/// render cannot clear a latch or start a swing. And the board's own
/// fight-over announcement pokes a `look`: a fight's end says nothing
/// about who else is standing in the room, and an assist without the
/// poke killed one monster of a pack and stopped (live, 2026-08-01).
/// The poke's answer is attributed, names the survivors, and the next
/// engage comes off it — never off the fight chatter.
pub fn assist_actions(
    bot: &mut crate::bot::Bot,
    cor: &crate::correlate::Correlated,
) -> Vec<String> {
    // A `look <direction>` block names the neighbour's occupants, not
    // ours — feeding it to the bot is how an assist would attack a
    // monster standing in the room next door.
    let sees = !matches!(cor.event, crate::events::Event::RoomSeen(_))
        || (cor.answers.is_some() && !cor.elsewhere);
    let mut out: Vec<String> = if sees {
        bot.on_event(&cor.event)
            .into_iter()
            .map(|crate::bot::BotAction::Send(cmd)| cmd)
            .collect()
    } else {
        Vec::new()
    };
    if let crate::events::Event::Line(line) = &cor.event
        && crate::bot::is_combat_off(line)
    {
        out.push("look".into());
    }
    out
}

/// The assist's heal for one prompt: which kind the marks ask for, and
/// whether the heal state will cast it this round. The bot decides
/// nothing here, it only lends its percent arithmetic, so the assist and
/// the farm's stop loop read the same number.
pub fn assist_heal(
    cfg: &crate::bot::BotConfig,
    bot: &crate::bot::Bot,
    heal: &mut crate::sheet::HealState,
    clock: &crate::world::RoundClock,
    hp: i32,
    now: std::time::Instant,
) -> Option<String> {
    if !cfg.auto_heal || bot.fled() {
        return None;
    }
    let percent = bot.hp_percent(hp)?;
    let need = crate::bot::heal_need(cfg, percent)?;
    match heal.attempt(now, clock, need) {
        crate::sheet::CastAttempt::Send(cmd) => Some(cmd),
        _ => None,
    }
}

/// What the assist needs when a job hands the character back.
///
/// A job consumes every block its own steps are answered with, and the
/// assist is rebuilt fresh at the handover (see the job-end arm of
/// [`play`]), so the room the job ended in is one the assist has never
/// seen. A `/go` that arrives among monsters used to end exactly there:
/// the walk's last block listed a giant rat and a cave worm, the worm
/// lunged, and the assist stood idle until the operator typed the
/// backstab. Live, 2026-09-04. A blow landing on us is deliberately not
/// answered by a counter-attack either, see the `CombatHit` arm of
/// `Bot::on_event`, so nothing else would ever have started the fight.
///
/// The poke is a look, for the same reason the fight-over poke in
/// [`assist_actions`] is: the assist believes only a block that answers
/// a command, and a look's answer is attributed and names everyone
/// standing here now. Only a job that ended somewhere KNOWN is worth
/// it, since after a death the board is not even at a room prompt. And
/// only with the assist on, since nobody else acts on the answer.
pub fn handover_actions(ended: &crate::farm::Phase, assist: bool) -> Vec<String> {
    match ended.room() {
        Some(_) if assist => vec!["look".into()],
        _ => Vec::new(),
    }
}

fn setup_region(out: &mut impl std::io::Write, rows: u16) -> std::io::Result<()> {
    let region_bottom = rows.saturating_sub(2).max(1);
    // Set the region, park the server cursor at its bottom, save it.
    out.write_all(format!("\x1b[1;{region_bottom}r\x1b[{region_bottom};1H\x1b7").as_bytes())?;
    out.flush()
}

#[allow(clippy::too_many_arguments)]
fn redraw_bottom(
    out: &mut impl std::io::Write,
    state: &GameState,
    now: std::time::Instant,
    target: &str,
    phase: Option<&crate::farm::Phase>,
    room_id: crate::lost::Fix,
    exp_per_hour: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let status = render_status(state, now, target, phase, room_id, exp_per_hour, level, assist, cols as usize);
    paint_bottom(out, &status, editor, rows)
}

/// The two reserved rows: the status bar in reverse video, then the
/// input line with the cursor parked in it. One copy of the escape
/// sequence, so the lobby and a session lay out the same.
///
/// `status` is painted as given and must already be the terminal's
/// width. See [`fit`].
fn paint_bottom(
    out: &mut impl std::io::Write,
    status: &str,
    editor: &InputEditor,
    rows: u16,
) -> std::io::Result<()> {
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let line = editor.line();
    let cursor_col = 3 + editor.cursor() as u16;
    out.write_all(
        format!(
            "\x1b[{status_row};1H\x1b[2K\x1b[7m{status}\x1b[0m\
             \x1b[{input_row};1H\x1b[2K> {line}\x1b[{input_row};{cursor_col}H"
        )
        .as_bytes(),
    )?;
    out.flush()
}

/// One status line for both commands, exactly `width` characters.
///
/// `play` and `farm` used to build their own, so the same session looked
/// different depending on which command started it. The farm-only parts
/// are optional and simply absent when a person is driving: there is no
/// activity to report, and no room id, because the board only ever prints
/// a room's NAME — the number comes from the runner or the graph.
// Every field is an independent fact about the session and the bar is
// the one place they meet; grouping them into a struct would exist only
// to satisfy the lint. Same call as `repaint` and `redraw_bottom` above.
#[allow(clippy::too_many_arguments)]
pub fn render_status(
    state: &GameState,
    now: std::time::Instant,
    target: &str,
    phase: Option<&crate::farm::Phase>,
    room_id: crate::lost::Fix,
    exp_per_hour: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    width: usize,
) -> String {
    let mut s = String::new();
    // Who is driving the character. A farm outranks the assist because
    // it owns the connection outright while it runs; the assist only
    // acts when no farm does. Saying nothing when the assist is on is
    // what made a self-ended farm indistinguishable from a running one,
    // and sent the operator hunting a Ctrl-F regression that was not
    // there (2026-08-01).
    if let Some(phase) = phase {
        s.push_str(&format!("{} | ", phase.label()));
    } else if assist {
        s.push_str("assist | ");
    }
    s.push_str(&format!("HP {}", state.hp));
    if let Some(ma) = state.mana {
        s.push_str(&format!(" MA {ma}"));
    }
    if let Some(status) = &state.status {
        s.push_str(&format!(" ({})", status.word()));
    }
    s.push_str(&format!(" | {}", tick_readout(state, now)));
    if let Some(room) = &state.room {
        s.push_str(&format!(" | {}", room.name));
        if let Some(id) = room_id.last_known() {
            // A trailing `?` is the whole point of the type reaching the
            // bar: the operator can see the client has lost the thread
            // before /go routes from a room nobody is in.
            let sure = if room_id.confirmed().is_some() { "" } else { "?" };
            s.push_str(&format!(" [{}/{}{sure}]", id.map, id.room));
        }
    }
    if let Some(rate) = exp_per_hour {
        s.push_str(&format!(" | {rate} xp/hr"));
    }
    // How long until the next level, at the rate we are actually
    // earning. Refreshed on its own timer, so it goes stale between
    // ticks rather than jittering with every kill.
    if let Some(p) = level {
        s.push_str(&format!(
            " | L{}->{} {}",
            p.level,
            p.level + 1,
            crate::progress::eta_label(p.needed, exp_per_hour)
        ));
    }
    s.push_str(&format!(" | {target}"));
    fit(&s, width)
}

/// A string cut and padded to exactly `width` characters, with every
/// control character replaced by a space.
///
/// Defence in depth: nothing painted into a fixed row may contain a
/// control character, whatever built the string. A newline there scrolls
/// the terminal and the whole display appears to flash.
fn fit(s: &str, width: usize) -> String {
    let mut out: Vec<char> = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    out.truncate(width);
    while out.len() < width {
        out.push(' ');
    }
    out.into_iter().collect()
}

/// The clock countdowns: the round, then HP, then mana when the
/// character has a pool. A cycle nobody has observed yet shows `-`. The
/// second number in a pair is the rest or meditate cycle, shown only
/// while it runs.
fn tick_readout(state: &GameState, now: std::time::Instant) -> String {
    fn secs(d: Option<std::time::Duration>) -> String {
        match d {
            Some(d) => format!("{:.1}", d.as_secs_f64()),
            None => "-".into(),
        }
    }
    let t = &state.ticks;
    let mut s = format!("Tick {} | HP {}", secs(t.time_to_round(now)), secs(t.hp_natural.time_to_next(now)));
    if t.hp_rest.active() {
        s.push_str(&format!("/{}", secs(t.hp_rest.time_to_next(now))));
    }
    if state.mana.is_some() {
        s.push_str(&format!(" | MA {}", secs(t.mana_natural.time_to_next(now))));
        if t.mana_meditate.active() {
            s.push_str(&format!("/{}", secs(t.mana_meditate.time_to_next(now))));
        }
    }
    s
}

/// A one-line status bar with a scrolling region above it.
///
/// The same DECSTBM trick `play` uses, minus the input line: the terminal
/// is told to scroll only the rows above the last one, so ordinary
/// `println!`-style output flows past while the bottom row stays put.
///
/// This exists because a long unattended run is unreadable otherwise. The
/// feed alone cannot answer "what is it doing *now*" — waiting to depart,
/// travelling and wedged all look like silence.
pub struct StatusBar {
    rows: u16,
    cols: u16,
    out: std::io::Stdout,
    /// Suppresses redraws that would paint the identical line.
    last: String,
}

impl StatusBar {
    /// Reserve the bottom row. Returns `None` when stdout is not a
    /// terminal (piped to a file, or a CI run), where the escapes would
    /// be noise rather than a bar.
    pub fn enter() -> Option<StatusBar> {
        use std::io::IsTerminal;
        let out = std::io::stdout();
        if !out.is_terminal() {
            return None;
        }
        let (cols, rows) = crossterm::terminal::size().ok()?;
        let mut bar = StatusBar {
            rows,
            cols,
            out,
            last: String::new(),
        };
        let region_bottom = rows.saturating_sub(1).max(1);
        bar.write(&format!(
            "\x1b[1;{region_bottom}r\x1b[{region_bottom};1H"
        ));
        Some(bar)
    }

    fn write(&mut self, s: &str) {
        use std::io::Write;
        let _ = self.out.write_all(s.as_bytes());
        let _ = self.out.flush();
    }

    /// Print one feed line into the scrolling region.
    pub fn line(&mut self, text: &str) {
        let region_bottom = self.rows.saturating_sub(1).max(1);
        // Park the cursor in the region before writing, or the line lands
        // on the bar; repaint the bar afterwards because a scroll can
        // shift it.
        self.write(&format!("\x1b[{region_bottom};1H\r\n{text}"));
        let last = self.last.clone();
        self.paint(&last);
    }

    /// Update the bar. Cheap to call on every event: identical text is
    /// dropped rather than repainted.
    pub fn status(&mut self, text: &str) {
        if text == self.last {
            return;
        }
        self.last = text.to_string();
        let last = self.last.clone();
        self.paint(&last);
    }

    fn paint(&mut self, text: &str) {
        let row = self.rows.max(1);
        let width = self.cols as usize;
        let mut line: String = text.chars().take(width).collect();
        while line.chars().count() < width {
            line.push(' ');
        }
        // Save/restore around the bar so the scrolling region's own
        // cursor is left where the feed expects it.
        self.write(&format!("\x1b7\x1b[{row};1H\x1b[2K\x1b[7m{line}\x1b[0m\x1b8"));
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        // Give the terminal back: full scroll region, cursor below.
        let rows = self.rows.max(1);
        self.write(&format!("\x1b[r\x1b[{rows};1H\r\n"));
    }
}

/// A client-side job driving this session from inside `play` — a farm
/// run or a `/go` walk.
///
/// There is exactly one slot, because each owns the connection outright:
/// the navigator and the bot both send movement and both read room
/// blocks, so two of these at once would fight over the same events (see
/// [`crate::farm::run_farm`]).
pub struct Job {
    handle: tokio::task::JoinHandle<()>,
    phase: tokio::sync::watch::Receiver<crate::farm::Phase>,
    /// "farm" or "go", for the retirement notice.
    what: &'static str,
}

/// Print a one-off notice into the scrolling region without disturbing
/// the board's cursor.
///
/// Newlines are translated, because the terminal is in raw mode: a bare
/// `\n` drops a row without returning the carriage, so a multi-line
/// notice would stairstep off the right edge.
fn note(out: &mut impl std::io::Write, text: &str) -> std::io::Result<()> {
    out.write_all(b"\x1b8")?;
    out.write_all(format!("\r\n{}\r\n", text.replace('\n', "\r\n")).as_bytes())?;
    out.write_all(b"\x1b7")?;
    Ok(())
}

/// The status bar's text, without painting it. Same phase and room
/// lookups as `repaint`, so the timer arm can tell whether a fresh
/// repaint would actually change anything before it pays for one.
#[allow(clippy::too_many_arguments)]
fn bar_text(
    state_rx: &tokio::sync::watch::Receiver<GameState>,
    target: &str,
    job: Option<&Job>,
    here: crate::lost::Fix,
    exp_per_hour: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    cols: u16,
) -> String {
    let phase = job.map(|j| j.phase.borrow().clone());
    let room_id = match phase.as_ref().and_then(|p| p.room()) {
        Some(at) => crate::lost::Fix::Confirmed(at),
        None => here,
    };
    let state = state_rx.borrow().clone();
    render_status(&state, std::time::Instant::now(), target, phase.as_ref(), room_id, exp_per_hour, level, assist, cols as usize)
}

/// Redraw the bottom rows, reading the farm's phase when one is running.
#[allow(clippy::too_many_arguments)]
fn repaint(
    out: &mut impl std::io::Write,
    state_rx: &tokio::sync::watch::Receiver<GameState>,
    target: &str,
    job: Option<&Job>,
    here: crate::lost::Fix,
    exp_per_hour: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let phase = job.map(|j| j.phase.borrow().clone());
    // The runner's own belief wins while it drives -- it knows which of
    // two same-named rooms it walked to -- and the client's tracking
    // covers everything else.
    let room_id = match phase.as_ref().and_then(|p| p.room()) {
        Some(at) => crate::lost::Fix::Confirmed(at),
        None => here,
    };
    let state = state_rx.borrow().clone();
    redraw_bottom(
        out,
        &state,
        std::time::Instant::now(),
        target,
        phase.as_ref(),
        room_id,
        exp_per_hour,
        level,
        assist,
        editor,
        cols,
        rows,
    )
}

/// Start a farm run on an already-connected session.
///
/// Everything it needs comes from the profile the session was opened
/// with, so `/farm` needs no arguments. Errors are the operator's to read,
/// not a reason to drop the connection — being told "no [farm] table" and
/// staying logged in is strictly better than being thrown out.
///
/// Refused outright when the profile has no username, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The check lives here rather than at the call sites
/// so that a new caller cannot forget it. Public for the test that pins
/// the refusal.
pub fn start_farm(session: Arc<Session>, loop_name: Option<&str>) -> Result<Job, String> {
    let profile = session.profile();
    needs_username(&profile)?;
    // A named loop replaces the circuit, not the policy: every knob in
    // the profile's [farm] table -- the hp gates, the dwell budgets, the
    // nav limits -- still applies to it. The library holds routes, not
    // settings.
    let base = profile.farm.clone().unwrap_or_default();
    let graph = Arc::new(crate::graph::RoomGraph::load(&base.content)?);
    let cfg = match loop_name {
        None => profile
            .farm
            .clone()
            .ok_or("no [farm] table in the profile: nothing to patrol")?,
        Some(name) => {
            let l = crate::loops::load(&crate::loops::dir(), name)?;
            for warning in l.check_names(&graph) {
                eprintln!("loop {name}: {warning}");
            }
            l.to_farm(&base, &graph)?
        }
    };
    let plan = crate::farm::FarmPlan::build(&cfg, &graph)?;
    let bot = profile.bot.clone().unwrap_or_default();
    // Automation goes back under flood control. `play` unpaced this
    // session for the operator's keystrokes; the runner it is about to
    // hand the connection to cycled at loopback echo speed without this
    // (~40 look+attack commands in 400ms, run4 2026-08-01).
    Ok(spawn_run(session, graph, plan, bot, cfg, "farm"))
}

/// Roam the region the operator fenced, on an already-connected session.
///
/// The walls came off the map and are not stored anywhere: this is the
/// only thing that will ever see them, and when the run ends they are
/// gone. Everything else — the hp gates, the dwell budgets, the nav
/// limits — still comes from the profile's `[farm]` table, exactly as a
/// named loop does. The library holds routes; the profile holds policy.
///
/// Refused outright when the profile has no username, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The check lives here rather than at the call sites
/// so that a new caller cannot forget it. Public for the test that pins
/// the refusal.
pub fn start_roam(
    session: Arc<Session>,
    walls: crate::roam::Walls,
    here: crate::lost::Fix,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_username(&profile)?;
    let cfg = profile.farm.clone().unwrap_or_default();
    let graph = Arc::new(crate::graph::RoomGraph::load(&cfg.content)?);
    // Where the character stands is what the region is measured from, so
    // a roam started from an unknown position has nothing to measure.
    // Refusing beats guessing: the fence would be anchored somewhere
    // nobody is.
    let start = here.confirmed().ok_or(
        "nobody knows with any confidence where you are standing; /where first, then roam",
    )?;
    let plan = crate::farm::FarmPlan::roaming(start, walls, &graph)?;
    let bot = profile.bot.clone().unwrap_or_default();
    Ok(spawn_run(session, graph, plan, bot, cfg, "roam"))
}

/// Hand the connection to the runner and report what it did.
///
/// Shared by `/farm` and the map's roam so the two cannot describe the
/// same ending differently — the only thing that varies is whether laps
/// or rooms are the number that means anything.
fn spawn_run(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    plan: crate::farm::FarmPlan,
    bot: crate::bot::BotConfig,
    cfg: crate::farm::FarmConfig,
    what: &'static str,
) -> Job {
    // Automation goes back under flood control. `play` unpaced this
    // session for the operator's keystrokes; the runner it is about to
    // hand the connection to cycled at loopback echo speed without this
    // (~40 look+attack commands in 400ms, run4 2026-08-01).
    session.set_pace(session.profile().pace());
    let roaming = plan.roam.is_some();
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let end = match crate::farm::run_farm(&session, graph, &plan, &bot, &cfg, Some(&tx)).await {
            Ok((end, stats)) => crate::farm::Phase::Done {
                why: format!(
                    "{} ({} kills, {}{}{})",
                    match end {
                        crate::farm::FarmEnd::LoopsDone => "loops walked",
                        crate::farm::FarmEnd::TimeUp => "time up",
                        crate::farm::FarmEnd::Died => "died",
                        crate::farm::FarmEnd::TooHurt =>
                            "too hurt: travel interrupt budget spent",
                    },
                    stats.kills,
                    // A roam walks no circuits, so "0 loops" would be
                    // true and useless.
                    if roaming {
                        format!("{} rooms", stats.roamed)
                    } else {
                        format!("{} loops", stats.loops)
                    },
                    // The room model runs in shadow, and this is the only
                    // place a `/farm` run can report what it measured.
                    // Silent when it never disagreed.
                    stats
                        .divergence_summary()
                        .map(|d| format!("; room model: {d}"))
                        .unwrap_or_default(),
                    // Silent on a run that never went to a bank, the
                    // same rule the room model fragment follows.
                    if stats.deposits > 0 {
                        format!("; {} deposits", stats.deposits)
                    } else {
                        String::new()
                    },
                ),
                // `FarmEnd` carries no room; the shadow model's belief is
                // not confirmed enough to hand a caller as an arrival.
                at: None,
            },
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
        };
        let _ = tx.send(end);
    });
    Job {
        handle,
        phase: rx,
        what,
    }
}

/// Walk to a room on an already-connected session.
///
/// Walk versus run is `walking`, which is the `/bot` toggle read at the
/// moment the command was typed. Toggling `/bot` mid-walk deliberately
/// does not change a walk already in flight: the mode is captured here,
/// and a walk that changed its mind halfway would be very hard to
/// reason about from the keyboard.
///
/// Refused outright when the profile has no username, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The check lives here rather than at the call sites
/// so that a new caller cannot forget it. Public for the test that pins
/// the refusal.
pub fn start_go(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
    to: mud_core::content::RoomId,
    bot: crate::bot::BotConfig,
    walking: bool,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_username(&profile)?;
    let base = profile.farm.clone().unwrap_or_else(|| crate::farm::FarmConfig {
        // A profile with no [farm] table still gets a working `/go`; it
        // just needs to be told where the rooms live, and that is the
        // same place the locator already looked.
        content: content_path(&profile),
        ..Default::default()
    });
    let cfg = crate::go::go_config(&base, walking);
    // Automation goes back under flood control, exactly as a farm does:
    // `play` unpaced this session for the operator's keystrokes.
    session.set_pace(profile.pace());
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let end = match crate::go::run_go(&session, graph, hint, to, &bot, &cfg, Some(&tx)).await {
            Ok(crate::go::GoEnd::Arrived(at)) => crate::farm::Phase::Done {
                why: format!("arrived at {}/{}", at.map, at.room),
                at: Some(at),
            },
            // Where it stands matters more than why it stopped: a bare
            // "stopped" strands the operator worse than never trying. And
            // a travel interrupt is as solid a position as an arrival —
            // the character stopped there, it did not vanish.
            Ok(crate::go::GoEnd::Stopped(at)) => crate::farm::Phase::Done {
                why: format!(
                    "stopped at {}/{}: travel interrupt budget spent",
                    at.map, at.room
                ),
                at: Some(at),
            },
            Ok(crate::go::GoEnd::Died) => {
                crate::farm::Phase::Done { why: "died".into(), at: None }
            }
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
        };
        let _ = tx.send(end);
    });
    Ok(Job {
        handle,
        phase: rx,
        what: "go",
    })
}

/// `/bank`. The same job shape as `start_go`, ending in a `Phase::Done`
/// that says what was deposited and where, or why nothing was.
///
/// Refused outright when the profile has no username, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The check lives here rather than at the call sites
/// so that a new caller cannot forget it. Public for the test that pins
/// the refusal.
pub fn start_bank(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
    bot: crate::bot::BotConfig,
    walking: bool,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_username(&profile)?;
    let base = profile.farm.clone().unwrap_or_else(|| crate::farm::FarmConfig {
        content: content_path(&profile),
        ..Default::default()
    });
    let cfg = crate::go::go_config(&base, walking);
    session.set_pace(profile.pace());
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let end = match crate::bank::run_bank(&session, graph, hint, &bot, &cfg, Some(&tx)).await {
            Ok(crate::bank::ErrandEnd::Deposited { farthings, at, bank }) => {
                crate::farm::Phase::Done {
                    why: format!("deposited {farthings} copper farthings at {bank}"),
                    at: Some(at),
                }
            }
            Ok(crate::bank::ErrandEnd::Nothing(why)) => crate::farm::Phase::Done {
                why: format!("nothing deposited: {why}"),
                at: None,
            },
            Ok(crate::bank::ErrandEnd::Died) => {
                crate::farm::Phase::Done { why: "died".into(), at: None }
            }
            Ok(crate::bank::ErrandEnd::TimeUp) => {
                crate::farm::Phase::Done { why: "time up".into(), at: None }
            }
            Ok(crate::bank::ErrandEnd::TooHurt) => crate::farm::Phase::Done {
                why: "stopped: travel interrupt budget spent".into(),
                at: None,
            },
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
        };
        let _ = tx.send(end);
    });
    Ok(Job {
        handle,
        phase: rx,
        what: "bank",
    })
}

/// The room database this profile uses: `[farm].content` when it has
/// one, else the same default `mmc path` uses.
///
/// Ask the board where the character is, and walk until the answer is
/// forced.
///
/// A job rather than an inline answer because it SENDS: a look, and then
/// up to [`crate::lost::BUDGET`] steps. Everything that sends on this
/// connection goes through the one job slot, so that the runner and the
/// operator can never both be driving.
///
/// Refused outright when the profile has no username, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The check lives here rather than at the call sites
/// so that a new caller cannot forget it. Public for the test that pins
/// the refusal.
pub fn start_where(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
) -> Result<Job, String> {
    needs_username(&session.profile())?;
    session.set_pace(session.profile().pace());
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let nav = crate::nav::Navigator::new(graph.clone(), crate::nav::NavConfig::default())
            .with_capabilities(session.capabilities());
        let end = match crate::farm::look_around(&session, "the where look").await {
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
            Ok(seen) => {
                // An impossible id when there is no hint, so the one-hop
                // shortcut necessarily misses and the search runs.
                let hint = hint.unwrap_or(mud_core::content::RoomId { map: 0, room: 0 });
                match crate::lost::place(&session, &graph, &nav, hint, &seen).await {
                    Ok(p) => crate::farm::Phase::Placed {
                        at: p.at,
                        steps: p.steps,
                    },
                    Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
                }
            }
        };
        let _ = tx.send(end);
    });
    Ok(Job {
        handle,
        phase: rx,
        what: "where",
    })
}

/// That default is RELATIVE, so a `play` started anywhere but the repo
/// root finds nothing. Callers that refuse should say which path they
/// tried — "unknown room" is a very confusing way to learn about a
/// working directory.
fn content_path(profile: &crate::profile::Profile) -> std::path::PathBuf {
    profile
        .farm
        .as_ref()
        .map(|f| f.content.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("re/mmud_wgnt.sqlite"))
}

/// The room graph, and a navigator over it.
///
/// Best effort: the room database is how a name becomes a number, and
/// without it the bar simply shows the name, as it always did. The graph
/// is handed back alongside because `/go` needs to ask it questions the
/// navigator does not answer — which rooms carry a name, and how far
/// away they are.
/// The room a `/room` or `/map` argument means, defaulting to where the
/// character stands.
///
/// Shared rather than written twice: both verbs take the same optional
/// argument with the same meaning, and `go::resolve` already solves name
/// matching and ambiguity reporting for all three.
fn here_or(
    graph: &crate::graph::RoomGraph,
    here: Option<mud_core::content::RoomId>,
    target: &Option<String>,
) -> Result<mud_core::content::RoomId, crate::go::GoRefusal> {
    match target {
        Some(t) => crate::go::resolve(graph, here, t),
        // The client only knows where it is once a room block has been
        // localized, which is why this can fail at all.
        None => here.ok_or_else(|| {
            crate::go::GoRefusal::Unknown("where you are standing (walk a step first)".into())
        }),
    }
}

/// `/loop`: the library's names, or one loop's stops.
///
/// Names are checked against the graph as they are printed, because the
/// moment somebody looks at a loop is the moment to tell them it was
/// written for a different world.
fn describe_loops(graph: Option<&crate::graph::RoomGraph>, name: Option<&str>) -> Vec<String> {
    let dir = crate::loops::dir();
    let Some(name) = name else {
        let names = crate::loops::list(&dir);
        if names.is_empty() {
            return vec![format!(
                "no loops yet in {} (mark stops on /map and press s)",
                dir.display()
            )];
        }
        let mut out = vec![format!("{} loops in {}:", names.len(), dir.display())];
        out.extend(names.into_iter().map(|n| format!("  {n}")));
        return out;
    };

    let l = match crate::loops::load(&dir, name) {
        Ok(l) => l,
        Err(e) => return vec![format!("loop: {e}")],
    };
    let mut out = vec![format!(
        "{}{}",
        l.name,
        l.note.as_deref().map(|n| format!(" - {n}")).unwrap_or_default()
    )];
    for stop in &l.stops {
        let name = stop
            .name
            .clone()
            .or_else(|| {
                let id = stop.room()?;
                graph?.room(id).map(|r| r.name.clone())
            })
            .unwrap_or_default();
        out.push(format!("  {}  {name}", stop.at));
    }
    if let Some(finish) = &l.finish {
        out.push(format!("  finish at {finish}"));
    }
    if let Some(graph) = graph {
        out.extend(l.check_names(graph).into_iter().map(|w| format!("  !! {w}")));
    }
    out
}

/// `/loop import`: read a MegaMud path and save what could be walked.
///
/// The report is printed whether or not the loop is saved, because how
/// far a foreign path got is the whole answer. Saved anyway when it got
/// somewhere: a route that stops short is still a route, and the operator
/// can see exactly where it stops.
fn import_loop(graph: &crate::graph::RoomGraph, file: &std::path::Path) -> Vec<String> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return vec![format!("loop: {}: {e}", file.display())],
    };
    let mp = match crate::mega::parse_mp(&text) {
        Ok(mp) => mp,
        Err(e) => return vec![format!("loop: {}: {e}", file.display())],
    };
    // Rebuilt per import rather than kept: 26k hashes take a moment and
    // importing is a thing somebody does once.
    let index = crate::mega::Index::build(graph);
    let (l, report) = match crate::mega::import(graph, &index, &mp, None) {
        Ok(pair) => pair,
        Err(e) => return vec![format!("loop: {e}")],
    };
    let mut out = report.lines();
    if l.stops.len() < 2 {
        out.push("  nothing walkable here; not saved".into());
        return out;
    }
    match l.save(&crate::loops::dir()) {
        Ok(path) => out.push(format!(
            "  saved {} stops as {:?} in {} (/farm {} to walk it)",
            l.stops.len(),
            l.name,
            path.display(),
            l.name
        )),
        Err(e) => out.push(format!("  not saved: {e}")),
    }
    out
}

/// What `play` sends once per realm entry.
///
/// `exp` feeds the status bar off the ordinary event stream (its answer
/// is read back in the same `select!` arm that calls this). `i` is
/// different: NOBODY HERE reads its reply. `Session::send` registers
/// any `i` into the session's own purse tracker regardless of who sent
/// it (see `session.rs`'s `PurseTracker`), so this send exists purely to
/// pre-warm `session.capabilities()`'s purse before the first toll route
/// is ever computed. Without it the purse sits at `Purse::ZERO` until
/// the operator happens to type `i` by hand or a toll gets crossed once
/// — every toll then reads unaffordable, the router always detours, and
/// because it never attempts the toll route it never learns otherwise.
/// Safe-direction, but a feature that quietly does nothing. If you are
/// looking at this `i` wondering who reads it: the session does. Do not
/// delete it again for that reason.
///
/// Split out from its one call site (in [`play`]) so this priming is a
/// seam a test can reach: `play` owns a real terminal in raw mode and a
/// background OS thread reading `crossterm::event::read()`, and cannot
/// be driven end to end.
///
/// Also spawns the inventory/spellbook probe
/// ([`crate::farm::probe_sheet`]) in the background — it is a real wire
/// conversation, up to roughly 13 seconds of round trips including the
/// mystic redirect, and cannot run inline here the way `exp`/`i` do:
/// `on_realm_entry` executes inside `play`'s hot `select!` loop, and
/// awaiting it there would freeze passthrough rendering, key handling
/// and the status bar for the whole probe. Nothing here waits on it —
/// every reader of `Session::raw_sheet` (via `crate::farm::sheet_from`)
/// already treats "not read yet" (an empty book) as a normal answer, the
/// same way an unread `Session::stats` is `Stats::default()` rather than
/// an error. Takes `&Arc<Session>` rather than `&Session` because the
/// spawned task needs an owned handle that outlives this function
/// returning.
///
/// `content`, when the caller has one loaded (see [`load_world`]), is
/// handed straight to [`crate::farm::probe_sheet`] so it can skip or
/// retarget the spellbook probe on a confidently-known class; `None`
/// (no world database, or the caller never held one) leaves probing
/// exactly as it always was.
pub fn on_realm_entry(session: &Arc<Session>, content: Option<Arc<mud_core::content::Content>>) {
    // The pack resolves off the `i` sent just below, so the table has
    // to be in place first.
    if let Some(content) = &content {
        session.set_content(Arc::clone(content));
    }
    session.send("exp");
    session.send("i");
    let probe_session = Arc::clone(session);
    tokio::spawn(async move {
        crate::farm::probe_sheet(&probe_session, content.as_deref()).await;
    });
}

/// The world files one content path loads: graph, spawn table and the
/// content decoder.
pub type World = (
    Arc<crate::graph::RoomGraph>,
    Arc<crate::spawn::SpawnTable>,
    Arc<mud_core::content::Content>,
);

/// World files by content path, loaded once per path for the life of
/// the program. A second connection on the same database reuses the
/// first load. A path that fails to load is not remembered, so a file
/// that appears later is found.
#[derive(Default)]
pub struct ContentCache {
    worlds: std::collections::HashMap<std::path::PathBuf, World>,
}

impl ContentCache {
    pub fn world(&mut self, db: &std::path::Path) -> Option<World> {
        if let Some(world) = self.worlds.get(db) {
            return Some(world.clone());
        }
        let world = load_world(db)?;
        self.worlds.insert(db.to_path_buf(), world.clone());
        Some(world)
    }
}

/// Loads the world files for one content path: the graph, the spawn
/// table and the content decoder. Takes a path, not a `Session`, on
/// purpose. It runs before any session is guaranteed to exist and its
/// job is "is there a world database", nothing about who is playing or
/// what they can afford. It builds no `Navigator` at all: see
/// [`finish_locator`], the only place one gets built, so there is no
/// unwired one for a second caller to reach for by mistake.
fn load_world(db: &std::path::Path) -> Option<World> {
    // The hand-played session keeps its own room model, and it needs the
    // death wordings as much as the farm does — more, on a shared board.
    let _ = crate::deaths::init(db);
    let graph = Arc::new(crate::graph::RoomGraph::load(db).ok()?);
    // Same file as the graph, so this fails only when that one would
    // have: both are one answer to "is there a world database".
    let spawns = Arc::new(crate::spawn::SpawnTable::load(db).ok()?);
    // The one decoder, held for the life of the session (spec
    // `2026-08-22-one-path-to-content-design.md`). Not yet consumed —
    // `graph` and `spawns` above still read the database on their own —
    // the views that replace those reads are a later task in the same
    // plan. Failure here is folded into the same "no world database"
    // answer as the other two.
    let content = Arc::new(mud_core::content_db::load(db).ok()?);
    Some((graph, spawns, content))
}

/// Finish what [`load_world`] began: build the interactive play loop's own
/// `Navigator`, with the session's real capabilities applied AT
/// CONSTRUCTION — never a separate step a second caller could skip.
///
/// `load_world` used to hand back a ready-made `Navigator` of its own,
/// which stayed representable on `Capabilities::unrestricted()` even
/// after this function existed to fix one up: nothing in the type
/// system stopped a future caller from taking `load_world`'s navigator
/// directly and walking with it unwired. Moving construction here
/// removes the unwired value itself rather than merely leaving it
/// unreached.
///
/// Split out from its one call site (in [`play`]) rather than folded in
/// inline, so the wiring itself is a testable seam: `play` owns a real
/// terminal in raw mode and a background OS thread reading
/// `crossterm::event::read()`, and cannot be driven end to end the way
/// `go::run_go` can. This function is the whole of what that call site
/// does with a `Session` in hand, so a test exercising it directly is
/// exercising the real wiring, not a stand-in for it.
///
/// The navigator this produces is the one the interactive play loop
/// walks with by hand (`mapview::run`'s route preview, in particular)
/// — it is the MOST-used path, not a side one, which is why it gets the
/// same treatment as every other `Navigator::new` site.
pub fn finish_locator(
    found: Option<World>,
    session: &Session,
) -> (
    Option<Arc<crate::graph::RoomGraph>>,
    Option<crate::nav::Navigator>,
    Option<Arc<crate::spawn::SpawnTable>>,
    Option<Arc<mud_core::content::Content>>,
) {
    match found {
        Some((g, s, c)) => {
            let nav = crate::nav::Navigator::new(g.clone(), crate::nav::NavConfig::default())
                .with_capabilities(session.capabilities());
            (Some(g), Some(nav), Some(s), Some(c))
        }
        None => (None, None, None, None),
    }
}

