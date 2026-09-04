//! Wire-layer tests: telnet IAC filtering, CP437 decoding, anti-bot
//! backspace resolution, and ANSI stripping.
//!
//! Ground truth comes from `tools/oracle/mudlib.py` (the Python driver
//! whose behavior this layer replaces).

use mud_client::wire::{AnsiStripper, TelnetFilter, cp437_to_string, resolve_backspaces, strip_ansi};

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const ECHO: u8 = 1;
const SGA: u8 = 3;

#[test]
fn telnet_do_is_stripped_and_refused() {
    let mut f = TelnetFilter::new();
    let out = f.push(&[b'a', IAC, DO, ECHO, b'b']);
    assert_eq!(out.data, b"ab");
    assert_eq!(out.replies, vec![IAC, WONT, ECHO]);
}

#[test]
fn telnet_will_is_stripped_and_refused() {
    let mut f = TelnetFilter::new();
    let out = f.push(&[IAC, WILL, SGA]);
    assert_eq!(out.data, b"");
    assert_eq!(out.replies, vec![IAC, DONT, SGA]);
}

#[test]
fn telnet_wont_dont_produce_no_reply() {
    let mut f = TelnetFilter::new();
    let out = f.push(&[IAC, WONT, ECHO, IAC, DONT, SGA, b'x']);
    assert_eq!(out.data, b"x");
    assert_eq!(out.replies, b"");
}

#[test]
fn telnet_escaped_iac_yields_literal_255() {
    let mut f = TelnetFilter::new();
    let out = f.push(&[b'a', IAC, IAC, b'b']);
    assert_eq!(out.data, vec![b'a', 255, b'b']);
    assert_eq!(out.replies, b"");
}

#[test]
fn telnet_subnegotiation_is_skipped() {
    let mut f = TelnetFilter::new();
    let out = f.push(&[b'a', IAC, SB, 31, 0, 80, 0, 24, IAC, SE, b'b']);
    assert_eq!(out.data, b"ab");
}

#[test]
fn telnet_sequence_split_across_pushes() {
    let mut f = TelnetFilter::new();
    let out1 = f.push(&[b'a', IAC]);
    assert_eq!(out1.data, b"a");
    let out2 = f.push(&[DO, ECHO, b'b']);
    assert_eq!(out2.data, b"b");
    assert_eq!(out2.replies, vec![IAC, WONT, ECHO]);
}

#[test]
fn cp437_ascii_passthrough() {
    assert_eq!(cp437_to_string(b"Hello [HP=35]:"), "Hello [HP=35]:");
}

#[test]
fn cp437_box_drawing_and_high_bytes() {
    assert_eq!(cp437_to_string(&[0xC9, 0xCD, 0xBB]), "\u{2554}\u{2550}\u{2557}"); // ╔═╗
    assert_eq!(cp437_to_string(&[0xB3]), "\u{2502}"); // │
    assert_eq!(cp437_to_string(&[0xFF]), "\u{A0}"); // NBSP
    assert_eq!(cp437_to_string(&[0x82]), "\u{E9}"); // é
}

#[test]
fn cp437_controls_preserved() {
    assert_eq!(cp437_to_string(b"\x1b[1m\r\n\x08"), "\x1b[1m\r\n\x08");
}

#[test]
fn backspace_resolves_antibot_junk() {
    // The live board embeds junk-char+BS pairs inside words: nP\borth = north.
    assert_eq!(resolve_backspaces("nP\x08orth"), "north");
    assert_eq!(
        resolve_backspaces("Obvious exits: nP\x08orth, sM\x08outh, wN\x08est, sW\x08outheast"),
        "Obvious exits: north, south, west, southeast"
    );
}

#[test]
fn backspace_at_start_is_dropped() {
    assert_eq!(resolve_backspaces("\x08abc"), "abc");
}

#[test]
fn backspace_consumes_multiple() {
    assert_eq!(resolve_backspaces("abc\x08\x08"), "a");
}

#[test]
fn backspace_never_eats_line_breaks() {
    assert_eq!(resolve_backspaces("ab\r\n\x08cd"), "ab\r\ncd");
}

#[test]
fn strip_ansi_matches_python_regex() {
    // Python: re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", s)
    assert_eq!(strip_ansi("\x1b[1;36mRoom Name\x1b[0m"), "Room Name");
    assert_eq!(strip_ansi("\x1b[2J\x1b[H\x1b[?7h\x1b[40mtext"), "text");
    // Sequence without an alphabetic final byte does not match; it stays.
    assert_eq!(strip_ansi("a\x1b[12"), "a\x1b[12");
    // Bare ESC (no bracket) stays.
    assert_eq!(strip_ansi("a\x1bb"), "a\x1bb");
}

#[test]
fn ansi_stripper_streams_across_chunk_boundaries() {
    // The session transcript is built incrementally; escape sequences
    // split at read boundaries must not leak into it.
    let mut s = AnsiStripper::new();
    let mut out = String::new();
    out.push_str(&s.push("a\x1b["));
    out.push_str(&s.push("1;3"));
    out.push_str(&s.push("6mb"));
    assert_eq!(out, "ab");
}

#[test]
fn ansi_stripper_matches_batch_strip_on_whole_input() {
    let input = "\x1b[2J\x1b[Hplain \x1b[1;36mtitle\x1b[0m rest\x1b[?25l tail";
    let mut s = AnsiStripper::new();
    let streamed: String = input.chars().map(|c| s.push(&c.to_string())).collect();
    assert_eq!(streamed, strip_ansi(input));
}

#[test]
fn ansi_stripper_keeps_non_matching_escapes() {
    // Bare ESC without '[' passes through, like the Python regex.
    let mut s = AnsiStripper::new();
    let mut out = String::new();
    out.push_str(&s.push("a\x1b"));
    out.push_str(&s.push("zb"));
    assert_eq!(out, "a\x1bzb");
}
