# `textscreen` Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the CP437 tables, the `Cell`/`Cells` grid and the diffing
terminal painter out of `dos-runtime` and `mud-core` into one crate both can
consume, removing a duplicated codepage table and making the painter testable.

**Architecture:** A leaf crate with no in-workspace dependencies. One
`[char; 256]` CP437 table serves two decode semantics -- wire (C0 is control)
and screen (C0 is a glyph) -- which today are two separate tables in two crates.
The painter moves unchanged except that it writes to an `impl Write` instead of
`print!`, which is what lets its row-diff be tested at all.

**Tech Stack:** Rust 2024, std only. No new dependencies.

**Spec:** `docs/2026-08-17-cnf-editor-design.md` (main `6cdbe5c3`)

## Global Constraints

- Edition `2024`; workspace lints `[workspace.lints.clippy] all = "warn"` apply.
- **Never run `cargo fmt`, `rustfmt`, or any formatter.** A bare `cargo fmt` in
  this workspace rewrites 159 files across every crate.
- Verify with `cargo test --workspace --no-run` **first** -- per-crate runs hide
  cross-crate compile breaks -- then `cargo test --workspace`.
- **Mutate before believing a test.** Every task has an explicit mutation step:
  break the implementation, watch the new test fail, restore. A test that still
  passes under mutation is not a test.
- Every file ends with a trailing newline.
- Commit message tags: `feat:`, `fix:`, `doc:`, `chore:`, `refactor:`, `style:`.
- Do not commit `.claude/` or `CLAUDE.md`.
- No new third-party dependencies in this plan.
- This is a **pure extraction**: no behaviour changes. If a move tempts you to
  fix something, leave it and note it; a refactor that also changes behaviour
  cannot be reviewed as either.

## File Structure

| File | Responsibility |
|---|---|
| `crates/textscreen/Cargo.toml` | Manifest, no deps |
| `crates/textscreen/src/lib.rs` | Crate docs, module declarations, re-exports |
| `crates/textscreen/src/cp437.rs` | One 256-entry table; `decode_wire`, `decode_screen`, `encode` |
| `crates/textscreen/src/cell.rs` | `Cell`, `Cells` and their query helpers |
| `crates/textscreen/src/paint.rs` | `Painter` -- the row-diffing renderer |
| `crates/textscreen/tests/cp437.rs` | Codepage tests, incl. the two moved from `mud-core` |
| `crates/textscreen/tests/paint.rs` | Painter tests -- new, impossible before this plan |
| `crates/dos-runtime/src/screen.rs` | Modified: `Screen` stays, `Cell`/`Cells` re-exported |
| `crates/dos-runtime/src/terminal.rs` | Modified: uses `Painter`, drops its own tables |
| `crates/mud-core/src/cp437.rs` | Deleted in Task 5 |

`Screen` stays in `dos-runtime`: `Screen::snapshot<G: Guest>` samples
`B800:0000` through a `Guest`, which is DOS-specific and has no place in a leaf
crate. Only the grid it produces is shared.

**`Widget` is deliberately not in this plan.** The spec lists it under
`textscreen`, and that is still where it lands -- but it lands in the `cnf`
plan, at the point a consumer exists. A trait with no implementor cannot be
tested and cannot be reviewed.

---

### Task 1: The crate, and CP437 with both semantics

**Files:**
- Create: `crates/textscreen/Cargo.toml`
- Create: `crates/textscreen/src/lib.rs`
- Create: `crates/textscreen/src/cp437.rs`
- Create: `crates/textscreen/tests/cp437.rs`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: nothing.
- Produces: `textscreen::cp437::{TABLE, decode_wire, decode_screen, encode}`
  where `TABLE: [char; 256]`, `decode_wire(&[u8]) -> String`,
  `decode_screen(&[u8]) -> String`, `encode(&str) -> Vec<u8>`.

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml`, add `"crates/textscreen",` to `members` after
`"crates/dos-runtime",`.

Create `crates/textscreen/Cargo.toml`:

```toml
[package]
name = "textscreen"
version = "0.1.0"
edition.workspace = true
license.workspace = true
publish.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Write the failing tests**

Create `crates/textscreen/tests/cp437.rs`:

```rust
//! CP437 is one table with two readings. These tests pin both, and pin that
//! they differ in exactly one place.

use textscreen::cp437::{decode_screen, decode_wire, encode};

#[test]
fn ascii_is_itself_in_both_readings() {
    for b in 0x20u8..0x7f {
        let s = String::from(b as char);
        assert_eq!(decode_wire(&[b]), s, "wire, byte {b:#04x}");
        assert_eq!(decode_screen(&[b]), s, "screen, byte {b:#04x}");
    }
}

#[test]
fn the_two_readings_differ_below_0x20_and_only_there() {
    // On the wire a C0 byte is a control code and must pass through: the ANSI
    // escapes, the line endings, the anti-bot backspaces all live here.
    assert_eq!(decode_wire(&[0x11]), "\u{11}");
    assert_eq!(decode_wire(&[0x1b]), "\u{1b}");
    // On a text screen the same byte is a glyph. 0x11 is a left-pointing
    // triangle, which is what a DOS menu draws its arrows with.
    assert_eq!(decode_screen(&[0x11]), "\u{25c4}");
}

#[test]
fn above_0x7f_the_two_readings_agree_entry_for_entry() {
    // This is the test that makes one table correct. If the high halves ever
    // diverge, the crate is secretly two tables again.
    for b in 0x80u8..=0xff {
        assert_eq!(
            decode_wire(&[b]),
            decode_screen(&[b]),
            "byte {b:#04x} disagrees between the two readings"
        );
    }
}

#[test]
fn the_high_half_is_box_drawing_and_accents() {
    assert_eq!(decode_wire(&[0xc9, 0xcd, 0xbb]), "\u{2554}\u{2550}\u{2557}");
    assert_eq!(decode_wire(&[0x82]), "\u{e9}");
}

#[test]
fn every_byte_survives_a_wire_round_trip() {
    let all: Vec<u8> = (0u8..=0xff).collect();
    assert_eq!(encode(&decode_wire(&all)), all);
}

#[test]
fn unmappable_characters_become_question_marks() {
    assert_eq!(encode("\u{4e2d}"), b"?".to_vec());
}

#[test]
fn encode_can_synthesize_the_telnet_iac_byte() {
    // CP437 0xFF is a non-breaking space -- and 0xFF is also telnet IAC. A
    // caller on a telnet path must double it. Pinned here so the hazard lives
    // with the function instead of in one caller's comment.
    assert_eq!(encode("\u{a0}"), vec![0xff]);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p textscreen --test cp437`
Expected: FAIL to compile -- `crates/textscreen/src/cp437.rs` does not exist.

- [ ] **Step 4: Write the implementation**

Create `crates/textscreen/src/lib.rs`:

```rust
//! A text screen: the codepage, the cell grid, and the painter that turns one
//! into ANSI.
//!
//! Extracted from `dos-runtime` and `mud-core`, which each owned a private
//! copy of part of it. The duplication was documented rather than fixed --
//! `dos-runtime`'s table said "this is a second copy of a table the workspace
//! already has" -- because neither crate was the right home: `mud-core` is the
//! MUD game crate and `dos-runtime` is a DOS runtime, and a Win32 console
//! needs the same grid as both.

pub mod cell;
pub mod cp437;
pub mod paint;
```

Create `crates/textscreen/src/cp437.rs`:

```rust
//! CP437, in the two readings a host needs.
//!
//! **The readings are not interchangeable and must not be merged.** On the
//! wire, bytes below `0x20` are control codes -- the ANSI escapes, the line
//! endings, the anti-bot backspaces -- and have to pass through untouched. In
//! a text-screen cell the same bytes are glyphs: `0x11` is a left-pointing
//! triangle, and a menu draws its arrows with it. Decoding a screen with the
//! wire reading throws the arrows away; decoding the wire with the screen
//! reading turns every escape into a face card.
//!
//! Both readings share one table. Below `0x80` the wire reading is the
//! identity, which is also what Python's `cp437` codec does -- the oracle
//! harness relies on that agreement.

/// CP437 as Unicode, all 256 entries. Index is the byte.
pub const TABLE: [char; 256] = [
    '\u{0}', '\u{263a}', '\u{263b}', '\u{2665}', '\u{2666}', '\u{2663}', '\u{2660}', '\u{2022}',
    '\u{25d8}', '\u{25cb}', '\u{25d9}', '\u{2642}', '\u{2640}', '\u{266a}', '\u{266b}', '\u{263c}',
    '\u{25ba}', '\u{25c4}', '\u{2195}', '\u{203c}', '\u{b6}', '\u{a7}', '\u{25ac}', '\u{21a8}',
    '\u{2191}', '\u{2193}', '\u{2192}', '\u{2190}', '\u{221f}', '\u{2194}', '\u{25b2}', '\u{25bc}',
    ' ', '!', '"', '#', '$', '%', '&', '\'',
    '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7',
    '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G',
    'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W',
    'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
    'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w',
    'x', 'y', 'z', '{', '|', '}', '~', '\u{2302}',
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
        .map(|&b| if b < 0x80 { b as char } else { TABLE[b as usize] })
        .collect()
}

/// Decode the contents of text-screen cells.
///
/// Every byte is a glyph, C0 included.
#[must_use]
pub fn decode_screen(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| TABLE[b as usize]).collect()
}

/// Encode text as CP437.
///
/// Characters outside the codepage become `?` -- one byte, like every other
/// character, because a DOS client reads whatever we send as CP437 no matter
/// what we meant.
///
/// # This can produce `0xFF`
///
/// `U+00A0` maps to `0xFF`, which on a telnet connection is IAC. Callers on a
/// telnet path must double it. That is not this function's job -- it does not
/// know what its output travels over -- but it is this function's hazard.
#[must_use]
pub fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if (c as u32) < 0x80 {
                c as u8
            } else {
                TABLE[0x80..]
                    .iter()
                    .position(|&high| high == c)
                    .map_or(b'?', |i| (i + 0x80) as u8)
            }
        })
        .collect()
}
```

Note the `encode` search starts at `0x80`: searching the whole table would let a
C0 glyph win a match and emit a control byte for a printable character.

Create empty placeholder modules so `lib.rs` compiles -- `crates/textscreen/src/cell.rs`
and `crates/textscreen/src/paint.rs`, each containing only:

```rust
//! Filled in by the next task.
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p textscreen --test cp437`
Expected: 7 passed.

- [ ] **Step 6: Mutate, and watch the tests fail**

Make each of these three changes one at a time, run
`cargo test -p textscreen --test cp437`, confirm the named test **fails**, then
revert:

1. In `decode_screen`, change the body to match `decode_wire`'s (`if b < 0x80 {
   b as char }`). Expect `the_two_readings_differ_below_0x20_and_only_there` to
   fail.
2. In `encode`, change `TABLE[0x80..]` to `TABLE` and drop the `+ 0x80`. Expect
   `every_byte_survives_a_wire_round_trip` to fail.
3. In `TABLE`, change entry `0xff` from `'\u{a0}'` to `' '`. Expect
   `encode_can_synthesize_the_telnet_iac_byte` to fail.

If any mutation leaves the suite green, the test for it is not testing it.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/textscreen
git commit -m "feat(textscreen): one CP437 table, two readings

The wire reading and the screen reading disagree below 0x20 and must: a C0
byte is a control code on a connection and a glyph in a cell. They agreed
above 0x7f already, in two separate tables in two crates, and a test now
pins that they still do."
```

---

### Task 2: Move `Cell` and `Cells`

**Files:**
- Modify: `crates/textscreen/src/cell.rs`
- Modify: `crates/dos-runtime/src/screen.rs:15-170`
- Modify: `crates/dos-runtime/Cargo.toml`

**Interfaces:**
- Consumes: `textscreen::cp437::decode_screen` from Task 1.
- Produces: `textscreen::cell::{Cell, Cells}`. `Cell { pub ch: u8, pub attr: u8 }`
  with `background(self) -> u8` and `foreground(self) -> u8`. `Cells { pub cols:
  usize, pub rows: usize, pub cells: Vec<Cell> }` with `blank(cols, rows) ->
  Self`, `cell(&self, row, col) -> Cell`, `line(&self, row) -> String`,
  `text(&self) -> String`, `contains(&self, &str) -> bool`, `find(&self, &str)
  -> Option<(usize, usize)>`, `highlighted_rows(&self, min_run: usize) ->
  Vec<usize>`, `selected(&self) -> Option<String>`.

- [ ] **Step 1: Move the code verbatim**

Cut `Cell` and `Cells` -- the whole of `crates/dos-runtime/src/screen.rs` lines
15 through the end of `impl Cells` -- into `crates/textscreen/src/cell.rs`,
including every doc comment. Add at the top of the new file:

```rust
//! A grid of text-screen cells.
//!
//! A `Cell` is a CP437 byte and a DOS attribute byte, which is what both a
//! sampled `B800:0000` and a Win32 console screen buffer hold, and what the
//! vendor's own `.SCN` screen images are made of (80x25 pairs, 4000 bytes).

use crate::cp437;
```

Anywhere the moved code decoded a byte to a character, it must now call
`cp437::decode_screen`. These are screen cells: C0 bytes are glyphs.

- [ ] **Step 2: Re-export from `dos-runtime` so nothing else changes yet**

In `crates/dos-runtime/Cargo.toml`, under `[dependencies]`:

```toml
textscreen = { path = "../textscreen" }
```

At the top of `crates/dos-runtime/src/screen.rs`, replacing the deleted types:

```rust
pub use textscreen::cell::{Cell, Cells};
```

`Screen` and its `snapshot<G: Guest>` stay exactly where they are.

- [ ] **Step 3: Verify the whole workspace still compiles**

Run: `cargo test --workspace --no-run`
Expected: compiles clean. A per-crate run would not catch a consumer of
`dos_runtime::screen::Cells` elsewhere in the workspace.

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace`
Expected: the same pass count as before this task. This is a pure move; a
changed count means something else changed.

- [ ] **Step 5: Mutate**

In `crates/textscreen/src/cell.rs`, change `Cells::blank` to fill with
`Cell { ch: b'x', attr: 7 }`. Run `cargo test --workspace`. Existing
`dos-runtime` screen tests must fail. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/textscreen crates/dos-runtime
git commit -m "refactor(textscreen): Cell and Cells move out of dos-runtime

A cell grid is not DOS-specific -- the Win32 console owns one outright and
never samples B800:0000. dos-runtime re-exports both so no consumer changes
in this commit."
```

---

### Task 3: Extract the painter, and make it testable

**Files:**
- Modify: `crates/textscreen/src/paint.rs`
- Create: `crates/textscreen/tests/paint.rs`

**Interfaces:**
- Consumes: `textscreen::cell::{Cell, Cells}`, `textscreen::cp437::TABLE`.
- Produces: `textscreen::paint::Painter` with `new() -> Self` and
  `paint(&mut self, out: &mut impl std::io::Write, grid: &Cells, cursor: (u8, u8),
  cursor_visible: bool) -> std::io::Result<()>`.

The one change from the code as it stands: it wrote with `print!` to stdout.
Taking an `impl Write` is what makes the row-diff observable, and it is the
reason this renderer has never had a test.

- [ ] **Step 1: Write the failing tests**

Create `crates/textscreen/tests/paint.rs`:

```rust
//! The painter's whole job is emitting less than a full screen. These tests
//! watch what it emits, which taking an `impl Write` is what allows.

use textscreen::cell::{Cell, Cells};
use textscreen::paint::Painter;

fn paint(p: &mut Painter, grid: &Cells) -> String {
    let mut out = Vec::new();
    p.paint(&mut out, grid, (0, 0), false).expect("write to a Vec");
    String::from_utf8(out).expect("painter emits UTF-8")
}

#[test]
fn the_first_paint_emits_every_row() {
    let grid = Cells::blank(80, 25);
    let mut p = Painter::new();
    let out = paint(&mut p, &grid);
    for row in 1..=25 {
        assert!(out.contains(&format!("\x1b[{row};1H")), "row {row} missing");
    }
}

#[test]
fn an_unchanged_repaint_emits_no_rows() {
    let grid = Cells::blank(80, 25);
    let mut p = Painter::new();
    let _ = paint(&mut p, &grid);
    let second = paint(&mut p, &grid);
    assert!(
        !second.contains("\x1b[1;1H"),
        "nothing changed, so no row should be addressed: {second:?}"
    );
}

#[test]
fn only_the_changed_row_is_repainted() {
    let mut grid = Cells::blank(80, 25);
    let mut p = Painter::new();
    let _ = paint(&mut p, &grid);

    grid.cells[3 * 80] = Cell { ch: b'A', attr: 7 };
    let out = paint(&mut p, &grid);

    assert!(out.contains("\x1b[4;1H"), "row 4 changed and must be addressed");
    assert!(!out.contains("\x1b[5;1H"), "row 5 did not change: {out:?}");
    assert!(!out.contains("\x1b[1;1H"), "row 1 did not change: {out:?}");
}

#[test]
fn dos_colour_order_is_remapped_not_passed_through() {
    // DOS counts blue as 1 and red as 4; ANSI the other way round. Attribute
    // 0x01 is blue on black, which is SGR 34 -- not 31.
    let mut grid = Cells::blank(1, 1);
    grid.cells[0] = Cell { ch: b'x', attr: 0x01 };
    let mut p = Painter::new();
    let out = paint(&mut p, &grid);
    assert!(out.contains(";34;40m"), "blue must become 34, got {out:?}");
}

#[test]
fn a_c0_byte_in_a_cell_paints_as_a_glyph() {
    // 0x11 in a cell is a left-pointing triangle. Emitting it as a control
    // byte would move the cursor instead of drawing an arrow.
    let mut grid = Cells::blank(1, 1);
    grid.cells[0] = Cell { ch: 0x11, attr: 7 };
    let mut p = Painter::new();
    let out = paint(&mut p, &grid);
    assert!(out.contains('\u{25c4}'), "expected an arrow glyph: {out:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p textscreen --test paint`
Expected: FAIL to compile -- `Painter` does not exist.

- [ ] **Step 3: Move the painter**

Replace `crates/textscreen/src/paint.rs` with the body of
`Terminal::paint` from `crates/dos-runtime/src/terminal.rs:171-219`, plus the
`TO_ANSI` constant from `:65`, restructured as:

```rust
//! Turning a cell grid into ANSI, one changed row at a time.
//!
//! The diff is per row, not per cell. That is enough for a local terminal and
//! for the guest screens this was built for; tightening it to runs of cells is
//! a change to make when something measures a need for it, not before.

use std::io::{self, Write};

use crate::cell::{Cell, Cells};
use crate::cp437::TABLE;

/// DOS attribute colour order to ANSI's.
///
/// The two differ: DOS counts blue as 1 and red as 4, ANSI the other way
/// round. Passing the index through unchanged swaps every red and blue on the
/// screen.
const TO_ANSI: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

/// A terminal's worth of remembered state: what was last painted.
#[derive(Default)]
pub struct Painter {
    last: Vec<Cell>,
}

impl Painter {
    #[must_use]
    pub fn new() -> Self {
        Self { last: Vec::new() }
    }

    /// Redraw, skipping rows that have not changed since the last paint.
    ///
    /// Writes to `out` rather than stdout so that what it chose to emit can be
    /// inspected. Does not flush -- the caller owns that decision.
    pub fn paint(
        &mut self,
        out: &mut impl Write,
        grid: &Cells,
        cursor: (u8, u8),
        cursor_visible: bool,
    ) -> io::Result<()> {
        let mut buf = String::with_capacity(8 * 1024);
        buf.push_str("\x1b[?25l");

        let unchanged = self.last.len() == grid.cells.len();
        for row in 0..grid.rows {
            let start = row * grid.cols;
            let end = start + grid.cols;
            if unchanged && self.last[start..end] == grid.cells[start..end] {
                continue;
            }
            buf.push_str(&format!("\x1b[{};1H", row + 1));
            let mut attr = None;
            for col in 0..grid.cols {
                let cell = grid.cell(row, col);
                if attr != Some(cell.attr) {
                    let fg = cell.foreground();
                    let bg = cell.background();
                    let fg_code = if fg >= 8 {
                        90 + u16::from(TO_ANSI[usize::from(fg - 8)])
                    } else {
                        30 + u16::from(TO_ANSI[usize::from(fg)])
                    };
                    let bg_code = 40 + u16::from(TO_ANSI[usize::from(bg)]);
                    buf.push_str(&format!("\x1b[0;{fg_code};{bg_code}m"));
                    attr = Some(cell.attr);
                }
                buf.push(TABLE[usize::from(cell.ch)]);
            }
            buf.push_str("\x1b[0m");
        }

        let (row, col) = cursor;
        buf.push_str(&format!(
            "\x1b[{};{}H",
            u16::from(row) + 1,
            u16::from(col) + 1
        ));
        buf.push_str(if cursor_visible {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });

        out.write_all(buf.as_bytes())?;
        self.last = grid.cells.clone();
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p textscreen --test paint`
Expected: 5 passed.

- [ ] **Step 5: Mutate**

One at a time, run `cargo test -p textscreen --test paint`, confirm the named
test fails, revert:

1. Delete the `continue` in the unchanged-row branch. Expect
   `an_unchanged_repaint_emits_no_rows` and `only_the_changed_row_is_repainted`
   to fail.
2. Change `TO_ANSI` to `[0, 1, 2, 3, 4, 5, 6, 7]`. Expect
   `dos_colour_order_is_remapped_not_passed_through` to fail.
3. Change `buf.push(TABLE[usize::from(cell.ch)])` to
   `buf.push(cell.ch as char)`. Expect `a_c0_byte_in_a_cell_paints_as_a_glyph`
   to fail.

- [ ] **Step 6: Commit**

```bash
git add crates/textscreen
git commit -m "feat(textscreen): the diffing painter, now testable

Same renderer, writing to an impl Write instead of print!. That is the whole
behavioural difference, and it is why this code has five tests today and none
for the last two phases -- a painter that prints to stdout cannot be asked
what it chose not to print."
```

---

### Task 4: `dos-runtime` uses the extracted painter

**Files:**
- Modify: `crates/dos-runtime/src/terminal.rs:41-65` (delete both tables)
- Modify: `crates/dos-runtime/src/terminal.rs:143-219` (`Terminal` holds a `Painter`)

**Interfaces:**
- Consumes: `textscreen::paint::Painter`, `textscreen::cp437`.
- Produces: no API change. `Terminal::new`, `Terminal::QUIT`,
  `Terminal::quit_requested` and `impl Driver for Terminal` keep their
  signatures.

- [ ] **Step 1: Delete the duplicated tables**

Remove `const CP437: [char; 256]` (`:41`) and `const TO_ANSI: [u8; 8]` (`:65`)
from `crates/dos-runtime/src/terminal.rs` entirely. They now live in
`textscreen`. Keep the doc comment's explanation of *why* the screen reading
differs from the wire reading, moved to wherever the file still needs it.

- [ ] **Step 2: Swap the field and the call**

In `Terminal`, replace `last: Vec<Cell>` with `painter: Painter`, and
initialise it in `new()` with `Painter::new()`. Replace the whole private
`paint` method with:

```rust
    fn paint(&mut self, grid: &Cells, cursor: (u8, u8), cursor_visible: bool) {
        let mut out = io::stdout().lock();
        let _ = self.painter.paint(&mut out, grid, cursor, cursor_visible);
        let _ = out.flush();
    }
```

The flush stays here: the painter deliberately does not flush, because a caller
batching several paints should not pay for each one.

- [ ] **Step 3: Verify the workspace compiles**

Run: `cargo test --workspace --no-run`
Expected: clean.

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace`
Expected: same pass count as Task 2 left it.

- [ ] **Step 5: Prove the guest path still renders**

The suite does not cover the interactive path end to end, so check it by hand.
Run the DOS runtime against LORD as the win32/dos work does today, confirm the
menu paints with its box drawing and arrows intact, and quit with Ctrl-`]`. A
red/blue swap or missing arrows is what a bad extraction looks like here, and
both are visible instantly.

- [ ] **Step 6: Commit**

```bash
git add crates/dos-runtime
git commit -m "refactor(dos-runtime): paint through textscreen

Deletes the second CP437 table and the second DOS-to-ANSI colour map. The
table's own doc comment had described itself as a duplicate since it was
written."
```

---

### Task 5: Delete `mud_core::cp437`

**Files:**
- Delete: `crates/mud-core/src/cp437.rs`
- Delete: `crates/mud-core/tests/cp437.rs` (its cases live in `textscreen`'s now)
- Modify: `crates/mud-core/src/lib.rs:11`
- Modify: `crates/mbbs-server/src/conn.rs`, `crates/mbbs-server/src/termcompat.rs`
- Modify: `crates/mud-client/src/wire.rs`, `crates/mud-server/src/server.rs`
- Modify: `crates/mbbs-server/Cargo.toml`, `crates/mud-client/Cargo.toml`, `crates/mud-server/Cargo.toml`

**Interfaces:**
- Consumes: `textscreen::cp437::{decode_wire, encode}`.
- Produces: nothing new. `mud_core::cp437` ceases to exist.

Every one of these call sites is on the **wire**, so each `cp437::decode`
becomes `cp437::decode_wire`. That is a rename, not a semantic change: the old
`decode` was the wire reading.

- [ ] **Step 1: Add the dependency to the three consuming crates**

In each of `crates/mbbs-server/Cargo.toml`, `crates/mud-client/Cargo.toml` and
`crates/mud-server/Cargo.toml`, under `[dependencies]`:

```toml
textscreen = { path = "../textscreen" }
```

- [ ] **Step 2: Repoint every call site**

Across `crates/mbbs-server/src/conn.rs`, `crates/mbbs-server/src/termcompat.rs`,
`crates/mud-client/src/wire.rs` and `crates/mud-server/src/server.rs`:

- `use mud_core::cp437;` becomes `use textscreen::cp437;`
- `cp437::decode(` becomes `cp437::decode_wire(`
- `cp437::encode(` is unchanged

Find them all with:

```bash
grep -rn "mud_core::cp437\|cp437::decode" crates/*/src crates/*/tests
```

Do not change `crates/dos-runtime`, which Task 4 already handled and which uses
the screen reading.

- [ ] **Step 3: Delete the module**

Remove `crates/mud-core/src/cp437.rs`, remove `pub mod cp437;` from
`crates/mud-core/src/lib.rs:11`, and remove `crates/mud-core/tests/cp437.rs` --
`textscreen/tests/cp437.rs` already carries `ascii_is_itself`,
`control_bytes_pass_through` (as
`the_two_readings_differ_below_0x20_and_only_there`),
`the_high_half_is_box_drawing_and_accents`, `every_byte_survives_a_round_trip`
and `unmappable_characters_become_question_marks`.

- [ ] **Step 4: Verify the workspace compiles, then run it**

Run: `cargo test --workspace --no-run`
Expected: clean. Any missed call site fails here, by name.

Run: `cargo test --workspace`
Expected: total count drops by exactly the 5 tests deleted from
`mud-core/tests/cp437.rs`, and nothing else changes.

- [ ] **Step 5: Mutate**

In `textscreen::cp437::decode_wire`, drop the `b < 0x80` guard so it becomes
`decode_screen`. Run `cargo test --workspace`. The wire-facing suites --
`mbbs-server` and `mud-client` -- must fail, because an ANSI escape in a
transcript would now decode to a glyph. Revert.

If they do *not* fail, the wire path has no test that distinguishes the two
readings, which is the exact bug this crate exists to make impossible. Say so
rather than moving on.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: cp437 leaves mud-core for textscreen

mud-core is the MUD game crate; a codepage shared by the DOS runtime, the
telnet transport and the client does not belong to it. Its own doc comment
said as much. decode becomes decode_wire at every call site -- a rename, since
the old decode was already the wire reading."
```

---

## Self-Review

**Spec coverage.** The spec's `textscreen` section names CP437 (both semantics +
encode), `Cell`/`Cells`, the diffing painter, DOS-attr-to-SGR, and `Widget`.
Tasks 1-5 cover all but `Widget`, which is deferred to the `cnf` plan with the
reason stated in File Structure. The spec's migration order -- crate first,
`dos-runtime` second, `mud-core` consumers third -- is Tasks 1/3, 4, 5.

**Type consistency.** `Cell`, `Cells`, `Painter`, `decode_wire`,
`decode_screen`, `encode` and `TABLE` are spelled identically in every task that
mentions them. `Painter::paint` takes `(&mut impl Write, &Cells, (u8, u8), bool)`
in Task 3's definition and in Task 4's call.

**Known non-coverage, stated rather than hidden.** Task 4's interactive check is
manual. The workspace has no automated test that drives a real terminal, and
inventing one to cover a pure extraction would be a larger change than the
extraction.
