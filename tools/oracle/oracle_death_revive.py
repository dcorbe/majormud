#!/usr/bin/env python3
"""Capture the death-and-revival sequence for a mortally wounded character.

Oracle Delver was downed during the dodge-parry staging run and cannot be
healed: every recovery verb (`buy healing`, `drink <potion>`) answers "You may
not do that while you are mortally wounded!", and `/xexp` grants experience
without advancing the level (levelling requires a trainer).  The documented
recovery is to let the character die and revive at the area deathroom at the
cost of one life, so this run does that deliberately and pins every string
along the way -- the death threshold, the death lines, the revival room and
the life count before/after.

Bleeding out unattended is ~1 HP per slow tick (roughly 80 minutes from -33),
so the run teleports into a webbed room and lets a grey spider finish it.
"""
import re
import sys
import time

from mudlib import Session

RAW = "../../re/oracle/oracle_death_revive.raw"
LOG = "../../re/oracle/oracle_death_revive_timing.log"

# Level-1-only rooms under Newhaven; a grey spider bites for 3..12.
SPIDER_ROOMS = [1567, 1570, 1572, 1563, 1560, 1561, 1562, 1564]

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


sess.login("Oracle")
sess.send("E")
sess.dump(6.0)
note("=== entered; state before death ===")
cmd("st", tag="STATS BEFORE (lives)")

note(f"HP before = {hp()}")

# Park in a spider room and let the bleed-out finish quickly.
room_ix = 0
died = False
deadline = time.time() + 900
while not died and time.time() < deadline:
    room = SPIDER_ROOMS[room_ix % len(SPIDER_ROOMS)]
    room_ix += 1
    sess.send(f"/xgoto {room} 1", pause=1.6)
    sess.dump(1.2)
    out = cmd("look", tag=f"room {room}", pause=1.6, drain=2.0)
    if "grey spider" not in out and "giant bat" not in out:
        continue
    note(f"=== waiting for the end in room {room} ===")
    quiet = 0
    while time.time() < deadline:
        mk = sess.mark()
        sess.dump(2.0)
        new = sess.since(mk)
        for line in new.splitlines():
            line = line.strip()
            if line and ("HP=" in line or "damage" in line or "dead" in line
                         or "died" in line or "life" in line or "lives" in line
                         or "miracle" in line or "gods" in line
                         or "ground" in line or "Healer" in line):
                note(f"  | {line[:160]}")
        if re.search(r"you (?:have )?(?:died|are dead)|miracle|revive", new, re.I):
            died = True
            note("=== DEATH/REVIVAL detected ===")
            break
        if not new.strip():
            quiet += 1
            if quiet > 12:
                break
        else:
            quiet = 0
        h = hp()
        if h is not None and h > 0:
            died = True
            note(f"=== revived: HP back to {h} ===")
            break

sess.dump(4.0)
cmd("look", tag="ROOM AFTER DEATH")
cmd("st", tag="STATS AFTER (lives spent?)")
cmd("i", tag="INVENTORY AFTER")
note(f"HP after = {hp()}")

cmd("x", tag="logout", pause=1.6, drain=4.0)
note("done")
log.close()
