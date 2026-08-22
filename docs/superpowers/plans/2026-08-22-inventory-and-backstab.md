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

### Task 5: The client learns to sneak

**Files:** `graph.rs` (`Capabilities`), `nav.rs`, `session.rs`, `correlate.rs`;
own test binary. **Depends on:** Task 4.

**Why this task exists.** Task 5 previously reported `NEEDS_CONTEXT`: `decide()`
needs `stealthy: bool` and nothing could supply it. Nothing in `mud-client` has
ever sent `sneak`. This task builds the producer.

**Policy: ALWAYS SNEAK when capable.** Daniel chose this on 2026-08-22 over a
chance-gated alternative that would have computed the §11.3 odds and armed only
above a threshold. He was told the tradeoff — at Stealth 56 roughly two openers
in three arrive seen, and a failed sneak means opening with the backstab weapon
as an ordinary attack — and chose the simple policy anyway. Do not reintroduce
gating.

**The state model is trivial, and that is a finding, not an oversight.** Sneak is
consumed by exactly one move (byte-verified 2026-08-22; see `theft.md` §11.1 and
`mud-core`'s transit re-roll). So "armed" means *we sent `sneak` since the last
move*, and it clears when we move. There is no wear-off, so there is no
silent-break detection to write. MudPlay's elaborate stealth FSM exists only
because it assumed persistence; do not copy it.

- [ ] **Step 1:** Add `stealth: u32` to `Capabilities`, filled from
      `Session::stats()`. Mirror exactly how `picklocks` was done — that landed
      in `77d36eae` and is the pattern to follow.
- [ ] **Step 2:** Classify `sneak` in the correlator. An unclassified reply is
      never attributed and the caller burns its deadline — that is how the
      locked door reported a phantom timeout (`b19f862d`). Not optional plumbing.
- [ ] **Step 3:** Send `sneak` before each nav step when `stealth > 0` and not
      engaged. Read the replies honestly:
      - `"You may not sneak right now!"` — hard block (being fought or engaged).
        No retry this step. Move anyway, unsneaked.
      - `"You don't think you're sneaking."` — the attempt failed. NOT armed.
      - a bare `"Attempting to sneak..."` with no failure line — treat as armed.
        Success is genuinely silent (`theft.md` §11.1: "the player is never told
        sneaking worked"), and failure is only *sometimes* reported, gated on a
        perception roll. So this is optimistic by necessity, not by choice.
        Say so in a comment.
- [ ] **Step 4:** Clear armed on every move. The transit re-roll means the client
      can never know it actually arrived unseen; it must not pretend otherwise.
- [ ] **Step 5: Mutate** — make a character with `stealth: 0` still send `sneak`
      and confirm a test fails; make `"You may not sneak right now!"` leave the
      client believing it is armed and confirm a test fails.
- [ ] **Step 6: Commit.**

---

### Task 6: Wiring the opener

**Files:** `nav.rs`, `bot.rs`; own test binary. **Depends on:** Task 5.

The seam is `Bot::engage` in `bot.rs`, reached from `on_event(Event::RoomSeen)`
and `on_event(Event::ActorEntered)`.

- [ ] **Step 1:** Evaluate `backstab::decide()` **before** arming sneak.
      Equipping breaks sneak, so a swap after arrival is too late — the ordering
      is decide → swap → sneak → move.
- [ ] **Step 2:** On entry with a target and the client believing itself armed,
      send `bs <target>` instead of the ordinary opener. Restore the primary
      weapon after the opening round.
- [ ] **Step 3:** Handle `"You cannot backstab with this weapon!"` — it means the
      equipment model was wrong about what is wielded. Log it as a correction.
      Do NOT silently retry.
- [ ] **Step 4:** A `bs` sent while not actually stealthy is a silent plain
      attack, which is acceptable and is why this policy is affordable. A `bs`
      with the wrong weapon costs the opening round, which is why Step 3 matters.
- [ ] **Step 5: Mutate** — move the swap to after the sneak and confirm an
      ordering test fails.
- [ ] **Step 6: Commit.**

## Self-Review

**Ordering:** 2 and 3 first (no dependencies), then 1, then 4, then 5.

**Known gap, stated:** the whole feature rests on name resolution, which is the
one part that can fail silently and expensively. Every other failure degrades to
a normal attack.
