# Prompt Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The client reads both of the board's prompt templates, carries the status word the prompt paints, tracks it in the game state, shows it in the status bar, and probes max mana beside max HP.

**Architecture:** `parse.rs` replaces its single prompt regex with a prompt reader that knows the HP-only template, which paints the status inside the frame, and the pool template, which paints it after the frame. `Event::Prompt` gains a `status` field of a closed `Status` type. `GameState` folds it. The vitals probe reads max mana from the same `Health:` line it already reads max HP from.

**Tech Stack:** Rust 2024 edition, tokio, regex, the existing `mud-client` test crates. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-04-prompt-status-and-recovery-design.md`, Part 1 and the "What the board sends" section. This plan is Phase 1 of that spec.

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with one newline.
- Commit after every task with a tagged message: `feat:`, `fix:`, `refactor:`, `doc:`, `test:`, `chore:`. Do not mention plans or specs in commit messages.
- Do not commit `.claude/` or `CLAUDE.md`.
- Do not use `/tmp`. Scratch files go under the repo and are deleted before commit.
- No test may read from `re/`. Test fixtures are string literals copied into the test.
- Prose in comments and docs: plain words, one idea per sentence, no em dashes, no parentheses, no semicolons.
- Run tests with `cargo test -p mud-client --test <name>`; the whole workspace with `cargo test --workspace`. Tests that need a live board or the Rust server are in `*_live.rs` and `live_server.rs` and are ignored by default.
- The status word type is closed: `Resting`, `Meditating`, and `Other(String)`. An unknown word must never hide a prompt.

## The board facts every task relies on

Two prompt templates from the game DLL. `%s` is a colour code except the last one, which is the status, ` (Resting) ` or ` (Meditating) ` with the spaces:

```
[HP=%s%d%s%s]:             HP only    ->  [HP=42 (Resting) ]:
[HP=%s%d%s/%s=%s%d%s]:%s   with pool  ->  [HP=36/MA=12]: (Resting) look
```

After ANSI stripping, the HP-only form reads `[HP=42 (Resting) ]:` and the pool form reads `[HP=36/MA=12]: (Resting) look`. Pool captions are `MA` or `KAI`. HP is signed. A pileup puts several prompts on one physical line: `[HP=16 (Resting) ]:[HP=17 (Resting) ]:exp`.

The raw bytes of the prompt that started this, from `test.raw` at `1788503928.343`:

```
\x1b[79D\x1b[K\x1b[0;37m[HP=42\x1b[0;37m (Resting) ]:look\r\n
```

---

### Task 1: The status type and the prompt field

**Files:**
- Modify: `crates/mud-client/src/events.rs:62-66`
- Modify: every `Event::Prompt { hp: .., mana: .. }` construction in `crates/mud-client/src`, `crates/mud-client/tests`, `crates/mud-client/examples` (about 66 sites, mechanical)
- Modify: `crates/mud-client/src/progress.rs:66` and `crates/mud-client/src/session.rs:1030` (destructuring patterns)
- Test: `crates/mud-client/tests/parse.rs`

**Interfaces:**
- Produces: `pub enum Status { Resting, Meditating, Other(String) }` in `mud_client::events`, with `Status::from_word(&str) -> Status` and `Status::word(&self) -> &str`.
- Produces: `Event::Prompt { hp: i32, mana: Option<i32>, status: Option<Status> }`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/parse.rs`:

```rust
// --- status ---

#[test]
fn a_status_word_round_trips_and_unknown_words_survive() {
    use mud_client::events::Status;
    assert_eq!(Status::from_word("Resting"), Status::Resting);
    assert_eq!(Status::from_word("Meditating"), Status::Meditating);
    assert_eq!(Status::from_word("Stunned"), Status::Other("Stunned".into()));
    assert_eq!(Status::Resting.word(), "Resting");
    assert_eq!(Status::Meditating.word(), "Meditating");
    assert_eq!(Status::Other("Stunned".into()).word(), "Stunned");
    // The field exists and a prompt can carry one.
    let ev = Event::Prompt { hp: 1, mana: None, status: Some(Status::Resting) };
    assert!(matches!(ev, Event::Prompt { status: Some(Status::Resting), .. }));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test parse a_status_word_round_trips`
Expected: compile error, `Status` not found in `mud_client::events`.

- [ ] **Step 3: Add the type and the field**

In `crates/mud-client/src/events.rs`, before `pub enum Event`:

```rust
/// The word the board paints into the prompt while the character is
/// in a recovery state. The DLL has exactly two, ` (Resting) ` and
/// ` (Meditating) `. Anything else the board ever paints lands in
/// `Other` so a new word can never hide a prompt again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Resting,
    Meditating,
    Other(String),
}

impl Status {
    pub fn from_word(word: &str) -> Status {
        match word {
            "Resting" => Status::Resting,
            "Meditating" => Status::Meditating,
            other => Status::Other(other.to_string()),
        }
    }

    pub fn word(&self) -> &str {
        match self {
            Status::Resting => "Resting",
            Status::Meditating => "Meditating",
            Status::Other(word) => word,
        }
    }
}
```

Change the `Prompt` variant:

```rust
    /// `[HP=35]:` / `[HP=26/MA=12]:` / `[HP=20/KAI=4]:` — HP may be
    /// negative while downed. `status` is the word the board paints
    /// while resting or meditating, `None` on a bare prompt.
    Prompt {
        hp: i32,
        mana: Option<i32>,
        status: Option<Status>,
    },
```

- [ ] **Step 4: Update every construction site mechanically**

Run from the repo root:

```bash
perl -0pi -e 's/(Prompt \{[^}]*?mana: (?:None|Some\([^)]*\)))(\s*)\}/$1, status: None$2}/g' \
  $(grep -rl "Prompt {" crates/mud-client/src crates/mud-client/tests crates/mud-client/examples)
```

This rewrites `Prompt { hp: 35, mana: None }` to `Prompt { hp: 35, mana: None, status: None }`, on one line or several. Patterns that already use `..` are untouched.

Then fix the two destructuring patterns by hand. `crates/mud-client/src/progress.rs:66`:

```rust
            Event::Prompt { hp, mana, .. } => {
```

`crates/mud-client/src/session.rs:1030` stays as it is for now. Task 3 changes it. For this task make it compile:

```rust
        Event::Prompt { hp, mana, .. } => {
```

`crates/mud-client/src/parse.rs` `prompt_event`:

```rust
fn prompt_event(c: &regex::Captures) -> Event {
    Event::Prompt {
        hp: c[1].parse().unwrap(),
        mana: c.get(2).map(|m| m.as_str().parse().unwrap()),
        status: None,
    }
}
```

- [ ] **Step 5: Build and run the whole client suite**

Run: `cargo build --workspace 2>&1 | grep -E "^(error|warning)" ; cargo test -p mud-client 2>&1 | tail -3`
Expected: no errors, no new warnings, every test passes including the new one. If the compiler names a site the perl command missed, add `status: None` there by hand.

- [ ] **Step 6: Check file endings and commit**

```bash
for f in $(git diff --name-only); do tail -c1 "$f" | xxd -p | grep -q 0a || echo "no newline: $f"; done
git add -A crates/mud-client
git commit -m "refactor(client): the prompt event carries a status word"
```

---

### Task 2: The parser reads both prompt templates

**Files:**
- Modify: `crates/mud-client/src/parse.rs:16-18` (the prompt regex), `:80-113` (end of buffer rule), `:127-166` (`handle_line`), `:270-276` (`prompt_event`)
- Test: `crates/mud-client/tests/parse.rs`
- Test: `crates/mud-client/tests/correlate.rs`

**Interfaces:**
- Consumes: `Status`, `Event::Prompt { hp, mana, status }` from Task 1.
- Produces: nothing new outside `parse.rs`. The `Parser::push` and `Parser::finish` signatures are unchanged.

- [ ] **Step 1: Write the failing parser tests**

Append to `crates/mud-client/tests/parse.rs`. The `use` line goes at the top of the file with the other imports.

```rust
use mud_client::events::Status;
```

```rust
// --- prompt status ---

#[test]
fn a_resting_prompt_keeps_its_status_inside_the_frame() {
    // test.raw, 2026-09-04, the exact bytes a /farm opening look died
    // on. The HP-only template paints the status before "]:".
    let ev = parse_all("\x1b[79D\x1b[K\x1b[0;37m[HP=42\x1b[0;37m (Resting) ]:look\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 42, mana: None, status: Some(Status::Resting) },
            Event::Line("look".into()),
        ]
    );
}

#[test]
fn a_pool_prompt_carries_its_status_after_the_frame() {
    // accept-run2.raw: the pool template paints the status after "]:",
    // glued to whatever follows.
    let ev = parse_all("[HP=36/MA=12]: (Resting) look\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 36, mana: Some(12), status: Some(Status::Resting) },
            Event::Line("look".into()),
        ]
    );
}

#[test]
fn a_resting_pileup_yields_one_prompt_per_redraw() {
    let ev = parse_all("[HP=16 (Resting) ]:[HP=17 (Resting) ]:exp\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 16, mana: None, status: Some(Status::Resting) },
            Event::Prompt { hp: 17, mana: None, status: Some(Status::Resting) },
            Event::Line("exp".into()),
        ]
    );
}

#[test]
fn a_pool_pileup_keeps_each_status_with_its_own_prompt() {
    let ev = parse_all("[HP=36/MA=12]: (Resting) [HP=37/MA=12]: (Resting) exp\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 36, mana: Some(12), status: Some(Status::Resting) },
            Event::Prompt { hp: 37, mana: Some(12), status: Some(Status::Resting) },
            Event::Line("exp".into()),
        ]
    );
}

#[test]
fn a_dangling_resting_prompt_emits_at_once() {
    let mut p = Parser::new();
    let ev = p.push("\r\n[HP=47 (Resting) ]:");
    assert_eq!(
        ev,
        vec![Event::Prompt { hp: 47, mana: None, status: Some(Status::Resting) }]
    );
    assert!(p.finish().is_empty());
}

#[test]
fn a_dangling_pool_prompt_with_its_full_status_emits_at_once() {
    let mut p = Parser::new();
    let ev = p.push("\r\n[HP=36/MA=12]: (Resting) ");
    assert_eq!(
        ev,
        vec![Event::Prompt { hp: 36, mana: Some(12), status: Some(Status::Resting) }]
    );
    assert!(p.finish().is_empty());
}

#[test]
fn a_dangling_pool_prompt_waits_for_a_half_arrived_status() {
    // The status word arrives without its trailing space. Holding the
    // prompt until the line completes is right: emitting it bare would
    // lose the status, and the echo behind it would read decorated.
    let mut p = Parser::new();
    assert!(p.push("\r\n[HP=36/MA=12]: (Resting)").is_empty());
    let ev = p.push(" look\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 36, mana: Some(12), status: Some(Status::Resting) },
            Event::Line("look".into()),
        ]
    );
}

#[test]
fn a_dangling_bare_pool_prompt_still_emits_at_once() {
    // Nothing follows "]:" yet. The prompt must not wait for a status
    // that may never come: the heal gate and Stat retirement key on it.
    let mut p = Parser::new();
    let ev = p.push("\r\n[HP=36/MA=12]:");
    assert_eq!(ev, vec![Event::Prompt { hp: 36, mana: Some(12), status: None }]);
}

#[test]
fn meditating_and_negative_hp_parse_in_both_templates() {
    assert_eq!(
        parse_all("[HP=-5 (Meditating) ]:"),
        vec![Event::Prompt { hp: -5, mana: None, status: Some(Status::Meditating) }]
    );
    assert_eq!(
        parse_all("[HP=20/KAI=4]: (Meditating) "),
        vec![Event::Prompt { hp: 20, mana: Some(4), status: Some(Status::Meditating) }]
    );
}

#[test]
fn an_unknown_status_word_never_hides_the_prompt() {
    let ev = parse_all("[HP=30 (Stunned) ]:look\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 30, mana: None, status: Some(Status::Other("Stunned".into())) },
            Event::Line("look".into()),
        ]
    );
}

#[test]
fn a_status_is_never_read_off_the_line_after_a_bare_hp_prompt() {
    // Only the pool template paints after "]:". An HP-only prompt
    // followed by a decorated line leaves the line alone.
    let ev = parse_all("[HP=30]: (Resting) look\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 30, mana: None, status: None },
            Event::Line("(Resting) look".into()),
        ]
    );
}
```

- [ ] **Step 2: Write the failing end-to-end test**

Append to `crates/mud-client/tests/correlate.rs`. Add these imports at the top of the file beside the existing ones:

```rust
use mud_client::parse::Parser;
use mud_core::text::color;
```

```rust
// ------------------------------------------------ through the parser

#[test]
fn a_look_echoed_behind_a_resting_prompt_is_answered_by_its_block() {
    // test.raw 1788503928, 2026-09-04: /farm sent `look` while resting
    // and died with "no room block came back". The block came back.
    // The prompt in front of the echo read `[HP=42 (Resting) ]:`, the
    // parser did not recognise it, the echo never matched, and the
    // block was unattributed.
    let mut p = Parser::new();
    let mut c = Correlator::new(TTL);
    let t = Instant::now();
    c.sent(CmdId(1), "look", t);
    let raw = format!(
        "\x1b[79D\x1b[K\x1b[0;37m[HP=42\x1b[0;37m (Resting) ]:look\r\n\
         {}Dark Cave\x1b[0m\r\n\
         \x1b[0;37m    This appears to be a natural cave.\x1b[0m\r\n\
         \x1b[0;32mObvious exits: west, southeast\x1b[0m\r\n",
        color::ROOM_NAME
    );
    let mut answered = None;
    for ev in p.push(&raw) {
        let cor = c.on_event(ev, t);
        if matches!(cor.event, Event::RoomSeen(_)) {
            answered = cor.answers;
        }
    }
    assert_eq!(answered, Some(CmdId(1)));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test parse status 2>&1 | tail -20; cargo test -p mud-client --test correlate behind_a_resting_prompt 2>&1 | tail -5`
Expected: the tests with a status in the frame or after it fail. `a_dangling_bare_pool_prompt_still_emits_at_once` and `a_status_is_never_read_off_the_line_after_a_bare_hp_prompt` may already pass. The correlate test fails with `answered == None`.

- [ ] **Step 4: Replace the prompt regex with a prompt reader**

In `crates/mud-client/src/parse.rs`, change the import line to bring in `Status`:

```rust
use crate::events::{Actor, Event, RoomView, Status};
```

Replace `PROMPT_RE` and its comment with:

```rust
// The board has two prompt templates (WCCMMUD.DLL):
//
//   [HP=%s%d%s%s]:             HP only, status INSIDE the frame
//   [HP=%s%d%s/%s=%s%d%s]:%s   with a pool, status AFTER the frame
//
// The status slot is ` (Resting) ` or ` (Meditating) `, spaces included.
// Unanchored: the board redraws the prompt mid-line (rest ticks, typing
// echo interleaved with async regen).
static PROMPT_FRAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[HP=(-?\d+)(?:/(?:MA|KAI)=(-?\d+))?(?: \(([^)]+)\) )?\]:").unwrap()
});
// The pool template's trailing status, matched at the start of whatever
// follows "]:". The trailing space is part of the DLL's slot, so a
// half-arrived word without it does not match and the prompt is held
// until the line completes.
static TRAILING_STATUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ \(([^)]+)\) ").unwrap());

/// One prompt found in a stripped line: the byte range it occupies,
/// status included, and the event it becomes.
struct FoundPrompt {
    start: usize,
    end: usize,
    event: Event,
}

/// The first prompt in `text`, whichever template painted it.
fn find_prompt(text: &str) -> Option<FoundPrompt> {
    let c = PROMPT_FRAME_RE.captures(text)?;
    let m = c.get(0).unwrap();
    let hp = c[1].parse().unwrap();
    let mana = c.get(2).map(|m| m.as_str().parse().unwrap());
    let mut status = c.get(3).map(|m| Status::from_word(m.as_str()));
    let mut end = m.end();
    // Only the pool template paints after the frame.
    if mana.is_some()
        && status.is_none()
        && let Some(t) = TRAILING_STATUS_RE.captures(&text[end..])
    {
        status = Some(Status::from_word(&t[1]));
        end += t.get(0).unwrap().end();
    }
    Some(FoundPrompt {
        start: m.start(),
        end,
        event: Event::Prompt { hp, mana, status },
    })
}
```

Delete `fn prompt_event`.

In `Parser::push`, replace the end of buffer block:

```rust
        // A prompt arrives with no newline; emit it the moment the
        // pending partial is exactly one-or-more complete prompts.
        if !self.buf.is_empty() {
            let cleaned = resolve_backspaces(&strip_ansi(&self.buf));
            let mut rest = cleaned.as_str();
            let mut pending = Vec::new();
            while let Some(found) = find_prompt(rest) {
                if found.start != 0 {
                    break;
                }
                pending.push(found.event);
                rest = &rest[found.end..];
            }
            if rest.is_empty() && !pending.is_empty() {
                events.append(&mut pending);
                self.buf.clear();
            }
        }
```

In `handle_line`, replace the prompt loop:

```rust
        // Prompts can appear anywhere in a physical line (mid-line
        // redraws); classify the segments between them in order.
        while let Some(found) = find_prompt(rest) {
            let before = &rest[..found.start];
            if !before.is_empty() {
                self.classify(before, opening, &sgr, events);
            }
            events.push(found.event);
            rest = &rest[found.end..];
            // The opening color applied to the first segment only.
            opening = None;
            saw_prompt = true;
        }
```

The rest of `handle_line`, including the `rfind("]:")` colour recovery, is unchanged.

- [ ] **Step 5: Run the parser and correlator suites**

Run: `cargo test -p mud-client --test parse 2>&1 | tail -5; cargo test -p mud-client --test correlate 2>&1 | tail -5`
Expected: all pass, including every test that existed before this task.

- [ ] **Step 6: Run the whole client suite**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked" | head -40`
Expected: every suite passes.

- [ ] **Step 7: Mutation check**

Temporarily change `if mana.is_some()` in `find_prompt` to `if true`. Run `cargo test -p mud-client --test parse bare_hp_prompt`. Expected: `a_status_is_never_read_off_the_line_after_a_bare_hp_prompt` fails. Revert the change and confirm it passes again.

- [ ] **Step 8: Commit**

```bash
git add crates/mud-client/src/parse.rs crates/mud-client/tests/parse.rs crates/mud-client/tests/correlate.rs
git commit -m "fix(client): the parser reads a resting or meditating prompt in both templates"
```

---

### Task 3: The game state tracks the status

**Files:**
- Modify: `crates/mud-client/src/session.rs:85-89` (`GameState`), `:1028-1036` (`apply_event`)
- Modify: every `GameState { .. }` literal in `crates/mud-client/tests` (the compiler lists them)
- Test: `crates/mud-client/tests/session_correlate.rs`

**Interfaces:**
- Consumes: `Status`, `Event::Prompt { status, .. }`.
- Produces: `GameState { hp: i32, mana: Option<i32>, room: Option<RoomView>, status: Option<Status> }`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/session_correlate.rs`. Add to the imports:

```rust
use mud_client::events::Status;
use tokio::net::TcpListener;
```

```rust
/// A board that paints a resting prompt on connect, then a bare one.
async fn resting_board() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n[HP=30 (Resting) ]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\n[HP=31]:").await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    addr
}

/// Wait until the game state satisfies `done`, or fail after five seconds.
async fn state_reaches(
    state: &mut tokio::sync::watch::Receiver<mud_client::session::GameState>,
    what: &str,
    mut done: impl FnMut(&mud_client::session::GameState) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if done(&state.borrow()) {
            return;
        }
        tokio::time::timeout_at(deadline, state.changed())
            .await
            .unwrap_or_else(|_| panic!("state never reached: {what}"))
            .expect("session closed");
    }
}

#[tokio::test]
async fn the_game_state_follows_the_prompt_status() {
    let addr = resting_board().await;
    let session = session_for(addr).await;
    let mut state = session.state();
    state_reaches(&mut state, "resting at 30", |s| {
        s.hp == 30 && s.status == Some(Status::Resting)
    })
    .await;
    // The bare prompt clears it. A status that stuck would keep every
    // consumer believing in a rest that ended.
    state_reaches(&mut state, "standing at 31", |s| s.hp == 31 && s.status.is_none()).await;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test session_correlate follows_the_prompt_status`
Expected: compile error, no field `status` on `GameState`.

- [ ] **Step 3: Add the field and fold it**

In `crates/mud-client/src/session.rs`:

```rust
/// Rolling view of the character, fed from parsed events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameState {
    pub hp: i32,
    pub mana: Option<i32>,
    pub room: Option<RoomView>,
    /// The word the last prompt painted: resting, meditating, or
    /// nothing. Tracked on every prompt, because the board has no
    /// wording for the end of a rest. The prompt is the only signal.
    pub status: Option<crate::events::Status>,
}
```

```rust
fn apply_event(state: &mut GameState, cor: &Correlated) -> bool {
    match &cor.event {
        Event::Prompt { hp, mana, status } => {
            let changed =
                state.hp != *hp || state.mana != *mana || state.status != *status;
            state.hp = *hp;
            state.mana = *mana;
            state.status = status.clone();
            changed
        }
```

Then build. The compiler names every `GameState { .. }` literal in the tests that lacks the field. Add `status: None,` to each.

- [ ] **Step 4: Run the test and the suite**

Run: `cargo test -p mud-client --test session_correlate 2>&1 | tail -5; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/session.rs crates/mud-client/tests
git commit -m "feat(client): the game state tracks the prompt status"
```

---

### Task 4: The status bar shows the status

**Files:**
- Modify: `crates/mud-client/src/tui.rs:1060-1063` (`render_status`, after the vitals)
- Modify: `docs/mud-client.md:491-505` (the status bar section)
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `GameState.status`, `Status::word`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/tui.rs`, adding `use mud_client::events::Status;` to the imports:

```rust
#[test]
fn status_line_shows_the_prompt_status_after_the_vitals() {
    let state = GameState {
        hp: 42,
        mana: Some(12),
        room: None,
        status: Some(Status::Resting),
    };
    let s = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(s.contains("HP 42 MA 12 (Resting)"), "{s}");

    let bare = GameState { status: None, ..state };
    let s = render_status(&bare, "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(!s.contains("("), "{s}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test tui shows_the_prompt_status`
Expected: FAIL, the line does not contain `(Resting)`.

- [ ] **Step 3: Render it**

In `render_status`, after the mana segment:

```rust
    s.push_str(&format!("HP {}", state.hp));
    if let Some(ma) = state.mana {
        s.push_str(&format!(" MA {ma}"));
    }
    if let Some(status) = &state.status {
        s.push_str(&format!(" ({})", status.word()));
    }
```

- [ ] **Step 4: Run the tui suite**

Run: `cargo test -p mud-client --test tui 2>&1 | tail -5`
Expected: all pass.

- [ ] **Step 5: Document it**

In `docs/mud-client.md`, in "The status bar" section, after the second example block and its paragraph about position tracking, add:

```markdown
While the character rests or meditates the board paints a word into its
prompt, and the bar repeats it after the vitals:

```
 HP 42 MA 12 (Resting) | Dark Cave [1/2160] | mbbs
```

The board has no wording for the end of a rest. The prompt is the only
signal, so the word is tracked on every prompt and disappears the moment
the board stops painting it.
```

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/tests/tui.rs docs/mud-client.md
git commit -m "feat(client): the status bar shows resting and meditating"
```

---

### Task 5: The vitals probe fills max mana

**Files:**
- Modify: `crates/mud-client/src/farm.rs:1168-1190` (`parse_health`, `discover_max_hp`)
- Modify: `crates/mud-client/src/farm.rs:1674-1682` and `crates/mud-client/src/go.rs:275-281` (the probe call sites)
- Modify: `crates/mud-client/src/bot.rs:189` (`BotConfig.max_hp`, add `max_mana` beside it) and the `Default` at `:280`
- Modify: `crates/mud-client/tests/live_server.rs:305-325` (rename the probe)
- Modify: `docs/mud-client.md:61` (the `max_hp omitted` comment in the profile example)
- Test: `crates/mud-client/tests/farm.rs:564-615`

**Interfaces:**
- Produces: `pub fn parse_mana(text: &str) -> Option<(i32, i32)>` in `mud_client::farm`.
- Produces: `pub struct Vitals { pub max_hp: i32, pub max_mana: i32 }` and `pub async fn discover_vitals(session: &Session) -> Option<Vitals>` in `mud_client::farm`. `discover_max_hp` is removed.
- Produces: `BotConfig.max_mana: i32`, 0 meaning no pool or unknown.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/farm.rs`, adding `parse_mana` to the `use mud_client::farm::{...}` list:

```rust
// parse_mana: where BotConfig.max_mana comes from. The same `health`
// line carries the pool whenever one exists, captioned Mana or Kai.
// ---------------------------------------------------------------------

#[test]
fn reads_the_mana_pool_off_the_health_line() {
    assert_eq!(
        parse_mana("Health:    29/29    [100%]  Mana:   8/18  [44%]"),
        Some((8, 18))
    );
    assert_eq!(
        parse_mana("Health:    28/31    [90%]  Kai:   0/1   [0%]"),
        Some((0, 1))
    );
}

#[test]
fn a_health_line_without_a_pool_has_no_mana() {
    assert_eq!(parse_mana("Health:    35/35    [100%]"), None);
    assert_eq!(parse_mana(""), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test farm mana 2>&1 | tail -5`
Expected: compile error, `parse_mana` not found.

- [ ] **Step 3: Add the parser, the struct, and the probe**

In `crates/mud-client/src/farm.rs`, after `parse_health`:

```rust
/// The pool half of the board's `health` line: `Mana:   8/18  [44%]`
/// or `Kai:   0/1   [0%]`. Absent when the character has no pool.
pub fn parse_mana(text: &str) -> Option<(i32, i32)> {
    static MANA_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:Mana|Kai):\s*(\d+)/(\d+)").unwrap()
    });
    let c = MANA_RE.captures(text)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?))
}

/// The maxima the board reports for the character. `max_mana` is 0
/// when there is no pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vitals {
    pub max_hp: i32,
    pub max_mana: i32,
}
```

Replace `discover_max_hp` with:

```rust
/// Ask the board for the character's maximum HP and mana.
///
/// [`crate::bot::BotConfig::max_hp`] and `max_mana` scale every percent
/// policy the bot has, and a wrong value mis-scales them silently while
/// `0` disables them outright. Nothing else in the client parses a
/// maximum. The prompt only carries the current values, so the runner
/// asks rather than trusting a number typed into a profile.
pub async fn discover_vitals(session: &crate::session::Session) -> Option<Vitals> {
    let mark = session.mark();
    session.send("health");
    session
        .expect("Health:", std::time::Duration::from_secs(15))
        .await
        .ok()?;
    let text = session.since(mark);
    let (_, max_hp) = parse_health(&text)?;
    let max_mana = parse_mana(&text).map(|(_, max)| max).unwrap_or(0);
    Some(Vitals { max_hp, max_mana })
}
```

In `crates/mud-client/src/bot.rs`, beside `max_hp`:

```rust
    /// Character max HP; 0 = unknown, disables percent policies.
    pub max_hp: i32,
    /// Character max mana or kai; 0 = no pool or unknown. Probed with
    /// `max_hp`, never typed.
    pub max_mana: i32,
```

Copy whatever serde attribute `max_hp` carries onto `max_mana`, and add `max_mana: 0,` to `Default`.

At both call sites, `crates/mud-client/src/farm.rs` in `run_farm` and `crates/mud-client/src/go.rs` in `run_go`:

```rust
    // Every percent policy divides by these, and a wrong value mis-scales
    // heal and flee silently. 0 means the profile did not say, so ask.
    let mut bot_config = bot_config.clone();
    if (bot_config.max_hp == 0 || bot_config.max_mana == 0)
        && let Some(vitals) = crate::farm::discover_vitals(session).await
    {
        bot_config.max_hp = vitals.max_hp;
        bot_config.max_mana = vitals.max_mana;
    }
```

In `crates/mud-client/tests/live_server.rs`, rename the test to `discover_vitals_asks_the_board` and change the body to call `discover_vitals`, asserting `vitals.max_hp > 0` and `vitals.max_mana >= 0`. Keep everything else.

In `docs/mud-client.md` line 61, change the comment to `# max_hp and max_mana omitted: the runner probes the board for them at startup`.

- [ ] **Step 4: Build and run the suites**

Run: `cargo build --workspace 2>&1 | grep -E "^(error|warning)"; cargo test -p mud-client --test farm 2>&1 | tail -3; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: no errors, all pass. `grep -rn discover_max_hp crates` returns nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client docs/mud-client.md
git commit -m "feat(client): the vitals probe reads max mana beside max hp"
```

---

### Task 6: Comments and docs catch up with the templates

**Files:**
- Modify: `crates/mud-client/src/correlate.rs:101-104` (the `strip_decoration` doc)
- Modify: `crates/mud-client/src/bot.rs:367-384` (the `strip_status` doc)
- Modify: `crates/mud-client/tests/correlate.rs:69-71` and `:229`
- Modify: `docs/board-correlation.md:49-54`
- Modify: `re/docs/regeneration.md:141-143` (gitignored, local only)

- [ ] **Step 1: Correct the correlator doc**

Replace the `strip_decoration` doc comment in `crates/mud-client/src/correlate.rs`:

```rust
/// Strip a leading parenthesized status decoration: `(Resting) look` ->
/// `look`.
///
/// The pool prompt template paints its status AFTER the frame, `[HP=36/
/// MA=12]: (Resting) look`, and the parser now reads it off the prompt.
/// This strip stays as defence: a chunk boundary between `]:` and the
/// word leaves the word on the echo, and the echo must still match.
```

- [ ] **Step 2: Correct the bot doc**

Replace the paragraph of the `strip_status` doc in `crates/mud-client/src/bot.rs` that begins "DEFENCE IN DEPTH" through "this does not address that." with:

```rust
/// DEFENCE IN DEPTH. ` (Resting) ` is the status the board paints into
/// its prompt, and the pool template paints it AFTER the frame, so it
/// once rode into an `ActorEntered` name as "+  (Resting) fierce
/// filthbug". The parser now reads it off the prompt, and this strip
/// stays for the chunk boundary case that can still leave it on a line.
```

Keep the sentence about the case test.

- [ ] **Step 3: Correct the test comments**

In `crates/mud-client/tests/correlate.rs`, replace the comment at line 69 with:

```rust
    // The pool prompt template paints "(Resting) " after "]:", so a
    // chunk split there leaves it on the echo. Any single leading
    // parenthesized run strips.
```

And the comment on the `"(Resting)"` entry at line 229 with `// a status word cut off a split pool prompt: empty after decoration strip`.

- [ ] **Step 4: Document the templates**

In `docs/board-correlation.md`, after item 1 of "Why the client cannot tell", add a short block:

```markdown
   The prompt has two templates. HP only paints its status inside the
   frame, `[HP=42 (Resting) ]:`. With a pool it paints the status after
   the frame, glued to the echo, `[HP=36/MA=12]: (Resting) look`. The
   parser reads both and the echo reaches the correlator clean.
```

- [ ] **Step 5: Correct the spec locally**

In `re/docs/regeneration.md` section 5, replace the heading and its first sentence with:

```markdown
## 5. Rest and meditate are real; near-death recovery is separate

The board has a player rest mode and a meditate mode. `rest` prints `You
are now resting.` and paints ` (Resting) ` into the prompt; `meditate`
prints `You are now meditating.` and paints ` (Meditating) `. Nothing
prints when either ends. Most commands end them. Measured 2026-09-04:
`look`, `exp`, `health` and `help` do not; `hide` and any move do. The
earlier claim here that there is no player rest mode was wrong.
```

This file is gitignored and is not committed.

- [ ] **Step 6: Build, test, commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: all pass.

```bash
git add crates/mud-client/src/correlate.rs crates/mud-client/src/bot.rs crates/mud-client/tests/correlate.rs docs/board-correlation.md
git commit -m "doc: the prompt's two templates and where the status word comes from"
```

---

### Task 7: Verify against the capture and ship the binary

**Files:**
- None modified. This task gates the phase.

- [ ] **Step 1: Whole workspace**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: every line reads `ok`, zero `FAILED`.

- [ ] **Step 2: Replay the real capture**

Run: `cargo run -p mud-client --example replay_assist -- test ~/.config/mmc/test.toml 2>&1 | grep -n "Resting\|RoomSeen" | head -40`

Expected: prompts after the `rest` at `1788503773` print with `status: Some(Resting)`, and the `RoomSeen` for `Dark Cave` that follows the `look` at `1788503928` prints with `answers: Some(..)`, not `None`. If `replay_assist` does not print attribution, add a one-line `eprintln!` of `cor.answers` beside its existing event print for this run and remove it afterwards.

- [ ] **Step 3: Build the release binary**

Run: `cargo build --release -p mud-client 2>&1 | tail -2; ls -l ~/.local/bin/mmc; readlink -f ~/.local/bin/mmc`
Expected: a fresh `target/release/mmc` and the symlink pointing at it. The live session must be restarted to pick it up.

- [ ] **Step 4: Report**

State in the final report: the commits made, the replay evidence, and that the running `mmc` predates the fix until restarted.
