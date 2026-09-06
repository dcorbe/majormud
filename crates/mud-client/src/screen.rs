//! A window's in-memory terminal.
//!
//! The board's bytes go into a `vt100` parser instead of the real
//! terminal, so a window that is not on screen keeps its picture, and
//! switching to it paints that picture back exactly. The front end
//! takes a `snapshot` and diffs it against the last one it drew.

/// One window's terminal, `rows` by `cols`, with a scrollback capped at
/// the line count the profile's `scrollback_lines` names.
pub struct Screen {
    parser: vt100::Parser,
}

impl Screen {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Screen {
        Screen {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), scrollback),
        }
    }

    /// Bytes from the board, verbatim.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// One notice on its own line. Newlines are translated, because a
    /// bare `\n` in a terminal drops a row without returning the
    /// carriage and a multi-line notice would stairstep.
    pub fn note(&mut self, text: &str) {
        let body = text.replace('\n', "\r\n");
        let (_, col) = self.parser.screen().cursor_position();
        let lead = if col == 0 { "" } else { "\r\n" };
        self.parser.process(format!("{lead}{body}\r\n").as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// Scroll one screen height up into the scrollback, as far as it goes.
    pub fn page_up(&mut self) {
        let (rows, _) = self.size();
        let now = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(now + rows as usize);
    }

    pub fn page_down(&mut self) {
        let (rows, _) = self.size();
        let now = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(now.saturating_sub(rows as usize));
    }

    pub fn to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// True while PageUp has the view above the live rows.
    pub fn scrolled(&self) -> bool {
        self.parser.screen().scrollback() > 0
    }

    /// The rows in view as plain text, one line per row. For tests and
    /// for the lobby's log.
    pub fn text(&self) -> String {
        self.parser.screen().contents()
    }

    /// The terminal state, cloned, for the painter to diff against.
    pub fn snapshot(&self) -> vt100::Screen {
        self.parser.screen().clone()
    }
}
