//! Interactive terminal client: raw ANSI passthrough into a DECSTBM
//! scroll region, with a status bar and a local-editing input line on
//! the two reserved bottom rows.

use std::io::Write;
use std::sync::Arc;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::session::{GameState, Session};

/// How often the client asks the board where it stands against the next
/// level. Slow on purpose: the answer changes by one kill at a time, and
/// the command costs a round-trip on a connection a farm may be driving.
pub(crate) const LEVEL_POLL: std::time::Duration = std::time::Duration::from_secs(60);

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
        KeyOutcome::NewWindow { .. } | KeyOutcome::CloseWindow | KeyOutcome::Windows | KeyOutcome::Switch(_) => {
            // The front end handles these and never sends them here.
            (LobbyStep::Stay, None)
        }
        KeyOutcome::SetList { .. }
        | KeyOutcome::Set { .. }
        | KeyOutcome::Unset { .. }
        | KeyOutcome::Save { .. }
        | KeyOutcome::Load { .. } => (LobbyStep::Stay, None),
    }
}

/// One line in the lobby's log. The clock is UTC, hours minutes seconds,
/// because the client has no timezone table and a wrong local time is
/// worse than an honest UTC one.
pub fn log_line(
    now: std::time::SystemTime,
    number: usize,
    info: Option<&crate::window::WindowInfo>,
    kind: &crate::window::EventKind,
) -> String {
    use crate::window::EventKind;
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    let stamp = format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60);
    let who = match info {
        Some(i) if !i.username.is_empty() => format!("{}@{}:{}", i.username, i.host, i.port),
        Some(i) => format!("{}:{}", i.host, i.port),
        None => "lobby".to_string(),
    };
    let what = match kind {
        EventKind::Connected => "connected".to_string(),
        EventKind::Disconnected => "disconnected".to_string(),
        EventKind::Died => "died".to_string(),
        EventKind::JobEnded(label) => label.clone(),
        EventKind::AssistRefused(why) => format!("assist not started: {why}"),
        EventKind::Reconnecting { attempt } => format!("reconnecting, attempt {attempt}"),
    };
    format!("{stamp} window {number} {who} {what}")
}

/// Run the client until the operator quits. One front end owns the
/// terminal, the editor and the windows. Window 1 is the lobby, held
/// here rather than as a task: its settings are the template `/new`
/// copies and its screen is the log of every window's major events.
pub async fn run(
    settings: crate::settings::Settings,
    capture: Option<crate::session::Capture>,
) -> std::io::Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    crossterm::terminal::enable_raw_mode()?;
    // The one blocking read, on its own thread. Kept out of `Front` so a
    // test can hand it a channel it fills itself.
    let (key_tx, key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout();
    let mut front = Front::new(settings, capture, cols, rows, key_rx);
    let result = front.run(&mut out).await;
    let _ = out.write_all(format!("\x1b[0m\x1b[{};1H\r\n", front.rows).as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
    result
}

/// The screen the painter last drew, keyed by window so a switch forces
/// a full paint.
struct LastFrame {
    window: crate::window::WindowId,
    screen: vt100::Screen,
}

/// The lobby has this id. Windows get ids from 1 up.
const LOBBY: crate::window::WindowId = 0;

/// How many window messages one loop turn takes before it repaints. A
/// burst of board output is many small messages and painting each one
/// costs more than the burst does, but an unbounded drain would let a
/// loud board starve the keyboard.
const DRAIN: usize = 256;

/// The client's one front end. Public so a test can build one on a
/// synthetic key channel and a `Vec<u8>` and step it by hand: the
/// terminal size and the key reader thread are `run`'s business, not
/// this type's.
pub struct Front {
    rows: u16,
    cols: u16,
    editor: InputEditor,
    /// The template every `/new` copies, and what `/set` in the lobby edits.
    lobby: crate::settings::Settings,
    log: crate::screen::Screen,
    /// Windows 2 and up, in number order. Number is index plus two.
    windows: Vec<crate::window::WindowHandle>,
    /// The number of the window on screen, 1 for the lobby.
    active: usize,
    unread: Vec<crate::window::WindowId>,
    /// A window whose map view has the terminal.
    frozen: Option<crate::window::WindowId>,
    last: Option<LastFrame>,
    last_bar: Option<String>,
    next_id: crate::window::WindowId,
    quit_armed: bool,
    /// Something on screen moved since the last paint.
    repaint: bool,
    /// The active window's rows moved since the last paint, as opposed
    /// to the bar or the input line. Snapshotting copies the whole
    /// scrollback, so a frame that only reprints the bar skips it.
    screen_moved: bool,
    front_tx: tokio::sync::mpsc::UnboundedSender<crate::window::FrontMsg>,
    front_rx: tokio::sync::mpsc::UnboundedReceiver<crate::window::FrontMsg>,
    key_rx: tokio::sync::mpsc::UnboundedReceiver<TermEvent>,
    cache: Arc<std::sync::Mutex<ContentCache>>,
}

enum Flow {
    Continue,
    Exit,
}

impl Front {
    /// The terminal size and the keystroke channel come from the caller,
    /// which is `run` in play and a test otherwise.
    pub fn new(
        settings: crate::settings::Settings,
        capture: Option<crate::session::Capture>,
        cols: u16,
        rows: u16,
        key_rx: tokio::sync::mpsc::UnboundedReceiver<TermEvent>,
    ) -> Front {
        let (front_tx, front_rx) = tokio::sync::mpsc::unbounded_channel();
        let scrollback = settings.profile().scrollback_lines as usize;
        let mut log = crate::screen::Screen::new(rows.saturating_sub(2), cols, scrollback);
        for warning in settings.warnings() {
            log.note(&format!("-- {warning} --"));
        }
        let mut front = Front {
            rows,
            cols,
            editor: InputEditor::new(),
            lobby: settings,
            log,
            windows: Vec::new(),
            active: 1,
            unread: Vec::new(),
            frozen: None,
            last: None,
            last_bar: None,
            next_id: 1,
            quit_armed: false,
            repaint: false,
            screen_moved: true,
            front_tx,
            front_rx,
            key_rx,
            cache: Arc::new(std::sync::Mutex::new(ContentCache::default())),
        };
        // Started with a profile that names a host: window 2, connected,
        // and the capture goes with it.
        if !front.lobby.profile().host.is_empty() {
            let number = front.open(front.lobby.clone(), capture, None);
            front.switch(number);
        }
        front
    }

    /// The number of the window on screen, 1 for the lobby.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Whether the lobby's template holds unsaved edits. This is what
    /// `/quit` asks about for window 1.
    pub fn lobby_dirty(&self) -> bool {
        self.lobby.dirty()
    }

    pub async fn run(&mut self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        self.paint(out)?;
        while self.step(out).await? {}
        Ok(())
    }

    /// One event and one paint. `false` means the front end is done.
    pub async fn step(&mut self, out: &mut impl std::io::Write) -> std::io::Result<bool> {
        tokio::select! {
            ev = self.key_rx.recv() => {
                let Some(ev) = ev else { return Ok(false) };
                match ev {
                    TermEvent::Resize(w, h) => {
                        // A map view draws the real terminal itself, so
                        // it needs the new size as much as the front end
                        // does. `on_key` never sees a resize, so this is
                        // the only place it can be handed over.
                        if let Some(id) = self.frozen
                            && let Some(win) = self.windows.iter().find(|win| win.id == id)
                        {
                            let _ = win.keys.send(ev);
                        }
                        self.resize(w, h);
                    }
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        // The editor's line may have moved under any key,
                        // so a key always earns its repaint. A key can
                        // also note into the screen or the log, by many
                        // paths, so it earns a fresh snapshot too.
                        self.repaint = true;
                        self.screen_moved = true;
                        if let Flow::Exit = self.on_key(key, TermEvent::Key(key)) {
                            return Ok(false);
                        }
                    }
                    _ => {}
                }
            }
            msg = self.front_rx.recv() => {
                if let Some(msg) = msg {
                    self.on_front(msg);
                }
            }
        }
        for _ in 0..DRAIN {
            let Ok(msg) = self.front_rx.try_recv() else { break };
            self.on_front(msg);
        }
        if self.repaint {
            self.paint(out)?;
        }
        Ok(true)
    }

    fn open(
        &mut self,
        settings: crate::settings::Settings,
        capture: Option<crate::session::Capture>,
        first: Option<KeyOutcome>,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let handle = crate::window::spawn(
            id,
            settings,
            self.rows,
            self.cols,
            self.cache.clone(),
            self.front_tx.clone(),
            capture,
            first,
        );
        self.windows.push(handle);
        let number = self.windows.len() + 1;
        self.log.note(&format!("-- opened window {number} --"));
        number
    }

    fn switch(&mut self, number: usize) {
        self.active = number;
        if let Some(id) = self.id_of(number) {
            self.unread.retain(|u| *u != id);
        }
        self.last = None;
    }

    fn id_of(&self, number: usize) -> Option<crate::window::WindowId> {
        match number {
            1 => Some(LOBBY),
            n => self.windows.get(n.checked_sub(2)?).map(|w| w.id),
        }
    }

    fn number_of(&self, id: crate::window::WindowId) -> Option<usize> {
        if id == LOBBY {
            return Some(1);
        }
        self.windows.iter().position(|w| w.id == id).map(|i| i + 2)
    }

    fn active_window(&self) -> Option<&crate::window::WindowHandle> {
        self.windows.get(self.active.checked_sub(2)?)
    }

    /// A note into whatever is on screen.
    fn note(&mut self, text: &str) {
        match self.active_window() {
            None => self.log.note(text),
            Some(w) => w.screen.lock().expect("screen lock").note(text),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.log.resize(rows.saturating_sub(2), cols);
        for w in &self.windows {
            let _ = w.msgs.send(crate::window::WindowMsg::Resize { rows, cols });
        }
        self.last = None;
        self.last_bar = None;
        self.repaint = true;
    }

    fn on_key(&mut self, key: KeyEvent, raw: TermEvent) -> Flow {
        if let Some(id) = self.frozen {
            if let Some(w) = self.windows.iter().find(|w| w.id == id) {
                let _ = w.keys.send(raw);
            }
            return Flow::Continue;
        }
        match key.code {
            KeyCode::PageUp => {
                self.scroll(true);
                return Flow::Continue;
            }
            KeyCode::PageDown => {
                self.scroll(false);
                return Flow::Continue;
            }
            _ => self.unscroll(),
        }
        // The lobby has no board to pass keys to, so it is never in
        // passthrough and `handle_key` is told so every time.
        let (mut passthrough, farming) = match self.active_window() {
            None => (false, false),
            Some(w) => {
                let info = w.info.lock().expect("info lock");
                (info.passthrough, info.farming)
            }
        };
        let was = passthrough;
        let outcome = handle_key(&key, &mut self.editor, &mut passthrough, farming);
        if passthrough != was
            && let Some(w) = self.active_window()
        {
            w.info.lock().expect("info lock").passthrough = passthrough;
            let text = if passthrough {
                "-- keys passed through to the board, Ctrl-P to return --"
            } else {
                "-- back to the line editor --"
            };
            w.screen.lock().expect("screen lock").note(text);
        }
        if !matches!(outcome, KeyOutcome::Quit | KeyOutcome::Continue) {
            self.quit_armed = false;
        }
        match outcome {
            KeyOutcome::Continue => Flow::Continue,
            KeyOutcome::QuitNow => Flow::Exit,
            KeyOutcome::Quit => {
                let connected: Vec<usize> = self
                    .windows
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.info.lock().expect("info lock").connected)
                    .map(|(i, _)| i + 2)
                    .collect();
                let mut dirty: Vec<usize> = self
                    .windows
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.info.lock().expect("info lock").dirty)
                    .map(|(i, _)| i + 2)
                    .collect();
                if self.lobby.dirty() {
                    dirty.insert(0, 1);
                }
                match quit_windows_refusal(&connected, &dirty, &mut self.quit_armed) {
                    Some(text) => {
                        self.note(&text);
                        Flow::Continue
                    }
                    None => Flow::Exit,
                }
            }
            KeyOutcome::NewWindow { file } => {
                let settings = match file {
                    // Detached: the copy carries the lobby's settings but
                    // not its file, so a bare `/save` here asks for a
                    // name instead of writing this character over the
                    // one the lobby was loaded from.
                    None => Ok(self.lobby.detached()),
                    Some(f) => crate::settings::Settings::load(std::path::Path::new(&f)),
                };
                match settings {
                    Ok(s) => {
                        let number = self.open(s, None, None);
                        self.switch(number);
                    }
                    Err(e) => self.note(&format!("-- new: {e} --")),
                }
                Flow::Continue
            }
            KeyOutcome::CloseWindow => {
                self.close_active();
                Flow::Continue
            }
            KeyOutcome::Windows => {
                let infos: Vec<(usize, Option<crate::window::WindowInfo>)> = std::iter::once((1, None))
                    .chain(self.windows.iter().enumerate().map(|(i, w)| {
                        (i + 2, Some(w.info.lock().expect("info lock").clone()))
                    }))
                    .collect();
                let text = windows_listing(&infos);
                self.note(&text);
                Flow::Continue
            }
            // A `/N` cannot arrive while a map view is up: `on_key`
            // hands every key to the frozen window and returns before a
            // line is ever parsed.
            KeyOutcome::Switch(n) => {
                if self.id_of(n).is_some() {
                    self.switch(n);
                } else {
                    self.note(&format!("-- no window {n} --"));
                }
                Flow::Continue
            }
            KeyOutcome::Refuse(why) => {
                self.note(&format!("-- {why} --"));
                Flow::Continue
            }
            KeyOutcome::Note(text) => {
                self.note(&text);
                Flow::Continue
            }
            other => {
                if self.active == 1 {
                    // A `/connect host:port` opens a window on the
                    // target and lets that window write the host into
                    // its own settings. Answered by the lobby it would
                    // write them into the template every later `/new`
                    // copies, and every later `/quit` would refuse over
                    // an edit the operator never made. A bare `/connect`
                    // carries no host to write, so it goes the ordinary
                    // way and keeps its refusal when the template names
                    // nowhere to dial.
                    if let KeyOutcome::Connect { target: Some(target) } = other {
                        let first = KeyOutcome::Connect { target: Some(target) };
                        let number = self.open(self.lobby.detached(), None, Some(first));
                        self.switch(number);
                        return Flow::Continue;
                    }
                    // `/quit` never reaches `lobby_step` from here: the
                    // front end owns it and answers for every window at
                    // once, so the arm's own arming flag has nothing to
                    // remember between lines.
                    let mut quit_armed = false;
                    let (step, text) = lobby_step(other, &mut self.lobby, &mut quit_armed);
                    if let Some(text) = text {
                        self.log.note(&text);
                    }
                    if let LobbyStep::Connect = step {
                        let number = self.open(self.lobby.detached(), None, None);
                        self.switch(number);
                    }
                } else if let Some(w) = self.active_window()
                    && w.msgs.send(crate::window::WindowMsg::Outcome(other)).is_err()
                {
                    let id = w.id;
                    self.remove(id);
                }
                Flow::Continue
            }
        }
    }

    fn scroll(&mut self, up: bool) {
        match self.active_window() {
            None => {
                if up {
                    self.log.page_up()
                } else {
                    self.log.page_down()
                }
            }
            Some(w) => {
                let mut s = w.screen.lock().expect("screen lock");
                if up {
                    s.page_up()
                } else {
                    s.page_down()
                }
            }
        }
        self.last = None;
    }

    /// Any key but a page key puts the view back on the live rows.
    fn unscroll(&mut self) {
        let scrolled = match self.active_window() {
            None => {
                let was = self.log.scrolled();
                self.log.to_bottom();
                was
            }
            Some(w) => {
                let mut s = w.screen.lock().expect("screen lock");
                let was = s.scrolled();
                s.to_bottom();
                was
            }
        };
        if scrolled {
            self.last = None;
        }
    }

    fn close_active(&mut self) {
        if self.active == 1 {
            self.note("-- the lobby stays open --");
            return;
        }
        let Some((id, connected)) = self
            .active_window()
            .map(|w| (w.id, w.info.lock().expect("info lock").connected))
        else {
            return;
        };
        if connected {
            self.note("-- still connected: /disconnect first --");
            return;
        }
        self.remove(id);
    }

    /// Drop a window's handle, which ends its task, and renumber.
    fn remove(&mut self, id: crate::window::WindowId) {
        let Some(number) = self.number_of(id) else { return };
        self.windows.remove(number - 2);
        self.unread.retain(|u| *u != id);
        self.log.note(&format!("-- closed window {number} --"));
        self.repaint = true;
        self.screen_moved = true;
        if self.active >= number {
            let to = (self.active - 1).max(1);
            self.switch(to);
        }
    }

    fn on_front(&mut self, msg: crate::window::FrontMsg) {
        use crate::window::FrontMsg;
        match msg {
            // Only the window on screen is worth a frame. Every other
            // window is drawing into a picture nobody is looking at.
            FrontMsg::Changed { window, screen } => {
                if self.id_of(self.active) == Some(window) {
                    self.repaint = true;
                    self.screen_moved |= screen;
                }
            }
            FrontMsg::Event { window, kind } => {
                // A window already removed has no number to log under.
                // Its last events arrive after its handle is gone.
                let Some(number) = self.number_of(window) else { return };
                let info = self
                    .windows
                    .iter()
                    .find(|w| w.id == window)
                    .map(|w| w.info.lock().expect("info lock").clone());
                let line = log_line(std::time::SystemTime::now(), number, info.as_ref(), &kind);
                self.log.note(&line);
                if self.id_of(self.active) != Some(window) && !self.unread.contains(&window) {
                    self.unread.push(window);
                }
                self.repaint = true;
                self.screen_moved = true;
            }
            FrontMsg::Takeover { window, on } => {
                self.frozen = on.then_some(window);
                if !on {
                    self.last = None;
                    self.last_bar = None;
                }
                self.repaint = true;
                self.screen_moved = true;
            }
            FrontMsg::Ended { window } => self.remove(window),
        }
    }

    pub fn paint(&mut self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        self.repaint = false;
        if self.frozen.is_some() {
            return Ok(());
        }
        let id = self.id_of(self.active).unwrap_or(LOBBY);
        let info = self
            .active_window()
            .map(|w| w.info.lock().expect("info lock").clone());
        let unread: Vec<usize> = self.unread.iter().filter_map(|u| self.number_of(*u)).collect();
        let bar = window_bar(self.active, info.as_ref(), &unread, self.cols);
        let had_last = self.last.as_ref().is_some_and(|l| l.window == id);
        // A snapshot deep-copies the scrollback, thousands of rows of it.
        // A bar that ticked moved none of them, so the snapshot already
        // held describes the screen and the diff against it is empty.
        let moved = self.screen_moved || !had_last;
        self.screen_moved = false;
        let mut frame = Vec::new();
        if moved {
            let snapshot = match self.active_window() {
                None => self.log.snapshot(),
                Some(w) => w.screen.lock().expect("screen lock").snapshot(),
            };
            let last = self.last.as_ref().filter(|l| l.window == id).map(|l| &l.screen);
            frame = screen_bytes(&snapshot, last);
            self.last = Some(LastFrame { window: id, screen: snapshot });
        }
        // A full paint starts with an erase, which wipes the two reserved
        // rows. The bar has to be redrawn with them, so the last bar only
        // counts as painted while the last screen does.
        let last_bar = if had_last { self.last_bar.as_deref() } else { None };
        frame.extend(bottom_bytes(&bar, last_bar, &self.editor, self.rows, self.cols));
        out.write_all(&frame)?;
        out.flush()?;
        self.last_bar = Some(bar);
        Ok(())
    }
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
    "/set", "/unset", "/save", "/load", "/connect", "/disconnect", "/new", "/close", "/windows",
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
    /// Open a window on a copy of the lobby's settings, or on that file.
    NewWindow {
        file: Option<String>,
    },
    /// Close the current window. The front end refuses for the lobby and
    /// for a connected window.
    CloseWindow,
    /// List the windows.
    Windows,
    /// Switch to window `n`, 1-based.
    Switch(usize),
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
        "/new" => Some(KeyOutcome::NewWindow {
            file: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/close" => Some(KeyOutcome::CloseWindow),
        "/windows" => Some(KeyOutcome::Windows),
        _ if verb.starts_with('/') && verb.len() == 2 && verb.as_bytes()[1].is_ascii_digit() && verb != "/0" => {
            Some(KeyOutcome::Switch((verb.as_bytes()[1] - b'0') as usize))
        }
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
/new [file]          open a window on the lobby's settings, or on a profile file
/close               close this window, once it is disconnected
/windows             list the windows
/1 .. /9             switch to a window, the lobby is /1
PageUp, PageDown     scroll this window, any other key returns to the bottom
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

/// The name a job or the assist will watch for its own death line. The
/// board gives it on the stat sheet, so the profile's `username` only
/// has to carry it before the character is in the realm.
pub fn needs_name(session: &Session) -> Result<String, String> {
    session
        .character_name()
        .ok_or_else(|| "no character name yet. Enter the realm first, or /set username <name>".into())
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
pub(crate) fn new_assist(session: &Session, cfg: &crate::bot::BotConfig) -> (crate::bot::Bot, crate::sheet::HealState) {
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

/// The two reserved rows. The bar is written only when its text changed,
/// and always by overwriting the row with text padded to the width. A
/// clear-line before the rewrite is what made the bar flash over SSH.
pub fn bottom_bytes(bar: &str, last_bar: Option<&str>, editor: &InputEditor, rows: u16, cols: u16) -> Vec<u8> {
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let width = cols as usize;
    let mut out = Vec::new();
    if last_bar != Some(bar) {
        let padded = fit(bar, width);
        out.extend_from_slice(format!("\x1b[{status_row};1H\x1b[7m{padded}\x1b[0m").as_bytes());
    }
    let line = fit(&format!("> {}", editor.line()), width);
    let cursor_col = 3 + editor.cursor() as u16;
    // Shown last, every frame. A full screen paint replays whatever the
    // board sent, and a board that hid the cursor for its own full-screen
    // screen leaves it hidden over the input line otherwise.
    out.extend_from_slice(
        format!("\x1b[{input_row};1H{line}\x1b[{input_row};{cursor_col}H\x1b[?25h").as_bytes(),
    );
    out
}

/// The active window's screen. A full paint the first time a window is
/// shown, a diff after that. The diff assumes the terminal cursor is
/// where the last frame's screen left it and the pen set to what the
/// last frame was drawing with, so both are restored first. The bar
/// resets the pen between frames, so without this a coloured board's
/// diff paints in the default colour.
pub fn screen_bytes(cur: &vt100::Screen, last: Option<&vt100::Screen>) -> Vec<u8> {
    match last {
        None => cur.contents_formatted(),
        Some(last) => {
            let (row, col) = last.cursor_position();
            let mut out = format!("\x1b[{};{}H", row + 1, col + 1).into_bytes();
            out.extend(last.attributes_formatted());
            out.extend(cur.contents_diff(last));
            out
        }
    }
}

/// irssi's activity list: the numbers of windows with an unread major
/// event, or nothing.
pub fn act_suffix(unread: &[usize]) -> String {
    if unread.is_empty() {
        return String::new();
    }
    let list: Vec<String> = unread.iter().map(|n| n.to_string()).collect();
    format!("  Act: {}", list.join(","))
}

/// The bar for the window on screen. `None` is the lobby.
pub fn window_bar(number: usize, info: Option<&crate::window::WindowInfo>, unread: &[usize], cols: u16) -> String {
    let own = match info {
        None => "lobby".to_string(),
        Some(info) => info.bar.clone(),
    };
    fit(&format!("{number}: {own}{}", act_suffix(unread)), cols as usize)
}

/// `/windows`: one line per window.
pub fn windows_listing(infos: &[(usize, Option<crate::window::WindowInfo>)]) -> String {
    infos
        .iter()
        .map(|(n, info)| match info {
            None => format!("{n}: lobby"),
            Some(i) => {
                let who = if i.username.is_empty() {
                    String::new()
                } else {
                    format!("{}@", i.username)
                };
                let state = if i.connected { "connected" } else { "not connected" };
                let unsaved = if i.dirty { ", unsaved" } else { "" };
                format!("{n}: {who}{}:{} {state}{unsaved}", i.host, i.port)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `/quit` with windows open: refuse once, naming what would be lost.
/// The second `/quit` in a row goes through. Any other command disarms.
pub fn quit_windows_refusal(connected: &[usize], dirty: &[usize], armed: &mut bool) -> Option<String> {
    if connected.is_empty() && dirty.is_empty() {
        return None;
    }
    if *armed {
        return None;
    }
    *armed = true;
    let mut parts = Vec::new();
    for n in connected {
        parts.push(format!("window {n} is connected"));
    }
    for n in dirty {
        parts.push(format!("window {n} is unsaved"));
    }
    Some(format!("-- {}. /quit again to exit anyway --", parts.join(", ")))
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
    pub(crate) handle: tokio::task::JoinHandle<()>,
    pub(crate) phase: tokio::sync::watch::Receiver<crate::farm::Phase>,
    /// "farm" or "go", for the retirement notice.
    pub(crate) what: &'static str,
}

/// Start a farm run on an already-connected session.
///
/// Everything it needs comes from the profile the session was opened
/// with, so `/farm` needs no arguments. Errors are the operator's to read,
/// not a reason to drop the connection — being told "no [farm] table" and
/// staying logged in is strictly better than being thrown out.
///
/// Refused outright when the character has no name, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The name is the one off the stat sheet once the
/// character is in the realm, and the profile's `username` before that.
/// The check lives here rather than at the call sites so that a new
/// caller cannot forget it. Public for the test that pins the refusal.
pub fn start_farm(session: Arc<Session>, loop_name: Option<&str>) -> Result<Job, String> {
    let profile = session.profile();
    needs_name(&session)?;
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
/// Refused outright when the character has no name, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The name is the one off the stat sheet once the
/// character is in the realm, and the profile's `username` before that.
/// The check lives here rather than at the call sites so that a new
/// caller cannot forget it. Public for the test that pins the refusal.
pub fn start_roam(
    session: Arc<Session>,
    walls: crate::roam::Walls,
    here: crate::lost::Fix,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_name(&session)?;
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
                        crate::farm::FarmEnd::Died => crate::farm::DIED,
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
/// Refused outright when the character has no name, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The name is the one off the stat sheet once the
/// character is in the realm, and the profile's `username` before that.
/// The check lives here rather than at the call sites so that a new
/// caller cannot forget it. Public for the test that pins the refusal.
pub fn start_go(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
    to: mud_core::content::RoomId,
    bot: crate::bot::BotConfig,
    walking: bool,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_name(&session)?;
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
                crate::farm::Phase::Done { why: crate::farm::DIED.into(), at: None }
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
/// Refused outright when the character has no name, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The name is the one off the stat sheet once the
/// character is in the realm, and the profile's `username` before that.
/// The check lives here rather than at the call sites so that a new
/// caller cannot forget it. Public for the test that pins the refusal.
pub fn start_bank(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
    bot: crate::bot::BotConfig,
    walking: bool,
) -> Result<Job, String> {
    let profile = session.profile();
    needs_name(&session)?;
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
                crate::farm::Phase::Done { why: crate::farm::DIED.into(), at: None }
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
/// Refused outright when the character has no name, because a runner
/// that cannot recognise the character's own death line drives a corpse
/// around the board. The name is the one off the stat sheet once the
/// character is in the realm, and the profile's `username` before that.
/// The check lives here rather than at the call sites so that a new
/// caller cannot forget it. Public for the test that pins the refusal.
pub fn start_where(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    hint: Option<mud_core::content::RoomId>,
) -> Result<Job, String> {
    needs_name(&session)?;
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
pub(crate) fn content_path(profile: &crate::profile::Profile) -> std::path::PathBuf {
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
pub(crate) fn here_or(
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
pub(crate) fn describe_loops(graph: Option<&crate::graph::RoomGraph>, name: Option<&str>) -> Vec<String> {
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
pub(crate) fn import_loop(graph: &crate::graph::RoomGraph, file: &std::path::Path) -> Vec<String> {
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

