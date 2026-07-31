//! The DOS codepage the board speaks on the wire, in both directions.

use mud_core::cp437;

#[test]
fn ascii_is_itself() {
    assert_eq!(cp437::decode(b"Obvious exits: north"), "Obvious exits: north");
    assert_eq!(cp437::encode("Obvious exits: north"), b"Obvious exits: north");
}

/// Control bytes are data, not text: the render preamble, the line
/// endings and the anti-bot backspaces all have to survive a decode.
#[test]
fn control_bytes_pass_through() {
    assert_eq!(cp437::decode(b"\x1b[1;36m\r\n\x08"), "\x1b[1;36m\r\n\x08");
    assert_eq!(cp437::encode("\x1b[1;36m\r\n\x08"), b"\x1b[1;36m\r\n\x08");
}

#[test]
fn the_high_half_is_box_drawing_and_accents() {
    assert_eq!(cp437::decode(&[0x82]), "\u{E9}"); // é
    assert_eq!(cp437::decode(&[0xC9, 0xCD, 0xBB]), "\u{2554}\u{2550}\u{2557}"); // ╔═╗
    assert_eq!(cp437::decode(&[0xFF]), "\u{A0}"); // NBSP
}

#[test]
fn every_byte_survives_a_round_trip() {
    let all: Vec<u8> = (0..=255u8).collect();
    assert_eq!(cp437::encode(&cp437::decode(&all)), all);
}

/// Anything outside the codepage becomes a question mark rather than
/// several bytes of UTF-8 — a DOS client would render that as mojibake.
#[test]
fn unmappable_characters_become_question_marks() {
    assert_eq!(cp437::encode("caf\u{E9} \u{4E2D}"), b"caf\x82 ?");
}
