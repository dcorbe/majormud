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
    let mut state_rx = session.state();
    let target = match session.profile().target {
        crate::dialect::Target::MbbsEmu => "mbbs",
        crate::dialect::Target::RustServer => "rust",
    };

    crossterm::terminal::enable_raw_mode()?;
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    let mut editor = InputEditor::new();

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
    redraw_bottom(&mut out, &state_rx.borrow().clone(), target, &editor, cols, rows)?;

    let result = loop {
        tokio::select! {
            bytes = raw_rx.recv() => match bytes {
                Ok(bytes) => {
                    // Into the scroll region: restore server cursor,
                    // write verbatim, save it again.
                    out.write_all(b"\x1b8")?;
                    out.write_all(&bytes)?;
                    out.write_all(b"\x1b7")?;
                    redraw_bottom(&mut out, &state_rx.borrow().clone(), target, &editor, cols, rows)?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break Ok(()), // disconnected
            },
            changed = state_rx.changed() => {
                if changed.is_err() { break Ok(()); }
                let s = state_rx.borrow().clone();
                redraw_bottom(&mut out, &s, target, &editor, cols, rows)?;
            }
            ev = key_rx.recv() => {
                let Some(ev) = ev else { break Ok(()) };
                match ev {
                    TermEvent::Resize(w, h) => {
                        cols = w;
                        rows = h;
                        setup_region(&mut out, rows)?;
                        redraw_bottom(&mut out, &state_rx.borrow().clone(), target, &editor, cols, rows)?;
                    }
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        if handle_key(&key, &mut editor, &session) {
                            break Ok(());
                        }
                        redraw_bottom(&mut out, &state_rx.borrow().clone(), target, &editor, cols, rows)?;
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
fn handle_key(key: &KeyEvent, editor: &mut InputEditor, session: &Session) -> bool {
    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
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
            if line == "/quit" {
                return true;
            }
            session.send(&line);
        }
        _ => {}
    }
    false
}

fn setup_region(out: &mut impl std::io::Write, rows: u16) -> std::io::Result<()> {
    let region_bottom = rows.saturating_sub(2).max(1);
    // Set the region, park the server cursor at its bottom, save it.
    out.write_all(format!("\x1b[1;{region_bottom}r\x1b[{region_bottom};1H\x1b7").as_bytes())?;
    out.flush()
}

fn redraw_bottom(
    out: &mut impl std::io::Write,
    state: &GameState,
    target: &str,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let status = render_status(state, target, cols as usize);
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
pub fn render_status(state: &GameState, target: &str, width: usize) -> String {
    let mut s = format!("HP {}", state.hp);
    if let Some(ma) = state.mana {
        s.push_str(&format!(" MA {ma}"));
    }
    if let Some(room) = &state.room {
        s.push_str(&format!(" | {}", room.name));
    }
    s.push_str(&format!(" | {target}"));
    let mut out: Vec<char> = s.chars().collect();
    out.truncate(width);
    while out.len() < width {
        out.push(' ');
    }
    out.into_iter().collect()
}
