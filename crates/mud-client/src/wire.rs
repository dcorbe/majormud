//! Wire layer: telnet IAC filtering, CP437 decoding, anti-bot backspace
//! resolution, and ANSI stripping.
//!
//! Pipeline order for parsing: bytes -> [`TelnetFilter`] -> [`cp437_to_string`]
//! -> [`strip_ansi`] -> [`resolve_backspaces`]. The interactive client passes
//! raw post-telnet bytes through to the terminal untouched; only the parser
//! consumes the cleaned form.

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

/// Result of pushing bytes through a [`TelnetFilter`].
pub struct TelnetOutput {
    /// Application data with telnet commands removed.
    pub data: Vec<u8>,
    /// Negotiation replies that must be written back to the server.
    pub replies: Vec<u8>,
}

/// Parser state carried across `push` calls; a telnet command may be split
/// at any byte boundary between socket reads.
enum State {
    Data,
    Iac,
    Option(u8),
    Sub,
    SubIac,
}

/// Stateful telnet command filter. Refuses every option (DO -> WONT,
/// WILL -> DONT), skips subnegotiation, and unescapes IAC IAC.
pub struct TelnetFilter {
    state: State,
}

impl TelnetFilter {
    pub fn new() -> Self {
        TelnetFilter { state: State::Data }
    }

    pub fn push(&mut self, input: &[u8]) -> TelnetOutput {
        let mut data = Vec::with_capacity(input.len());
        let mut replies = Vec::new();
        for &b in input {
            match self.state {
                State::Data => {
                    if b == IAC {
                        self.state = State::Iac;
                    } else {
                        data.push(b);
                    }
                }
                State::Iac => match b {
                    IAC => {
                        data.push(IAC);
                        self.state = State::Data;
                    }
                    DO | DONT | WILL | WONT => self.state = State::Option(b),
                    SB => self.state = State::Sub,
                    _ => self.state = State::Data, // NOP, GA, etc.
                },
                State::Option(cmd) => {
                    match cmd {
                        DO => replies.extend_from_slice(&[IAC, WONT, b]),
                        WILL => replies.extend_from_slice(&[IAC, DONT, b]),
                        _ => {} // DONT/WONT need no answer
                    }
                    self.state = State::Data;
                }
                State::Sub => {
                    if b == IAC {
                        self.state = State::SubIac;
                    }
                }
                State::SubIac => {
                    // IAC SE ends subnegotiation; IAC IAC is escaped data
                    // inside it (discarded either way); anything else means
                    // we are still inside the subnegotiation.
                    self.state = if b == SE { State::Data } else { State::Sub };
                }
            }
        }
        TelnetOutput { data, replies }
    }
}

impl Default for TelnetFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// CP437 upper half (0x80-0xFF): box drawing, accented letters, symbols.
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{A0}',
];

/// Decode CP437 (DOS codepage) bytes to a String. Bytes 0x00-0x7F map to
/// their ASCII identities (matching Python's cp437 codec); the high half
/// maps to box drawing, accented letters, and symbols.
pub fn cp437_to_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                CP437_HIGH[(b - 0x80) as usize]
            }
        })
        .collect()
}

/// Apply backspace semantics: each 0x08 removes the preceding character.
/// The live board hides junk-char+backspace pairs inside words as an
/// anti-bot measure (`nP\x08orth` reads "north"). Never removes past a
/// line break.
pub fn resolve_backspaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\x08' {
            match out.chars().last() {
                Some('\n') | Some('\r') | None => {} // drop the BS itself
                Some(_) => {
                    out.pop();
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Streaming variant of [`strip_ansi`]: escape sequences may split at
/// read boundaries; incomplete candidates are held until decidable.
pub struct AnsiStripper {
    /// Undecided prefix: "", "\x1b", or "\x1b[" + params so far.
    held: String,
}

impl AnsiStripper {
    pub fn new() -> Self {
        AnsiStripper { held: String::new() }
    }

    /// Feed a chunk, get the stripped text it completes.
    pub fn push(&mut self, chunk: &str) -> String {
        let mut out = String::with_capacity(chunk.len());
        for c in chunk.chars() {
            loop {
                if self.held.is_empty() {
                    if c == '\x1b' {
                        self.held.push(c);
                    } else {
                        out.push(c);
                    }
                    break;
                }
                if self.held == "\x1b" {
                    if c == '[' {
                        self.held.push(c);
                        break;
                    }
                    // Bare ESC is not a CSI candidate: flush and retry c.
                    out.push_str(&self.held);
                    self.held.clear();
                    continue;
                }
                // held is "\x1b[" + params
                if c.is_ascii_digit() || c == ';' || c == '?' {
                    self.held.push(c);
                    break;
                }
                if c.is_ascii_alphabetic() {
                    self.held.clear(); // complete match: strip it
                    break;
                }
                // Non-matching final byte: keep the sequence verbatim.
                out.push_str(&self.held);
                self.held.clear();
                continue;
            }
        }
        out
    }
}

impl Default for AnsiStripper {
    fn default() -> Self {
        Self::new()
    }
}

/// Remove ANSI CSI sequences, byte-for-byte equivalent to the Python
/// driver's `re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", s)`. A sequence
/// lacking an alphabetic final byte is not a match and is kept verbatim.
pub fn strip_ansi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && i + 1 < chars.len() && chars[i + 1] == '[' {
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';' || chars[j] == '?') {
                j += 1;
            }
            if j < chars.len() && chars[j].is_ascii_alphabetic() {
                i = j + 1; // matched: skip the whole sequence
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
