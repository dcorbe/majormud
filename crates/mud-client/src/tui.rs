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

/// Run the interactive client until the user quits (Ctrl-Q or /quit).
///
/// Layout: rows 1..h-2 are a DECSTBM scroll region receiving the raw
/// server stream verbatim; row h-1 is the status bar; row h is the
/// input line. The server-side cursor position is kept with DECSC/DECRC
/// around every passthrough write.
pub async fn play(session: Arc<Session>) -> std::io::Result<()> {
    let mut raw_rx = session.raw();
    let mut events = session.events();
    let mut state_rx = session.state();
    let target = match session.profile().target {
        crate::dialect::Target::MbbsEmu => "mbbs",
        crate::dialect::Target::RustServer => "rust",
    };

    crossterm::terminal::enable_raw_mode()?;
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
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
    // The assist: a bot that fights and loots BESIDE the operator while
    // no farm runs. Never heals or flees — movement and rest belong to
    // the person holding the keyboard. `/bot` toggles it; the profile's
    // `assist_play` starts it on. Rebuilt on every toggle-on so its
    // latches start clean.
    let mut assist: Option<crate::bot::Bot> = None;
    let assist_config = {
        let mut cfg = session.profile().bot.clone().unwrap_or(crate::bot::BotConfig {
            // A profile without a [bot] table still gets a useful
            // assist: attack and loot are the whole point of asking.
            auto_combat: true,
            auto_get: true,
            ..Default::default()
        });
        cfg.auto_heal = false;
        cfg.auto_flee = false;
        cfg
    };
    if assist_config.assist_play {
        assist = Some(crate::bot::Bot::new(assist_config.clone()));
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
    // `content` is held for the session's lifetime alongside `graph` and
    // `spawns`; `on_realm_entry` is its one reader, for the spellbook
    // probe's class/magictype skip (Task 5 of `one-path-to-content`).
    let (graph, nav, spawns, content) = finish_locator(locator(session.profile()), &session);

    // Key events come from a blocking reader thread.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut out = std::io::stdout();
    setup_region(&mut out, rows)?;
    repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;

    let result = loop {
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    // Into the scroll region: restore server cursor,
                    // write verbatim, save it again.
                    out.write_all(b"\x1b8")?;
                    out.write_all(&bytes)?;
                    out.write_all(b"\x1b7")?;
                    repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break Ok(()), // disconnected
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
                                    exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
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
                        && let Some(bot) = assist.as_mut()
                    {
                        for cmd in assist_actions(bot, cor) {
                            session.send(&cmd);
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
                if changed.is_err() { break Ok(()); }
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
                repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
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
                        assist = Some(crate::bot::Bot::new(assist_config.clone()));
                    }
                    for cmd in handover_actions(&ended, assist.is_some()) {
                        session.send(&cmd);
                    }
                    note(&mut out, &format!("-- {what} ended: {} --", ended.label()))?;
                }
                repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
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
                let Some(ev) = ev else { break Ok(()) };
                match ev {
                    TermEvent::Resize(w, h) => {
                        cols = w;
                        rows = h;
                        setup_region(&mut out, rows)?;
                        repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                    }
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        let was = passthrough;
                        let outcome =
                            handle_key(&key, &mut editor, &session, &mut passthrough, job.is_some());
                        match outcome {
                            KeyOutcome::Quit => break Ok(()),
                            KeyOutcome::Loops { name } => {
                                note(&mut out, &describe_loops(graph.as_deref(), name.as_deref()).join("\n"))?;
                            }
                            KeyOutcome::ImportLoop { file } => {
                                note(&mut out, &match graph.as_ref() {
                                    None => vec![format!(
                                        "-- loop: no room database at {} --",
                                        content_path(session.profile()).display()
                                    )],
                                    Some(g) => import_loop(g, std::path::Path::new(&file)),
                                }.join("\n"))?;
                            }
                            KeyOutcome::StartFarm { loop_name } => {
                                // Reachable mid-run now that the editor
                                // works while farming: one job only.
                                if let Some(j) = job.as_ref() {
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
                                            content_path(session.profile()).display()
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
                                                let started = start_go(
                                                    session.clone(),
                                                    g.clone(),
                                                    here.confirmed(),
                                                    to,
                                                    assist_config.clone(),
                                                    assist.is_some(),
                                                );
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
                                        },
                                    }
                                }
                            }
                            KeyOutcome::Where => {
                                match graph.as_ref() {
                                    None => note(&mut out, &format!(
                                        "-- where: no room database at {} --",
                                        content_path(session.profile()).display()
                                    ))?,
                                    Some(g) => {
                                        let started = start_where(session.clone(), g.clone(), here.confirmed());
                                        note(&mut out, "-- working out where you are (Ctrl-F to take over) --")?;
                                        phase_rx = Some(started.phase.clone());
                                        job = Some(started);
                                    }
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
                                        content_path(session.profile()).display()
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
                                                let mut on_event = |cor: &crate::correlate::Correlated| {
                                                    if let crate::events::Event::Line(line) = &cor.event {
                                                        exp.observe(line);
                                                    }
                                                    if job.is_none()
                                                        && let Some(bot) = assist.as_mut()
                                                    {
                                                        for cmd in assist_actions(bot, cor) {
                                                            session.send(&cmd);
                                                        }
                                                    }
                                                };
                                                crate::mapview::run(
                                                    &session,
                                                    &mut view,
                                                    &mut key_rx,
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
                                                crate::mapview::ViewAction::Quit => break Ok(()),
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
                                                    let started = start_go(
                                                        session.clone(),
                                                        g.clone(),
                                                        here.confirmed(),
                                                        to,
                                                        assist_config.clone(),
                                                        assist.is_some(),
                                                    );
                                                    note(&mut out, &format!(
                                                        "-- walking to {name} [{}/{}] (Ctrl-F to take over) --",
                                                        to.map, to.room
                                                    ))?;
                                                    phase_rx = Some(started.phase.clone());
                                                    job = Some(started);
                                                }
                                                _ => {}
                                            }
                                        }
                                    },
                                    _ => note(&mut out, &format!(
                                        "-- map: no room database at {} --",
                                        content_path(session.profile()).display()
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
                                let on = assist.take().is_none();
                                if on {
                                    // Fresh on every start: latches from
                                    // an earlier stretch describe fights
                                    // that are over.
                                    assist = Some(crate::bot::Bot::new(assist_config.clone()));
                                }
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
                        repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
                    }
                    _ => {}
                }
            }
        }
    };

    // Reset scroll region and leave the terminal usable.
    let _ = out.write_all(b"\x1b[r");
    let _ = out.write_all(format!("\x1b[{rows};1H\r\n").as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
    result
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

/// What a keystroke asked the client to do. Returned rather than acted
/// on, because starting a farm needs to spawn a task and own its handle —
/// which is `play`'s business, not the key handler's.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Continue,
    Quit,
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
        "/where" => Some(KeyOutcome::Where),
        "/room" => Some(KeyOutcome::Room {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/map" => Some(KeyOutcome::Map {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/help" | "/?" => Some(KeyOutcome::Help),
        _ => None,
    }
}

/// Text for `/help`. A function rather than a `const` so it reads next
/// to `slash`, the thing it has to stay in sync with.
fn help_text() -> &'static str {
    "/quit                disconnect and exit
/farm [loop]         patrol the profile's circuit, or a named loop from the library
/loop [name]         list the loop library, or show one loop's stops
/loop import <file>  read a MegaMud .mp path into the library
/bot                 toggle the fight/loot assist (walk vs. run for /go)
/go <room>           walk to a room, by id (1/2324) or name
/where               work out which room you're standing in
/room [target]       what the world database knows about a room (default: here)
/map [target]        draw the plane around a room (default: here)
/help, /?            this list

Ctrl-F  take the keyboard back from a running farm/go/where/roam
Ctrl-P  toggle passthrough, for full-screen board screens (train stats)
Ctrl-Q  quit"
}

/// One keystroke against the session. Public for the keyboard-contract
/// tests: what farming swallows, what the editor keeps, what goes out.
pub fn handle_key(
    key: &KeyEvent,
    editor: &mut InputEditor,
    session: &Session,
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
            KeyCode::Char('q') => KeyOutcome::Quit,
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
            return KeyOutcome::Quit;
        }
        if let Some(bytes) = key_bytes(key) {
            session.send_raw(&bytes);
        }
        return KeyOutcome::Continue;
    }
    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyOutcome::Quit;
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => editor.insert(c),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Up => editor.history_prev(),
        KeyCode::Down => editor.history_next(),
        KeyCode::Enter => {
            let line = editor.take_line();
            match slash(&line) {
                Some(outcome) => return outcome,
                None => {
                    session.send(&line);
                }
            }
        }
        _ => {}
    }
    KeyOutcome::Continue
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
    target: &str,
    phase: Option<&crate::farm::Phase>,
    room_id: crate::lost::Fix,
    exp_per_min: Option<i64>,
    level: Option<crate::progress::LevelProgress>,
    assist: bool,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let status = render_status(state, target, phase, room_id, exp_per_min, level, assist, cols as usize);
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

/// One status line, exactly `width` characters (padded/truncated).
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
    target: &str,
    phase: Option<&crate::farm::Phase>,
    room_id: crate::lost::Fix,
    exp_per_min: Option<i64>,
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
    if let Some(rate) = exp_per_min {
        s.push_str(&format!(" | {rate} xp/min"));
    }
    // How long until the next level, at the rate we are actually
    // earning. Refreshed on its own timer, so it goes stale between
    // ticks rather than jittering with every kill.
    if let Some(p) = level {
        s.push_str(&format!(
            " | L{}->{} {}",
            p.level,
            p.level + 1,
            crate::progress::eta_label(p.needed, exp_per_min)
        ));
    }
    s.push_str(&format!(" | {target}"));
    // Defence in depth: nothing painted into a fixed row may contain a
    // control character, whatever built the string. A newline here
    // scrolls the terminal and the whole display appears to flash.
    let mut out: Vec<char> = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    out.truncate(width);
    while out.len() < width {
        out.push(' ');
    }
    out.into_iter().collect()
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

/// Redraw the bottom rows, reading the farm's phase when one is running.
#[allow(clippy::too_many_arguments)]
fn repaint(
    out: &mut impl std::io::Write,
    state_rx: &tokio::sync::watch::Receiver<GameState>,
    target: &str,
    job: Option<&Job>,
    here: crate::lost::Fix,
    exp_per_min: Option<i64>,
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
        target,
        phase.as_ref(),
        room_id,
        exp_per_min,
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
fn start_farm(session: Arc<Session>, loop_name: Option<&str>) -> Result<Job, String> {
    let profile = session.profile().clone();
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
fn start_roam(
    session: Arc<Session>,
    walls: crate::roam::Walls,
    here: crate::lost::Fix,
) -> Result<Job, String> {
    let profile = session.profile().clone();
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
                    "{} ({} kills, {}{})",
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
fn start_go(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
    to: mud_core::content::RoomId,
    bot: crate::bot::BotConfig,
    walking: bool,
) -> Job {
    let profile = session.profile().clone();
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
    Job {
        handle,
        phase: rx,
        what: "go",
    }
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
fn start_where(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
) -> Job {
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
    Job {
        handle,
        phase: rx,
        what: "where",
    }
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
/// `content`, when the caller has one loaded (see [`locator`]), is
/// handed straight to [`crate::farm::probe_sheet`] so it can skip or
/// retarget the spellbook probe on a confidently-known class; `None`
/// (no world database, or the caller never held one) leaves probing
/// exactly as it always was.
pub fn on_realm_entry(session: &Arc<Session>, content: Option<Arc<mud_core::content::Content>>) {
    session.send("exp");
    session.send("i");
    let probe_session = Arc::clone(session);
    tokio::spawn(async move {
        crate::farm::probe_sheet(&probe_session, content.as_deref()).await;
    });
}

/// Loads the world files: the graph and the spawn table. Takes a
/// `Profile`, not a `Session` — on purpose. It runs before any session
/// is guaranteed to exist and its job is "is there a world database",
/// nothing about who is playing or what they can afford. It builds no
/// `Navigator` at all: see [`finish_locator`], the only place one gets
/// built, so there is no unwired one for a second caller to reach for
/// by mistake.
fn locator(
    profile: &crate::profile::Profile,
) -> Option<(
    Arc<crate::graph::RoomGraph>,
    Arc<crate::spawn::SpawnTable>,
    Arc<mud_core::content::Content>,
)> {
    let db = content_path(profile);
    // The hand-played session keeps its own room model, and it needs the
    // death wordings as much as the farm does — more, on a shared board.
    let _ = crate::deaths::init(&db);
    let graph = Arc::new(crate::graph::RoomGraph::load(&db).ok()?);
    // Same file as the graph, so this fails only when that one would
    // have: both are one answer to "is there a world database".
    let spawns = Arc::new(crate::spawn::SpawnTable::load(&db).ok()?);
    // The one decoder, held for the life of the session (spec
    // `2026-08-22-one-path-to-content-design.md`). Not yet consumed —
    // `graph` and `spawns` above still read the database on their own —
    // the views that replace those reads are a later task in the same
    // plan. Failure here is folded into the same "no world database"
    // answer as the other two.
    let content = Arc::new(mud_core::content_db::load(&db).ok()?);
    Some((graph, spawns, content))
}

/// Finish what [`locator`] began: build the interactive play loop's own
/// `Navigator`, with the session's real capabilities applied AT
/// CONSTRUCTION — never a separate step a second caller could skip.
///
/// `locator` used to hand back a ready-made `Navigator` of its own,
/// which stayed representable on `Capabilities::unrestricted()` even
/// after this function existed to fix one up: nothing in the type
/// system stopped a future caller from taking `locator`'s navigator
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
    found: Option<(
        Arc<crate::graph::RoomGraph>,
        Arc<crate::spawn::SpawnTable>,
        Arc<mud_core::content::Content>,
    )>,
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
