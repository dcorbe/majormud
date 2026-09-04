# XP Per Hour Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The experience rate the client shows and reasons with is per hour, not per minute.

**Architecture:** `progress.rs` computes the rate once. It becomes `per_hour`, the level ETA label takes a per hour rate, and the two renderers print `xp/hr`. One task, since every site is a mechanical consequence of the first.

**Tech Stack:** Rust 2024 edition, the existing `mud-client` test crates.

**Spec:** `docs/superpowers/specs/2026-09-04-prompt-status-and-recovery-design.md`, Part 4. This plan is Phase 4 of that spec.

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with one newline.
- Commit with a tagged message. Do not mention plans or specs in commit messages. End the commit message with a blank line and `Claude-Session: https://claude.ai/code/session_0183JngSdxae1Eg4EDaso8H1`.
- Do not commit `.claude/`, `CLAUDE.md`, or anything under `.superpowers/`.
- Do not use `/tmp`.
- Prose in comments and docs: plain words, no em dashes, no parentheses as asides, no semicolons.

---

### Task 1: Per hour everywhere

**Files:**
- Modify: `crates/mud-client/src/progress.rs:176-196` (`reset` doc, `per_minute`), `:244-260` (`eta_label`)
- Modify: `crates/mud-client/src/tui.rs` (every `exp.per_minute(` call, the `exp_per_min` parameters of `redraw_bottom`, `render_status`, `repaint`, `bar_text`, and the `xp/min` label near line 1165)
- Modify: `crates/mud-client/src/bin/mmc.rs:452`
- Test: `crates/mud-client/tests/expmeter.rs:31-45`, `:95-117`, `crates/mud-client/tests/tui.rs:237-240`, `:505`

**Interfaces:**
- Produces: `ExpMeter::per_hour(&self, elapsed: Duration) -> Option<i64>` replacing `per_minute`. `eta_label(needed: i64, per_hour: Option<i64>) -> String`.

- [ ] **Step 1: Rewrite the failing tests**

In `crates/mud-client/tests/expmeter.rs`, rename `it_reports_a_rate_per_minute` to `it_reports_a_rate_per_hour`, change its doc comment to say per hour, and change its assertions. The meter observes two gains of 300, so:

```rust
    assert_eq!(m.per_hour(Duration::from_secs(120)), Some(18_000));
    assert_eq!(m.per_hour(Duration::from_secs(60)), Some(36_000));
```

In `too_early_to_say_is_not_zero`, `per_minute` becomes `per_hour` and the answer stays `None`.

Change the `eta_label` assertions to per hour rates that give the same labels:

```rust
    assert_eq!(eta_label(0, Some(30_000)), "ready");
```

```rust
    assert_eq!(eta_label(10_000, None), "?");
    assert_eq!(eta_label(10_000, Some(0)), "?");
```

```rust
    assert_eq!(eta_label(6_000, Some(6_000)), "1h0m");
    assert_eq!(eta_label(4_500, Some(6_000)), "45m");
    assert_eq!(eta_label(7_200, Some(6_000)), "1h12m");
```

```rust
    assert_eq!(eta_label(10_000_000, Some(60)), ">99h");
```

In `crates/mud-client/tests/tui.rs` near line 237, change `"255 xp/min"` to `"255 xp/hr"` and `"xp/min"` to `"xp/hr"`, and the comment near line 505 to say `xp/hr`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test expmeter 2>&1 | tail -5; cargo test -p mud-client --test tui xp 2>&1 | tail -5`
Expected: compile error, no method `per_hour`, and the label test fails on `xp/hr`.

- [ ] **Step 3: The rate**

In `crates/mud-client/src/progress.rs` replace `per_minute` with:

```rust
    /// Experience per hour over `elapsed`, or `None` when too little
    /// time has passed for the figure to mean anything, which is a
    /// better answer than a number produced by dividing by nearly zero.
    pub fn per_hour(&self, elapsed: std::time::Duration) -> Option<i64> {
        let secs = elapsed.as_secs_f64();
        if secs < 1.0 {
            return None;
        }
        Some((self.total as f64 * 3600.0 / secs).round() as i64)
    }
```

In the `reset` doc, change `[`Self::per_minute`]` to `[`Self::per_hour`]`. Replace `eta_label` with:

```rust
/// How long at the current rate, short enough for the status bar.
///
/// Honest about what it does not know: no rate yet, the first minute of
/// any run and after every death resets the meter, gives `?` rather
/// than a fabricated number, and a crawl is capped rather than printed
/// to false precision.
pub fn eta_label(needed: i64, per_hour: Option<i64>) -> String {
    if needed <= 0 {
        return "ready".to_string();
    }
    let Some(rate) = per_hour.filter(|r| *r > 0) else {
        return "?".to_string();
    };
    let mins = needed * 60 / rate;
    if mins >= 99 * 60 {
        return ">99h".to_string();
    }
    if mins >= 60 {
        format!("{}h{}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}
```

- [ ] **Step 4: The renderers**

In `crates/mud-client/src/tui.rs`, rename every `exp_per_min` parameter to `exp_per_hour`, change every `exp.per_minute(` to `exp.per_hour(`, and change the label to `" | {rate} xp/hr"`. In `crates/mud-client/src/bin/mmc.rs` change `exp.per_minute(` to `exp.per_hour(`. Then `grep -rn "per_minute\|xp/min\|per_min" crates` must return nothing.

- [ ] **Step 5: Build and run the suites**

Run: `cargo build --workspace 2>&1 | grep -E "^(error|warning)"; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: no errors, all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client
git commit -m "feat(client): the experience rate reads per hour"
```

---

### Task 2: Verify and ship

- [ ] **Step 1: Whole workspace**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: every line reads `ok`, zero `FAILED`.

- [ ] **Step 2: Release build**

Run: `cargo build --release -p mud-client 2>&1 | tail -1; readlink -f ~/.local/bin/mmc`
Expected: a fresh `target/release/mmc`, symlinked.
