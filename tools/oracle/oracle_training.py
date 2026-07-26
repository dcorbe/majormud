#!/usr/bin/env python3
"""Capture the trainer flow, and level Oracle Delver up.

Two things are unmeasured before this run:

  * `/xexp` moves the experience total but never advances the level -- the
    module gates levelling on a TRAINER visit, which nothing in `re/oracle/`
    has ever captured.
  * A mortally wounded character (HP < 0) cannot be healed by any verb, so the
    only recovery is death and revival at the cost of one life.  That sequence
    is pinned here too when the character starts downed.

Trainers are shop type 8.  Room 1/289 "Halls of Training, Entrance" carries
shop 39, "Sysop Trainer", with `shopclasslimit = 0` (every class) and a level
band of 1..999 -- and it has no walking path from the rest of the world, so
`/xgoto` is the only way in.  That makes it the natural staging trainer.

Levelling matters to the dodge-parry expedition for a specific reason: the
character's accuracy sets the parry denominator `floor(accuracy/8)`, and
levels 2-3 leave a Warrior at accuracy 43, i.e. still denominator 5 and still
a 40% prediction against a Dodge-20 template -- so the extra hit points are
free from the experiment's point of view.  Levels 4+ move the step.

Usage:  python3 oracle_training.py [target-level]
"""
import re
import sys
import time

from mudlib import Session

RAW = "../../re/oracle/oracle_training.raw"
LOG = "../../re/oracle/oracle_training_timing.log"

TRAINER_ROOM = 289
HEALER_ROOM = 2190
# Level-1-only rooms: a grey spider here will finish a downed character
# quickly, which beats bleeding ~1 HP/slow-tick from -6 to the -200 threshold.
DEATH_ROOMS = [1567, 1570, 1572, 1563, 1560, 1561]

TARGET_LEVEL = int(sys.argv[1]) if len(sys.argv) > 1 else 3

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()


def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line, flush=True)
    log.write(line + "\n")
    log.flush()


def cmd(text, tag=None, pause=1.6, drain=2.2):
    mk = sess.mark()
    sess.send(text, pause=pause)
    sess.dump(drain)
    out = sess.since(mk)
    note(f"--- {tag or text} ---")
    for line in out.splitlines():
        if line.strip():
            note(f"    | {line.rstrip()[:160]}")
    return out


def hp():
    m = re.findall(r"\[HP=(-?\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


def level():
    m = re.findall(r"Level:\s*(\d+)", sess.clean()[-2500:])
    return int(m[-1]) if m else None


sess.login("Oracle")
sess.send("E")
sess.dump(6.0)
cmd("st", tag="STATS on entry")
note(f"entry HP {hp()}, level {level()}")

# --- recover if downed ----------------------------------------------------
if (hp() or 0) < 0:
    note("=== mortally wounded: dying deliberately to revive (costs a life) ===")
    died = False
    deadline = time.time() + 900
    ix = 0
    while not died and time.time() < deadline:
        room = DEATH_ROOMS[ix % len(DEATH_ROOMS)]
        ix += 1
        sess.send(f"/xgoto {room} 1", pause=1.6)
        sess.dump(1.2)
        out = cmd("look", tag=f"death room {room}")
        if "grey spider" not in out and "giant bat" not in out:
            continue
        while time.time() < deadline:
            mk = sess.mark()
            sess.dump(2.0)
            new = sess.since(mk)
            for line in new.splitlines():
                if line.strip() and any(k in line for k in
                                        ("damage", "dead", "died", "miracle",
                                         "ground", "life", "lives", "Healer")):
                    note(f"    | {line.strip()[:160]}")
            h = hp()
            if h is not None and h > 0:
                died = True
                note(f"=== revived at HP {h} ===")
                break
            if not new.strip():
                break
    sess.dump(3.0)

sess.send(f"/xgoto {HEALER_ROOM} 1", pause=1.6)
sess.dump(1.2)
cmd("/xcash 5000 copper", tag="funds")
cmd("buy healing", tag="heal to full")
cmd("st", tag="STATS after recovery")

# --- the trainer ----------------------------------------------------------
note("=== to the Sysop Trainer (1/289) ===")
sess.send(f"/xgoto {TRAINER_ROOM} 1", pause=1.6)
sess.dump(1.2)
cmd("look", tag="TRAINER ROOM")
cmd("list", tag="trainer LIST")

start = level()
note(f"level before training: {start}")
for attempt in range(12):
    lv = level()
    if lv is not None and lv >= TARGET_LEVEL:
        note(f"reached level {lv}")
        break
    out = cmd("train", tag=f"TRAIN attempt {attempt+1}")
    if "experience" in out.lower() and ("not" in out.lower() or "need" in out.lower()):
        note("trainer refuses: granting more experience and retrying")
        cmd("/xexp 5000", tag="xexp top-up")
        continue
    cmd("st", tag="stats after train")

cmd("st", tag="FINAL STATS")
cmd("i", tag="FINAL INVENTORY")
note(f"final level {level()}, HP {hp()}")
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
