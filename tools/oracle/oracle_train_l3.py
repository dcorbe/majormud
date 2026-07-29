#!/usr/bin/env python3
"""Train Oracle Delver to level 3 — and NOT level 4.

Level 3 is the slice-8 expedition sweet spot: accuracy is IDENTICAL to
level 2 (isqrt(3) = isqrt(2) = 1 and tdiv(3,2) = tdiv(2,2) = 1, so both
level terms hold) while max HP rises and each level grants +2 lives.
Level 4 jumps accuracy by +10 (isqrt(4) = 2) and would invalidate every
configuration in oracle_dodge_parry.py, so this script hard-stops at 3
and refuses to send `train` when the level reads 3 or higher.

CP stay UNSPENT — stat raises move accuracy.

Field notes encoded here (tools/oracle/README.md):
  * `/xexp` grants experience but does not level; the trainer does.
  * The only trainer is shop 39 in room 1/289 ("Halls of Training"),
    reachable only by `/xgoto`.
  * Something in 1/289 pulses 10-18 damage on a ~4 s cadence — heal
    first, train in short bursts, leave immediately.
  * Training costs copper (50 cp for L2), so the purse is topped first.

Usage: python3 oracle_train_l3.py
"""
import re
import sys
import time

from mudlib import Session

RAW = "../../re/oracle/oracle_train_l3.raw"
LOG = "../../re/oracle/oracle_train_l3_timing.log"
HEALER = 2190
TRAINER = 289

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()


def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line, flush=True)
    log.write(line + "\n")
    log.flush()


def cmd(text, tag=None, pause=1.6, drain=2.0):
    mk = sess.mark()
    sess.send(text, pause=pause)
    sess.dump(drain)
    out = sess.since(mk)
    note(f"--- {tag or text} ---")
    for line in out.splitlines():
        if line.strip():
            note(f"    | {line.rstrip()[:160]}")
    return out


def level():
    out = cmd("st", tag="stats", drain=2.0)
    m = re.search(r"Level:\s*(\d+)", out)
    return int(m.group(1)) if m else None


def hp():
    m = re.findall(r"\[HP=(-?\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


def die_and_revive():
    """Mortally wounded refuses every verb; the only exit is death (one
    life, revives at FULL HP).  Walk into the spider caves and let one
    finish the job — the oracle_dodge_parry recovery, verbatim."""
    note("=== mortally wounded; dying deliberately to revive ===")
    deadline_r = time.time() + 900
    ix = 0
    while (hp() or 0) < 0 and time.time() < deadline_r:
        sess.send(f"/xgoto {[1567, 1570, 1572, 1563][ix % 4]} 1", pause=1.6)
        ix += 1
        sess.dump(1.2)
        mk = sess.mark()
        sess.send("look", pause=1.6)
        sess.dump(1.8)
        if "grey spider" not in sess.since(mk):
            continue
        for _ in range(40):
            sess.dump(2.0)
            if (hp() or 0) > 0:
                break
    note(f"=== recovered at HP {hp()} ===")
    sess.dump(2.0)


sess.login("Oracle")
sess.send("E")
sess.dump(5.0)

if (hp() or 0) < 0:
    die_and_revive()

lv = level()
note(f"entry level {lv}")
if lv is None:
    sys.exit("could not read the level; refusing to train blind")
if lv >= 3:
    note("already level 3+; nothing to do")
    cmd("x", tag="logout", drain=3.0)
    sys.exit(0)

# Fund the trip: healing at 2cp/HP plus training fees.  Exp is usually
# already banked ("You have progressed too far without training!"), so
# grant once and rely on train to say if it is short.
cmd("/xcash 5000 copper", tag="purse for training")
cmd("/xexp 2500", tag="grant exp (no-op if capped)")

deadline = time.time() + 420
while lv < 3 and time.time() < deadline:
    if (hp() or 0) < 0:
        die_and_revive()
    cmd(f"/xgoto {HEALER} 1", tag="to healer")
    out = cmd("buy healing", tag="heal to full", drain=2.5)
    h = hp()
    if h is None or h < 40:  # max is 44 at L2; the hazard hits 10-18/4s
        note(f"HP {h} too low to brave the trainer; retrying the heal")
        continue
    # THE BURST: the trainer room pulses 10-18 damage on a ~4 s cadence
    # (it killed this character once already), so goto/train/goto-out go
    # back-to-back at flood-control spacing with no reads in between —
    # about 3 s in the room, one pulse at most from full HP.
    mk = sess.mark()
    sess.send(f"/xgoto {TRAINER} 1", pause=1.6)
    sess.send("train", pause=1.6)
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(2.5)
    out = sess.since(mk)
    note("--- train burst ---")
    for line in out.splitlines():
        if line.strip():
            note(f"    | {line.rstrip()[:160]}")
    lv = level()
    note(f"level now {lv}")
    if lv is None:
        sys.exit("lost the level readout mid-training")
    if lv >= 3:
        break
    if "enough experience" in out or "need" in out.lower():
        cmd("/xexp 2500", tag="top up exp")

if lv == 3:
    note("=== level 3 reached; CP left unspent by design ===")
elif lv is not None and lv > 3:
    sys.exit(f"OVERSHOT to level {lv} — the expedition configs are void; "
             f"do not run them until this is understood")
else:
    sys.exit(f"training stalled at level {lv}")

cmd("x", tag="logout", drain=3.0)
note("done")
log.close()
