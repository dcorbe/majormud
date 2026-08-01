//! Interactive terminal client: raw ANSI passthrough into a DECSTBM
//! scroll region, with a status bar and a local-editing input line on
//! the two reserved bottom rows.

use std::io::Write as _;
use std::sync::Arc;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::session::{GameState, Session};

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
    let mut farm: Option<FarmSession> = None;
    // A clone of the runner's phase channel, kept separate so the select
    // can await it without borrowing `farm` (which the repaint needs).
    let mut phase_rx: Option<tokio::sync::watch::Receiver<crate::farm::Phase>> = None;
    // Where the client believes the character is, tracked whoever is
    // driving. A room block is a room block: it says as much when the
    // operator typed the move as when the runner did.
    let mut here: Option<mud_core::content::RoomId> = None;
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
    let started = std::time::Instant::now();
    let nav = locator(session.profile());

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
    repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;

    let result = loop {
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    // Into the scroll region: restore server cursor,
                    // write verbatim, save it again.
                    out.write_all(b"\x1b8")?;
                    out.write_all(&bytes)?;
                    out.write_all(b"\x1b7")?;
                    repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break Ok(()), // disconnected
            },
            ev = events.recv() => {
                // The display comes from raw passthrough, so nothing is
                // rendered here — only the counters and the assist.
                if let Ok(cor) = &ev {
                    if let crate::events::Event::Line(line) = &cor.event {
                        let before = exp.total();
                        exp.observe(line);
                        if exp.total() != before {
                            repaint(&mut out, &state_rx, target, farm.as_ref(), here,
                                    exp.per_minute(started.elapsed()), &editor, cols, rows)?;
                        }
                    }
                    // While a farm runs it owns the connection outright;
                    // the assist only drives a hand-played session. Room
                    // blocks are believed under the runner's own rule
                    // (farm.rs `bot_sees`): only a block that answers a
                    // command — the operator's look or step included —
                    // reaches the bot, so a stale or foreign render
                    // cannot clear a latch or start a swing.
                    if farm.is_none()
                        && let Some(bot) = assist.as_mut()
                    {
                        let sees = !matches!(cor.event, crate::events::Event::RoomSeen(_))
                            || cor.answers.is_some();
                        if sees {
                            for crate::bot::BotAction::Send(cmd) in bot.on_event(&cor.event) {
                                session.send(&cmd);
                            }
                        }
                    }
                }
            }
            changed = state_rx.changed() => {
                if changed.is_err() { break Ok(()); }
                if let (Some(nav), Some(room)) = (nav.as_ref(), state_rx.borrow().room.clone()) {
                    here = track(nav, here, &room).or(here);
                }
                repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;
            }
            // The bar must follow the runner, not just HP: travelling and
            // fighting can pass without a single point of damage.
            _ = async {
                match phase_rx.as_mut() {
                    Some(rx) => {
                        let _ = rx.changed().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;
            }
            ev = key_rx.recv() => {
                let Some(ev) = ev else { break Ok(()) };
                match ev {
                    TermEvent::Resize(w, h) => {
                        cols = w;
                        rows = h;
                        setup_region(&mut out, rows)?;
                        repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;
                    }
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        let was = passthrough;
                        let outcome =
                            handle_key(&key, &mut editor, &session, &mut passthrough, farm.is_some());
                        match outcome {
                            KeyOutcome::Quit => break Ok(()),
                            KeyOutcome::StartFarm => {
                                // Reachable mid-run now that the editor
                                // works while farming: one runner only.
                                if farm.is_some() {
                                    note(&mut out, "-- farm already running (Ctrl-F to take over) --")?;
                                } else {
                                    match start_farm(session.clone()) {
                                        Ok(started) => {
                                            note(&mut out, "-- farm running (Ctrl-F to take over) --")?;
                                            phase_rx = Some(started.phase.clone());
                                            farm = Some(started);
                                        }
                                        Err(e) => note(&mut out, &format!("-- {e} --"))?,
                                    }
                                }
                            }
                            KeyOutcome::StopFarm => {
                                if let Some(f) = farm.take() {
                                    f.handle.abort();
                                    phase_rx = None;
                                    // The keyboard is a person again:
                                    // flood control back off.
                                    session.set_pace(std::time::Duration::ZERO);
                                    note(&mut out, "-- farm stopped; you have the keyboard --")?;
                                }
                            }
                            KeyOutcome::ToggleAssist => {
                                if assist.take().is_none() {
                                    // Fresh on every start: latches from
                                    // an earlier stretch describe fights
                                    // that are over.
                                    assist = Some(crate::bot::Bot::new(assist_config.clone()));
                                    note(&mut out, "-- bot assist on: fighting and looting beside you (/bot to stop) --")?;
                                } else {
                                    note(&mut out, "-- bot assist off --")?;
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
                        repaint(&mut out, &state_rx, target, farm.as_ref(), here, exp.per_minute(started.elapsed()), &editor, cols, rows)?;
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
    StartFarm,
    StopFarm,
    ToggleAssist,
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
    if farming {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('f') => KeyOutcome::StopFarm,
                KeyCode::Char('q') => KeyOutcome::Quit,
                _ => KeyOutcome::Continue,
            };
        }
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
            match line.trim() {
                "/quit" => return KeyOutcome::Quit,
                "/farm" => return KeyOutcome::StartFarm,
                "/bot" => return KeyOutcome::ToggleAssist,
                _ => {
                    session.send(&line);
                }
            }
        }
        _ => {}
    }
    KeyOutcome::Continue
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
    room_id: Option<mud_core::content::RoomId>,
    exp_per_min: Option<i64>,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let status = render_status(state, target, phase, room_id, exp_per_min, cols as usize);
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
pub fn render_status(
    state: &GameState,
    target: &str,
    phase: Option<&crate::farm::Phase>,
    room_id: Option<mud_core::content::RoomId>,
    exp_per_min: Option<i64>,
    width: usize,
) -> String {
    let mut s = String::new();
    if let Some(phase) = phase {
        s.push_str(&format!("{} | ", phase.label()));
    }
    s.push_str(&format!("HP {}", state.hp));
    if let Some(ma) = state.mana {
        s.push_str(&format!(" MA {ma}"));
    }
    if let Some(room) = &state.room {
        s.push_str(&format!(" | {}", room.name));
        if let Some(id) = room_id {
            s.push_str(&format!(" [{}/{}]", id.map, id.room));
        }
    }
    if let Some(rate) = exp_per_min {
        s.push_str(&format!(" | {rate} xp/min"));
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

/// A farm run driving this session from inside `play`.
pub struct FarmSession {
    handle: tokio::task::JoinHandle<()>,
    phase: tokio::sync::watch::Receiver<crate::farm::Phase>,
}

/// Print a one-off notice into the scrolling region without disturbing
/// the board's cursor.
fn note(out: &mut impl std::io::Write, text: &str) -> std::io::Result<()> {
    out.write_all(b"\x1b8")?;
    out.write_all(format!("\r\n{text}\r\n").as_bytes())?;
    out.write_all(b"\x1b7")?;
    Ok(())
}

/// Redraw the bottom rows, reading the farm's phase when one is running.
#[allow(clippy::too_many_arguments)]
fn repaint(
    out: &mut impl std::io::Write,
    state_rx: &tokio::sync::watch::Receiver<GameState>,
    target: &str,
    farm: Option<&FarmSession>,
    here: Option<mud_core::content::RoomId>,
    exp_per_min: Option<i64>,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let phase = farm.map(|f| f.phase.borrow().clone());
    // The runner's own belief wins while it drives -- it knows which of
    // two same-named rooms it walked to -- and the client's tracking
    // covers everything else.
    let room_id = phase.as_ref().and_then(|p| p.room()).or(here);
    let state = state_rx.borrow().clone();
    redraw_bottom(
        out,
        &state,
        target,
        phase.as_ref(),
        room_id,
        exp_per_min,
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
fn start_farm(session: Arc<Session>) -> Result<FarmSession, String> {
    let profile = session.profile().clone();
    let cfg = profile
        .farm
        .clone()
        .ok_or("no [farm] table in the profile: nothing to patrol")?;
    let graph = Arc::new(crate::graph::RoomGraph::load(&cfg.content)?);
    let plan = crate::farm::FarmPlan::build(&cfg, &graph)?;
    let bot = profile.bot.clone().unwrap_or_default();
    // Automation goes back under flood control. `play` unpaced this
    // session for the operator's keystrokes; the runner it is about to
    // hand the connection to cycled at loopback echo speed without this
    // (~40 look+attack commands in 400ms, run4 2026-08-01).
    session.set_pace(profile.pace());
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let end = match crate::farm::run_farm(&session, graph, &plan, &bot, &cfg, Some(&tx)).await {
            Ok(_) => crate::farm::Phase::Done,
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
        };
        let _ = tx.send(end);
    });
    Ok(FarmSession { handle, phase: rx })
}

/// A navigator used only to work out where the character is.
///
/// Best effort: the room database is how a name becomes a number, and
/// without it the bar simply shows the name, as it always did. The path
/// comes from `[farm].content` when the profile has one, else the same
/// default `mmc path` uses.
fn locator(profile: &crate::profile::Profile) -> Option<crate::nav::Navigator> {
    let db = profile
        .farm
        .as_ref()
        .map(|f| f.content.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("re/mmud_wgnt.sqlite"));
    let graph = crate::graph::RoomGraph::load(&db).ok()?;
    Some(crate::nav::Navigator::new(
        Arc::new(graph),
        crate::nav::NavConfig::default(),
    ))
}

/// Resolve a room block to a room id, given where we thought we were.
///
/// `localize_view` tries the cheap one-hop answer first and falls back to
/// searching the whole graph by name and exits, so this works both for an
/// ordinary step and for the first block after logging in, when there is
/// no previous position at all.
fn track(
    nav: &crate::nav::Navigator,
    here: Option<mud_core::content::RoomId>,
    room: &crate::events::RoomView,
) -> Option<mud_core::content::RoomId> {
    // A room that cannot exist, so the neighbour shortcut misses and the
    // global search runs. Any seed would do; this one cannot be right by
    // accident.
    let hint = here.unwrap_or(mud_core::content::RoomId { map: 0, room: 0 });
    nav.localize_view(hint, room)
}
