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


sess.login("Oracle")
sess.send("E")
sess.dump(5.0)

lv = level()
note(f"entry level {lv}")
if lv is None:
    sys.exit("could not read the level; refusing to train blind")
if lv >= 3:
    note("already level 3+; nothing to do")
    cmd("x", tag="logout", drain=3.0)
    sys.exit(0)

# Fund the trip: healing at 2cp/HP plus training fees.
cmd("/xcash 5000 copper", tag="purse for training")
cmd(f"/xgoto {HEALER} 1", tag="to healer")
cmd("buy healing", tag="heal up", drain=2.5)

deadline = time.time() + 300
while lv < 3 and time.time() < deadline:
    # Grant exp OUTSIDE the hazard room, then dash in, train, dash out.
    cmd("/xexp 2500", tag="grant exp")
    cmd(f"/xgoto {TRAINER} 1", tag="to trainer")
    out = cmd("train", tag="train", drain=2.5)
    cmd(f"/xgoto {HEALER} 1", tag="back to healer")
    cmd("buy healing", tag="re-heal", drain=2.5)
    lv = level()
    note(f"level now {lv}")
    if lv is None:
        sys.exit("lost the level readout mid-training")
    if lv >= 3:
        break
    if "hand over" not in out and "attain" not in out:
        note("train did not take (probably short on exp); granting more")

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
