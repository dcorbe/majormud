//! Wire-layer tests: telnet IAC filtering, CP437 decoding, anti-bot
//! backspace resolution, and ANSI stripping.
//!
//! Ground truth comes from `tools/oracle/mudlib.py` (the Python driver
//! whose behavior this layer replaces) and the captured live-board
//! transcript `re/oracle/oracle_m1.raw`.

use mud_client::wire::{TelnetFilter, cp437_to_string, resolve_backspaces, strip_ansi};

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
fn corpus_oracle_m1_pipeline() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../re/oracle/oracle_m1.raw"
    );
    let raw = std::fs::read(path).expect("oracle_m1.raw fixture");
    let mut f = TelnetFilter::new();
    let out = f.push(&raw);
    let text = resolve_backspaces(&strip_ansi(&cp437_to_string(&out.data)));
    assert!(text.contains("\u{2554}"), "banner box art survives cp437"); // ╔
    assert_eq!(text.matches("Obvious exits: ").count(), 4);
    assert!(text.contains("Obvious exits: north, south, west, southeast"));
    assert!(text.contains("[HP=35"));
    assert!(!text.contains('\x08'), "all backspaces resolved");
}
