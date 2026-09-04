//! CP437 as it travels on the wire between the server and a terminal.
//!
//! Below `0x80` the encoding is the identity. Control bytes are control
//! codes there, not glyphs: the ANSI escapes, the line endings and the
//! anti-bot backspaces all have to pass through untouched. Python's `cp437`
//! codec agrees, and the oracle harness relies on that agreement.

/// The high half of CP437 as Unicode. Index is the byte minus `0x80`.
pub const HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// Decode bytes arriving from or leaving for a terminal.
///
/// Identity below `0x80`, so control bytes pass through as themselves.
#[must_use]
pub fn decode_wire(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if b < 0x80 { b as char } else { HIGH[usize::from(b - 0x80)] })
        .collect()
}

/// Encode text as CP437.
///
/// Characters outside the codepage become `?`. One byte, like every other
/// character, because a DOS client reads whatever we send as CP437 no matter
/// what we meant.
///
/// # This can produce `0xFF`
///
/// `U+00A0` maps to `0xFF`, which on a telnet connection is IAC. Callers on a
/// telnet path must double it. That is not this function's job, since it does
/// not know what its output travels over, but it is this function's hazard.
#[must_use]
pub fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if (c as u32) < 0x80 {
                c as u8
            } else {
                HIGH.iter()
                    .position(|&high| high == c)
                    .map_or(b'?', |i| (i + 0x80) as u8)
            }
        })
        .collect()
}
