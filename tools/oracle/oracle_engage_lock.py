#!/usr/bin/env python3
"""Slice-8 E3: WHERE is the engage/retaliation lock taken? (M7 carry 2)

The M7 slice-5 relocation moved the retaliation lock off the tail of the
player swing loop and onto the ATTACK command itself (decompile 26230
lives in the other arm of the 26112 split). Decompile-justified, never
measured. The genrdn draw order itself is not observable live, but the
lock's LOCATION is, through two independent transcript patterns:

  P2 (the sharp one): attack a passive giant rat with the sandbag hammer.
     The rat is DR 1 and the hammer 1..1, so every connect glances for 0
     damage. Under the OLD model (lock taken on the round's post-damage
     branch) zero damage means the lock roll never happens and the rat
     NEVER retaliates. Under the NEW model (lock at ATTACK time) it
     retaliates from round 1, damage or no damage.

  P1: attack and WALK out at the next flood-control slot, before the
     first round fires. A departure free-attack proves the monster was
     already engaged at command time. Interpretable only against:
  P0: enter, wait ~12 s, leave — the rat's intrinsic aggression baseline.
  P3: attack, wait one full round, walk out — a free attack is expected
     under BOTH models; validates that the free-attack observable works.

The walk-out MUST be a real direction (the free attack fires on walked
movement; /xgoto is a host-side teleport and skips it), so each trial
parses "Obvious exits:" and leaves by the first listed exit.

Trials: P0 x10, P2 x10 (3 rounds each), P1 x15, P3 x5. Heal at 2190
between trials whenever HP dips (rat bites 2..10 at attack-accuracy 10).

Usage: python3 oracle_engage_lock.py
"""
import re
import sys
import time

from mudlib import Session

RAW = "../../re/oracle/oracle_engage_lock.raw"
LOG = "../../re/oracle/oracle_engage_lock_timing.log"
HEALER = 2190
# TARGET REWORK #2 2026-07-29: rats are structurally absent, and BATS
# ARE NOCTURNAL — every bat success in this campaign landed 02:00-08:00
# and every 09:00+ sweep found none, fresh restart included. The kobold
# is up in daylight: DR 2 keeps P2's zero-damage premise, the 2..9 bite
# is survivable, and P0 measures its passivity before P2 leans on it.
TARGET, NOUN = "kobold", "kobold"
MAP = 6
RAT_ROOMS = [
    722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732, 733, 734,
    745, 746, 747, 748, 749, 750, 751,
]

import os
sfx = 2
while os.path.exists(RAW):
    RAW = f"../../re/oracle/oracle_engage_lock{sfx}.raw"
    LOG = f"../../re/oracle/oracle_engage_lock{sfx}_timing.log"
    sfx += 1

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()


def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line, flush=True)
    log.write(line + "\n")
    log.flush()


def cmd(text, tag=None, pause=1.6, drain=2.0, echo=True):
    mk = sess.mark()
    sess.send(text, pause=pause)
    sess.dump(drain)
    out = sess.since(mk)
    if echo:
        note(f"--- {tag or text} ---")
        for line in out.splitlines():
            if line.strip():
                note(f"    | {line.rstrip()[:160]}")
    return out


def hp():
    m = re.findall(r"\[HP=(-?\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


# A rat line aimed at me: "The giant rat bites you for N damage!",
# "The giant rat swings at you, but misses!" and kin. Anything rat-ward
# that names "you".
RAT_AT_ME = re.compile(rf"{TARGET} .*\byou\b", re.IGNORECASE)
EXITS = re.compile(r"Obvious exits:\s*([a-z, ]+)", re.IGNORECASE)
DIR_WORD = {"north": "n", "south": "s", "east": "e", "west": "w",
            "northeast": "ne", "northwest": "nw", "southeast": "se",
            "southwest": "sw", "up": "u", "down": "d"}


def rat_attacks_in(text):
    return [ln.strip() for ln in text.splitlines()
            if RAT_AT_ME.search(ln) and "swing at" not in ln
            and not ln.strip().startswith("You")]


def die_and_revive():
    """Mortally wounded refuses every verb; the only exit is death (one
    life, revives at FULL HP) — walk into the spider dens deliberately."""
    note("=== mortally wounded; dying deliberately to revive ===")
    deadline_r = time.time() + 900
    ix_r = 0
    while (hp() or 0) < 0 and time.time() < deadline_r:
        sess.send(f"/xgoto {[1567, 1570, 1572, 1563][ix_r % 4]} 1", pause=1.6)
        ix_r += 1
        sess.dump(1.2)
        for _ in range(40):
            sess.dump(2.0)
            if (hp() or 0) > 0:
                break
    note(f"=== recovered at HP {hp()} ===")


def heal_if_needed(floor=35):
    h = hp()
    if h is not None and h < 0:
        die_and_revive()
        h = hp()
    if h is not None and h < floor:
        cmd(f"/xgoto {HEALER} 1", tag="to healer", echo=False)
        out = cmd("buy healing", tag="heal", drain=2.5, echo=False)
        if "mortally" in out:
            die_and_revive()
        note(f"healed to {hp()}")


def find_rat(start_ix):
    """Sweep the rat rooms from start_ix; return (room, exit_dir, look) or None."""
    for k in range(len(RAT_ROOMS)):
        room = RAT_ROOMS[(start_ix + k) % len(RAT_ROOMS)]
        sess.send(f"/xgoto {room} {MAP}", pause=1.6)
        sess.dump(1.0)
        out = cmd("look", tag=f"room {room}", drain=1.8, echo=False)
        if TARGET not in out:
            continue
        m = EXITS.search(out)
        if not m:
            continue
        first = m.group(1).split(",")[0].strip().lower()
        d = DIR_WORD.get(first)
        if d:
            return room, d, out
    return None


sess.login("Oracle")
sess.send("E")
sess.dump(5.0)
if (hp() or 0) < 0:
    die_and_revive()
cmd("/xcash 2000 copper", tag="heal purse")
heal_if_needed(floor=999)   # start full

results = {"p0": [], "p1": [], "p2": [], "p3": []}
ix = 0

# ---- P0: aggression baseline ----------------------------------------------
for trial in range(10):
    heal_if_needed()
    hit = find_rat(ix)
    if not hit:
        note("P0: no rat found this sweep; continuing")
        ix += 7
        continue
    room, d, _ = hit
    ix += 1
    mk = sess.mark()
    sess.dump(12.0)
    unprovoked = rat_attacks_in(sess.since(mk))
    mk2 = sess.mark()
    sess.send(d, pause=1.6)
    sess.dump(2.5)
    leave = rat_attacks_in(sess.since(mk2))
    results["p0"].append((len(unprovoked), len(leave)))
    note(f"P0 trial {trial}: room {room} unprovoked={len(unprovoked)} "
         f"on-leave={len(leave)}")

# ---- P2: zero-damage retaliation (the sharp pattern) -----------------------
for trial in range(10):
    heal_if_needed()
    hit = find_rat(ix)
    if not hit:
        note("P2: no rat found this sweep; continuing")
        ix += 7
        continue
    room, d, _ = hit
    ix += 1
    mk = sess.mark()
    sess.send(f"a {NOUN}", pause=1.6)
    # three combat rounds is ~15 s; timestamp the first retaliation
    first_at = None
    t_attack = time.time()
    while time.time() - t_attack < 16.0:
        sess.dump(1.0)
        atk = rat_attacks_in(sess.since(mk))
        if atk and first_at is None:
            first_at = time.time() - t_attack
            note(f"P2 trial {trial}: first retaliation at +{first_at:.2f}s: "
                 f"{atk[0][:100]}")
            break
    if first_at is None:
        note(f"P2 trial {trial}: NO retaliation in 16s (room {room})")
    results["p2"].append(first_at)
    # disengage by teleporting away (no walked move, no free attack noise)
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(1.5)

# ---- P1: attack-and-leave before the first round ---------------------------
for trial in range(15):
    heal_if_needed()
    hit = find_rat(ix)
    if not hit:
        note("P1: no rat found this sweep; continuing")
        ix += 7
        continue
    room, d, _ = hit
    ix += 1
    sess.send(f"a {NOUN}", pause=1.6)
    mk = sess.mark()
    sess.send(d, pause=1.6)      # next flood-control slot, ~1.6s later
    sess.dump(2.5)
    out = sess.since(mk)
    free = rat_attacks_in(out)
    results["p1"].append(len(free))
    note(f"P1 trial {trial}: room {room} free-attacks-on-leave={len(free)}"
         + (f" [{free[0][:90]}]" if free else ""))

# ---- P3: attack, one full round, leave (observable control) ----------------
for trial in range(5):
    heal_if_needed()
    hit = find_rat(ix)
    if not hit:
        note("P3: no rat found this sweep; continuing")
        ix += 7
        continue
    room, d, _ = hit
    ix += 1
    sess.send(f"a {NOUN}", pause=1.6)
    sess.dump(6.0)               # let a round land
    mk = sess.mark()
    sess.send(d, pause=1.6)
    sess.dump(2.5)
    free = rat_attacks_in(sess.since(mk))
    results["p3"].append(len(free))
    note(f"P3 trial {trial}: room {room} free-attacks-on-leave={len(free)}")

note("=== SUMMARY ===")
p0u = sum(1 for u, _ in results["p0"] if u)
note(f"P0 baseline: {len(results['p0'])} trials, {p0u} with unprovoked attacks")
p2r = [x for x in results["p2"] if x is not None]
note(f"P2 zero-damage: {len(results['p2'])} trials, {len(p2r)} retaliated; "
     f"first-retaliation times {['%.1f' % x for x in p2r]}")
note(f"P1 attack-and-leave: free attacks per trial {results['p1']}")
note(f"P3 one-round control: free attacks per trial {results['p3']}")
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
