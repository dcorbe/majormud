//! CP437, the DOS codepage the board speaks on the wire.
//!
//! The original ran on DOS and its callers were DOS terminals, so every
//! byte on the wire is CP437 in both directions: box drawing in the
//! banners, accented letters in item and monster names. Decoding as
//! UTF-8 instead mangles the high half into replacement characters, and
//! encoding as UTF-8 sends two bytes where a period client expects one.
//!
//! Bytes 0x00-0x7F are their ASCII identities (matching Python's cp437
//! codec, which the oracle harness uses), so control bytes — the ANSI
//! escapes, the line endings, the anti-bot backspaces — pass through
//! untouched.

/// CP437 upper half (0x80-0xFF): box drawing, accented letters, symbols.
const HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{A0}',
];

/// Decode CP437 bytes to text. Total: every byte has a character.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                HIGH[(b - 0x80) as usize]
            }
        })
        .collect()
}

/// Encode text as CP437 bytes. Characters outside the codepage become
/// `?` — one byte, like every other character, because a DOS client
/// reads whatever we send as CP437 no matter what we meant.
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
