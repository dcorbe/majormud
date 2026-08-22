# The client knows what it carries — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development.
> Steps use checkbox (`- [ ]`) syntax. Run tasks strictly in order.

**Spec:** `docs/superpowers/specs/2026-08-22-inventory-and-backstab-design.md`

**Goal:** An equipment model the client can trust, an inventory it refreshes, an
item-identity join it never guesses at, and a backstab opener that uses them.

## Global Constraints

- **Targeted test binaries only.** `cargo test -p mud-client --test <name>`. Do
  NOT run the full suite. Daniel accepted that regressions may surface later.
- **Never run `cargo fmt`, `rustfmt`, or any formatter.**
- `CARGO_BUILD_JOBS=3`, foreground, one cargo call at a time. 7.5 GB machine.
- Every file ends with a blank line. Commit per task, tag-prefixed.
- **Never `git add -A` or `git add .`** — stage named files only.
- Do not connect to a live board.

## Dependencies, and what they mean for this plan

Tasks 1, 4 and 5 consume `Content.items` client-side, which arrives in
`2026-08-22-one-path-to-content.md` Task 2. **Those steps are written against a
dependency that does not exist yet and should be expected to need revision when
it lands.** If a step here contradicts what `Content` actually provides, the
plan is wrong, not the code — report it and stop rather than bending the
implementation to match a stale instruction.

Tasks 2 and 3 (equipment model, contents refresh) are pure wire work and depend
on nothing from the content track. They can run first and in parallel with it.

---

### Task 1: Item identity

**Files:** new module in `mud-client`; own test binary.
**Depends on:** content plan Task 2.

**Produces:** name → `Option<&content::Item>`, and a `bs_capable(item) -> bool`
reading ability `0x74` off the ability arrays.

- [ ] **Step 1: Write the failing tests.** A hit on a real item name; a
      near-miss that MUST return `None`; an article/plural form that should still
      resolve. Use real `Content.items` rows, not invented ones.
- [ ] **Step 2: Implement.** Normalise articles, plurals and quantities. When two
      rows are equally plausible, return `None` — ambiguity is a miss.
- [ ] **Step 3: Mutate** — make an unresolvable name return the nearest row
      instead of `None` and confirm the near-miss test fails. This is the
      defect the whole design guards against; if it does not bite, the test is
      wrong.
- [ ] **Step 4: Commit.**

---

### Task 2: The equipment model

**Files:** new module; own test binary. **Depends on:** nothing.

Equipment is authoritative because **nothing in the game force-unequips gear**
(`GAME_MECHANICS.md` §Equipment & gear, `[CONFIRMED]`). State changes only from
commands we issue.

- [ ] **Step 1:** Model the slots and the wielded weapon. Update from our own
      commands, confirmed by `You are now holding <new>.`
- [ ] **Step 2:** **The displaced item is never named on a weapon swap** — the
      board prints one line only. The client must remember what it took off.
      Write that test first: swap, then assert the client knows the previous
      weapon without ever having been told it.
- [ ] **Step 3:** A refused equip (class/level/slot) must NOT update the model.
      The client may not assume its own command succeeded.
- [ ] **Step 4: Mutate** — make the model read the displaced item from the wire
      (it isn't there) and confirm Step 2's test fails.
- [ ] **Step 5: Commit.**

---

### Task 3: Contents, refreshed and allowed to be stale

**Files:** `sheet.rs` (`Inventory`), `session.rs`; own test binary.
**Depends on:** nothing.

- [ ] **Step 1:** Keep the parsed contents on the session, refreshed from `i`.
      Contents are best-effort by design — loot, sales and consumables drift
      them. Do not attempt to track every mutation.
- [ ] **Step 2:** Preserve the existing wrap-rejoining and encumbrance parse;
      this is an extension of `sheet::Inventory`, not a replacement.
- [ ] **Step 3: Mutate** — break the wrap rejoin and confirm an existing parse
      test fails.
- [ ] **Step 4: Commit.**

---

### Task 4: The backstab decision

**Files:** new module; own test binary. **Depends on:** Tasks 1, 2.

A pure function of (wielded item, carried items, stealth state) → action.

- [ ] **Step 1: Write the table as tests first**, one case per spec row:
      dual-purpose (no swap); swap-needed; none carried (**do nothing** — the
      spec deliberately does NOT disarm to enable an unarmed backstab);
      identity unresolved (do nothing).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Mutate** — make the dual-purpose row swap anyway and confirm a
      test fails. Then make the unresolved case act and confirm a test fails.
- [ ] **Step 4: Commit.**

---

### Task 5: Wiring the opener

**Files:** `nav.rs`, combat path; own test binary. **Depends on:** Task 4.

- [ ] **Step 1: Read first.** Establish where an opening attack is issued on
      room entry today. If there is no such seam, **report `NEEDS_CONTEXT`**
      rather than inventing one.
- [ ] **Step 2:** Evaluate the decision **before** arming sneak — equipping
      breaks sneak, so the swap cannot happen after arrival.
- [ ] **Step 3:** On entry with a target and stealth believed intact, send
      `bs <target>`; after the opening round, restore the primary weapon. Firing
      `bs` when not actually stealthy is a silent plain attack and is acceptable;
      the per-move stealth re-roll means certainty is impossible.
- [ ] **Step 4:** Handle `"You cannot backstab with this weapon!"` honestly —
      it means the model was wrong about what is wielded. Log it as a
      correction, do not silently retry.
- [ ] **Step 5: Mutate** — move the swap to after the sneak and confirm an
      ordering test fails.
- [ ] **Step 6: Commit.**

## Self-Review

**Ordering:** 2 and 3 first (no dependencies), then 1, then 4, then 5.

**Known gap, stated:** the whole feature rests on name resolution, which is the
one part that can fail silently and expensively. Every other failure degrades to
a normal attack.
