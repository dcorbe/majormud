# `mmc audit` — dialect-divergence report for foreign-board captures

## Context

`mmc` (crates/mud-client) encodes stock MajorMUD 1.11p-WG wordings in three
load-bearing layers:

1. **Correlator reply grammar** — `correlate.rs` `completes()`/`confirms()`:
   a closed, stock-derived set of reply wordings per command kind. A wording
   gap leaves a sent command dangling at the FIFO head, where it **steals the
   next room block** from whoever really asked — position desync, step
   timeouts, dead runs.
2. **Parser classification** — `parse.rs`: regex families (MOB_ENTER_RE etc.),
   color anchors (`color::ROOM_NAME` = `\x1b[1;36m`, `YOUR_MISS` = `0;36`),
   prompt/redraw splitting in `handle_line`. A gap means missed events —
   invisible mob entries, unnoticed whiffs — and a blind bot.
3. **Nav wordings** — `nav.rs` constants (DOOR_BLOCKED, DOOR_YIELDED,
   BASH_FAILED, NO_SUCH_EXIT...). A gap makes doors "give up" or steps
   misread.

On 2026-08-01 the user started playing **mud.cwgaming.com:2323** (profile
`~/.config/mmc/pootwaddle.toml`) — a **standalone MajorMUD reimplementation**
(same kind of project as this repo's own server; it is NOT the stock DLL, the
user has confirmed this). Three divergences surfaced in one evening, each
breaking a different layer, each costing hours of archaeology and minutes to
fix once a capture existed:

- `"The door is closed."` (period; stock prints a bang) — correlator gap; the
  operator's hand-typed `n` dangled and stole the navigator's arrival block;
  the walk timed out a room behind itself. Fixed `b26afe5` + regression test
  (`tests/correlate.rs::a_door_refusal_with_a_period_retires_the_move`).
- Bare-`\r` + `ESC[2K` prompt-overwrite redraws (stock uses newlines) — the
  redrawn text reached `classify()` with a leading CR and every `^`-anchored
  rule missed; **zero `ActorEntered` events ever fired on that board**. Fixed
  `19ba567` (classify trims leading CRs) + test
  (`tests/parse.rs::a_line_redrawn_over_the_prompt_with_a_bare_cr_still_classifies`).
- `"creeps in the room from nowhere"` (stock: "into") — already covered by
  MOB_ENTER_RE's alternation; no fix needed.

**The goal:** stop doing archaeology. Build `mmc audit <capture-base>`, which
replays a capture through the real client stack and prints a divergence
report. Every future cwgaming (or any foreign board) mishap becomes a
five-minute report; each confirmed divergence is then folded into the one
grammar with a capture-cited test — exactly the pattern of the three fixes
above. Explicitly REJECTED alternatives: a per-board dialect fork (three
layers of duplicated wordings for what is so far three one-line variants —
revisit only if a *structural* divergence appears, e.g. different block or
prompt shape) and global fuzzy matching (erodes the closed grammar that
attribution correctness rests on; wrong-attribution is worse than missed).

The audit doubles as a conformance checker for this repo's own mud-server:
run it over any client capture against the local board and expect zero
findings.

## Capture format (both files written together by `Session::connect`)

- `<base>.raw` — **byte-faithful RX** (telnet/ANSI/CP437 intact). This is
  wire truth for the parser. No TX.
- `<base>_timing.log` — `EPOCH.mmm TAG payload` lines; TAG ∈ `!!` (session
  marks), `RX` (stripped text lines), `TX` (sent commands; written at
  session.rs:273). This holds the TX/RX **interleave order** the correlator
  needs, but its RX is post-strip — no SGR, so it cannot drive the parser's
  color-keyed rules by itself.
- Existing captures to run against: `~/bbs/cwrun1.raw`, `~/bbs/cwrun2.raw`
  (+ `_timing.log`). **These contain the account password in TX lines and
  are gitignored — never commit them, never quote TX login lines in
  reports.** Negative control: `re/oracle/*.raw` (120 stock-board captures;
  many lack timing logs — parser-level audit only for those).

## Design

New module `crates/mud-client/src/audit.rs` (pure: inputs in, `Report` out)
plus an `Audit { capture: PathBuf }` subcommand in `src/bin/mmc.rs` (clap
shape mirrors Play/Farm; takes the capture **base** path or either file and
derives the sibling).

### Replay pipeline

1. **Events with fidelity**: `.raw` → `wire::TelnetFilter` →
   `wire::cp437_to_string` → `parse::Parser` — copy the recipe from
   `tests/bot_corpus.rs::corpus_events` (lines 34-42). Keep per-event the
   source line text.
2. **Interleave**: parse `_timing.log` into an ordered list of
   `Tx(String)` / `Rx(String)` records. Align the parser's emitted
   line-events (stripped text) against the timing log's RX lines in order
   (they are the same lines, written from the same stream; alignment is a
   forward scan with skip tolerance for lines the timing log renders
   differently, e.g. multi-segment redraws). Inject each `Tx` into the event
   stream at its aligned position.
3. **Correlation**: drive `correlate::Correlator` exactly as the session
   does: `sent(CmdId(n), cmd, t)` for each TX, then feed each parsed event
   and record what it answers. Use synthetic increasing `Instant`s (the
   audit does not sleep; TTL expiry is simulated by feeding the log's
   timestamps if the Correlator API takes `t: Instant` — it does, see
   `tests/correlate.rs` `ans(&mut c, ev, t)` usage).

### Report sections

1. **Grammar gaps** (correlator layer): every TX entry retired by TTL/cap
   rather than by a grammar wording. Print the command, and the RX lines
   that arrived between it and its retirement (the softened answer is
   almost always in that window). This is precisely how `"The door is
   closed."` would have been caught in seconds.
2. **Suspicious unclassified lines** (parser layer): events that fell
   through to bare `Event::Line` but match suspicion heuristics:
   - contains `" the room from"`, `" in after you"`, `" at you"`,
     `"drop to the ground"`, `"You notice "`, `"Also here"`, `"Obvious
     exits"`, door/gate/bash/exit wordings — event-shaped text that did
     not classify;
   - leading control characters or unknown opening-SGR shapes on lines
     that otherwise match the above (the CR-redraw class).
   Deduplicate identical lines; print each with a count.
3. **Attribution anomalies**: a `RoomSeen` claimed by a command while a
   *younger* block-producing command (Move/Look/Bash kinds) was still
   pending — the steal signature — and any TX that outlived N (say 3)
   younger completions.
4. **Summary**: counts per section; process exit code 1 when any section is
   non-empty (so the audit can run in CI over `re/oracle` as a
   zero-divergence pin).

Output: plain text to stdout, mmc's existing eprintln/println conventions.
No JSON (YAGNI until something consumes it).

## Review findings (2026-08-01, before implementation)

Three corrections to the design above. The first blocks section 1, the
second is a disclosure bug, the third removes the riskiest part.

1. **`correlate.rs` cannot be "reused as-is".** Section 1 wants "entries
   retired by TTL/cap rather than by a grammar wording", and that is not
   observable from outside the `Correlator`: `expire()` silently
   `retain()`s, and `Correlated::answers` returns `Some(id)` identically
   for echo acceptance, the DUPLICATE execution echo, a confirm, and a
   grammar completion. "Saw the id twice ⇒ it completed" is therefore
   wrong on exactly the busy, double-echoing board the tool exists for.
   Two small additive changes are required: a drain for expired entries,
   and a way to tell `Kind::Opaque` from modelled kinds (`Kind` and
   `kind_of` are private; only `is_movement` is exported). Without the
   second, every `inventory`/`health`/attack TX lands in the grammar-gap
   section as a false positive, because Opaque commands are DESIGNED to
   expire. Note also that `expire()` runs only from `sent()` and
   `on_event()`, so a capture's trailing commands never expire — the
   replay needs a final drain.

2. **The report as specified prints the account password.** `login`
   sends it via `session.send()`, and session.rs writes every `Cmd::Line`
   to the timing log as `TX`; it is an Opaque command, so it is never
   retired by grammar and lands at the TOP of section 1 on the first real
   run. The prose rule ("never quote TX login lines") has to be a code
   rule: suppress Opaque from section 1 (see 1), and redact TX before the
   first in-game prompt.

3. **The alignment does not need to be heuristic.** "Forward scan with
   skip tolerance" is the riskiest part of the plan and everything in
   sections 1 and 3 rests on it. The timing log's RX lines are written
   from `AnsiStripper` output (session.rs), so the audit can run the same
   `TelnetFilter → AnsiStripper` alongside its parser and COUNT lines:
   TX record *k* sits after RX line *m*, so inject it once the stripper
   has emitted its *m*-th line. Exact, and it reuses the code path that
   wrote the log. (Room blocks still flush late because the parser
   buffers to the exits line — inherent, and it matches what the live
   correlator sees.) Minor: an `Instant` cannot be built from an epoch
   stamp; use `base + Duration::from_secs_f64(t - t0)`.

**Suggested sequencing.** Section 2 needs only the `.raw` — no timing
log, no alignment, no correlator change — and would have caught two of
the three divergences that motivated this plan (the CR-redraw, which
fired zero `ActorEntered` all evening, and the "creeps in" wording). So
ship **Stage A** = section 2 + the `re/oracle` zero-findings pin + the
subcommand, and gate **Stage B** = alignment, correlator observability,
sections 1 and 3 on Stage A actually surfacing something. Also note step
5's "assert the cwrun captures by hand" cannot become a committed test:
those captures are gitignored and hold the password.

**Overlap with the room model.** `world::Reconcile` (commit `1fc7f0d`)
now finds the same class of parser gap from LIVE play rather than from
captures: an occupant the board lists that the model never heard about
is a movemsg wording we cannot read. It is not a substitute — it says
nothing about the correlator layer, which is sections 1 and 3 — but it
lowers the urgency of section 2.

### Heuristic tuning rule

The suspicion heuristics MUST report zero findings across the stock corpus
(`re/oracle/*.raw`) — that is the false-positive pin, enforced by test. Any
heuristic that fires on stock captures gets narrowed, not shipped noisy.

## Implementation steps (TDD, in order)

1. **`audit.rs` skeleton + timing-log parser.** RED: unit tests for
   `TimingLog::parse` on synthetic text covering `!!`/`RX`/`TX`, the
   `EPOCH.mmm` stamps, and payload-with-spaces. GREEN: the parser.
2. **Alignment + correlator replay.** RED: a synthetic capture pair (raw
   bytes + timing lines built in-test, `nav_echo`-style wordings) where one
   TX (`n`) is answered by a softened refusal the grammar does not know
   (invent one, e.g. `"That way is shut."`) — assert the report lists it
   under grammar gaps with the refusal line in its window; and a well-formed
   exchange asserts an EMPTY report. GREEN: alignment + replay + section 1.
3. **Suspicious-line scan.** RED: synthetic stream with an entry-shaped
   line that misses the regex (e.g. `"A gruntling blorps in the room from
   nowhere."` fails because... pick a real gap: a leading `\x07` bell) →
   reported; a clean stock-shaped stream → empty. GREEN: section 2.
4. **Attribution anomalies.** RED: replay the exact `b26afe5` incident
   shape (typed n → unretired → block claimed by it while the walk's n was
   pending) with the softened wording REMOVED from a test-local grammar…
   NOTE: the real grammar now retires it; simulate the steal instead with a
   genuinely unknown refusal so the dangling entry claims the next block.
   Assert section 3 flags it. GREEN: section 3.
5. **Corpus pin.** Test iterating a handful of `re/oracle` raws (parser
   sections only where no timing log) asserting zero findings; plus the two
   `cwrun` captures asserted BY HAND once (run the binary, triage, and
   record the expected residue as of today — likely empty after the three
   fixes; if not, each finding is the next fix's evidence).
6. **`mmc.rs` subcommand** wiring + `--help` text; a `cli.rs` test for
   argument parsing if the existing `tests/cli.rs` covers other subcommands
   (it does — follow its pattern).
7. **Run it for real**: `mmc audit ~/bbs/cwrun2.raw` and over 2-3 oracle
   captures. Fold any real findings into grammar/parser via the established
   per-divergence pattern (capture-cited test first, then the variant).

## Key files

| File | Role |
|---|---|
| `crates/mud-client/src/audit.rs` | new — replay, alignment, report |
| `crates/mud-client/src/bin/mmc.rs` | new `Audit` subcommand (clap enum at top) |
| `crates/mud-client/src/lib.rs` | export `pub mod audit` |
| `crates/mud-client/src/correlate.rs` | reused as-is (`Correlator`, `CmdId`, `Correlated`) |
| `crates/mud-client/src/parse.rs`, `src/wire.rs` | reused as-is (recipe: `tests/bot_corpus.rs:34-42`) |
| `crates/mud-client/src/session.rs:60-300` | capture-format ground truth (TX at :273) |
| `crates/mud-client/tests/audit.rs` | new — the TDD tests above |

## Verification

- `cargo test -p mud-client` — all suites stay green (32 today; corpus pins
  untouched: the audit only READS).
- `cargo run -p mud-client --bin mmc -- audit ~/bbs/cwrun2.raw` → humane
  report; today's three fixed divergences must NOT appear (they retire/
  classify now); anything else listed is real work, one capture-cited fix
  each.
- `... -- audit ~/mmc-probe/run7.raw` (stock local board, no timing log) →
  zero findings, parser sections only.
- `cargo clippy -p mud-client`; `rustfmt --edition 2024` on the NEW files
  only (repo baseline is not fmt-clean; convention is new-files-clean).

## Out of scope

- Per-board dialect layer (`target = "cwgaming"` wording tables) — hold
  until a structural divergence (block shape, prompt format) forces it.
- Auto-fixing/fuzzy matching — divergences are folded in by hand, with
  tests, one at a time.
- The farm/assist/TUI — untouched.
- Rewriting the capture format (e.g. TX markers inside `.raw`) — the
  alignment approach works on every capture that already exists.
