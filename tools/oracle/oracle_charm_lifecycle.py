#!/usr/bin/env python3
"""Slice-8 E4/E5/E6: the live charm lifecycle, the EvilInCombat(52)
charge, and the pet give_up travel range (M7 carries 3, 4, 5).

Runs as a BARD (magery 4): song of charming #49 ("c char <target>",
level 4, mana 4, duration 100 ticks) and song of foolishness #96
("c fool <target>", level 5, mana 6, carries ability 52). The giant rat
(#1, charmlvl 1, charmres 40) is the pet: ~20% resist per cast, DR 1 so
strays cannot one-shot it.

Everything here is a CAPTURE: the port's charm strings are decompile- or
inference-derived (text.rs ORACLE tags), and this transcript is what
retags them MEASURED. Classification is permissive — every line around
each event is logged with a wall-clock delta; the analysis reads the log.

Charm-success detection is mechanical rather than textual (we do not yet
know the wording): cast, walk one room, and look for the rat arriving
behind us — a follower proves the charm; walking back and recasting
handles a resist.

Phases:
  E4  charm: success + resist strings, "Also here" rendering, a 10-room
      follow walk, one assist round (vs a grey spider), attack-own-pet,
      and TWO timed expiries (duration model: ~100 ticks x ~3.03 s).
  E5  foolishness: cast at a PASSIVE rat (charge + grudge retaliation),
      then at an ALREADY-ENGAGED rat (mode not in {0,4} => no charge);
      `st` is captured before/after every cast so any evil-points
      movement shows in the diff.
  E6  give_up: a 40-room walked circuit counting follow arrivals (does
      ordinary following accrue give_up at all?), then the strand test —
      /xgoto ten rooms away (a teleport leaves no breadcrumbs), wait ~20
      ticks, return, repeat until the pet is gone; the release wording
      and the strand count are the measurement.

Usage: python3 oracle_charm_lifecycle.py [account] [password]
"""
import os
import re
import sqlite3
import sys
import time
from collections import deque

from mudlib import Session

ACCOUNT = sys.argv[1] if len(sys.argv) > 1 else "Bard"
PASSWORD = sys.argv[2] if len(sys.argv) > 2 else "test123"

RAW = "../../re/oracle/oracle_charm_lifecycle.raw"
LOG = "../../re/oracle/oracle_charm_lifecycle_timing.log"
sfx = 2
while os.path.exists(RAW):
    RAW = f"../../re/oracle/oracle_charm_lifecycle{sfx}.raw"
    LOG = f"../../re/oracle/oracle_charm_lifecycle{sfx}_timing.log"
    sfx += 1

HEALER = 2190
# The group-6 spawn region spans 429 rooms; sweeping only 20 of them
# found nothing in 50 minutes (the region's rats sit anywhere in it).
# 547-699 is the contiguous sewer run — a ~12-minute lap.
RAT_ROOMS = [r for r in range(547, 700) if r != 592]
SPIDER_ROOMS = [1567, 1570, 1572, 1563, 1560, 1561]
DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw", "u", "d"]
DB = "../../re/mmud_wgnt.sqlite"


def walk_circuit(start_room, steps):
    """A walked loop of plain (type-0) exits on map 1 from start_room:
    BFS out ~steps/2 and walk back the same way. Returns direction list."""
    db = sqlite3.connect(DB)
    ex_cols = ",".join(f"roomexit_{i}" for i in range(1, 11))
    ty_cols = ",".join(f"roomtype_{i}" for i in range(1, 11))
    rooms = {}
    for row in db.execute(
            f"SELECT roomnumber,{ex_cols},{ty_cols} FROM room WHERE mapnumber=1"):
        rm, ex, ty = row[0], row[1:11], row[11:21]
        rooms[rm] = [(d, ex[d]) for d in range(10)
                     if ex[d] and ex[d] > 0 and ty[d] == 0]
    # BFS to depth steps//2 recording the outbound path
    half = steps // 2
    seen = {start_room: []}
    q = deque([start_room])
    far, far_path = start_room, []
    while q:
        cur = q.popleft()
        path = seen[cur]
        if len(path) > len(far_path):
            far, far_path = cur, path
        if len(path) >= half:
            continue
        for d, dest in rooms.get(cur, []):
            if dest in rooms and dest not in seen:
                seen[dest] = path + [d]
                q.append(dest)
    out = [DIRS[d] for d in far_path]
    back = [DIRS[{0: 1, 1: 0, 2: 3, 3: 2, 4: 7, 5: 6, 6: 5, 7: 4,
                  8: 9, 9: 8}[d]] for d in reversed(far_path)]
    return out + back


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


def mana():
    m = re.findall(r"MA=(\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


def wait_mana(need, cap=240):
    end = time.time() + cap
    while (mana() or 0) < need and time.time() < end:
        sess.dump(4.0)
    return (mana() or 0) >= need


def heal_full():
    cmd(f"/xgoto {HEALER} 1", tag="to healer", echo=False)
    cmd("buy healing", tag="heal", drain=2.5, echo=False)
    note(f"HP {hp()} MA {mana()}")


def find_rat(start_ix=0, alone=True):
    for k in range(len(RAT_ROOMS)):
        room = RAT_ROOMS[(start_ix + k) % len(RAT_ROOMS)]
        sess.send(f"/xgoto {room} 1", pause=1.6)
        sess.dump(1.0)
        out = cmd("look", tag=f"room {room}", drain=1.8, echo=False)
        if "giant rat" not in out:
            continue
        others = [h for h in ("spider", "cultist", "slime", "snake")
                  if h in out]
        if alone and others:
            continue
        return room, out
    return None, None


def pet_follows(direction):
    """Walk one room; True if the rat arrives behind us within ~4s."""
    mk = sess.mark()
    sess.send(direction, pause=1.6)
    sess.dump(4.0)
    out = sess.since(mk)
    arrived = [ln.strip() for ln in out.splitlines()
               if "giant rat" in ln and not ln.strip().startswith("You")]
    for ln in arrived:
        note(f"  FOLLOW| {ln[:150]}")
    return bool(arrived), out


def charm_rat(max_casts=12):
    """Cast until a follower is confirmed; returns (room, cast_time) or None."""
    room, _ = find_rat()
    if room is None:
        note("no lone rat found")
        return None
    for attempt in range(max_casts):
        if not wait_mana(4):
            note("mana regen stalled")
            return None
        t_cast = time.time()
        out = cmd("c char rat", tag=f"cast char (attempt {attempt})", drain=2.5)
        if "resist" in out.lower():
            note("  RESIST captured")
            continue
        # follower check: step out and (if confirmed) step back in
        back = {"n": "s", "s": "n", "e": "w", "w": "e"}
        exm = re.search(r"Obvious exits:\s*([a-z]+)", cmd("look", echo=False),
                        re.IGNORECASE)
        d = (exm.group(1).strip().lower() if exm else "n")
        d = {"north": "n", "south": "s", "east": "e", "west": "w"}.get(d, d[:1])
        ok, _ = pet_follows(d)
        if ok:
            note(f"CHARMED at +{time.time()-t_cast:.2f}s from cast")
            sess.send(back.get(d, "s"), pause=1.6)
            sess.dump(3.0)
            return room, t_cast
        note("  no follower yet (failed or silent); recasting")
        sess.send(back.get(d, "s"), pause=1.6)
        sess.dump(2.0)
    return None


sess.login(ACCOUNT, PASSWORD)
sess.send("E")
sess.dump(5.0)
st = cmd("st", tag="STATS on entry")
spl = cmd("spells", tag="spellbook")
if "char" not in spl:
    sys.exit("song of charming is not in the book; stage the Bard first")
cmd("/xcash 3000 copper", tag="purse")
heal_full()

# =========================== E4: the lifecycle ===========================
note("=== E4: charm lifecycle ===")
got = charm_rat()
if not got:
    sys.exit("could not charm a rat; see the log")
room, t_charm = got

cmd("look", tag="E4 'Also here' rendering with a pet present")

note("=== E4: 10-room follow walk ===")
for i, d in enumerate(walk_circuit(room, 10)):
    ok, _ = pet_follows(d)
    note(f"walk {i} {d}: follow={'yes' if ok else 'NO'}")

note("=== E4: assist round vs a grey spider ===")
for sp in SPIDER_ROOMS:
    sess.send(f"/xgoto {sp} 1", pause=1.6)
    sess.dump(1.0)
    out = cmd("look", tag=f"spider room {sp}", drain=1.8, echo=False)
    if "grey spider" in out:
        # NB: the pet does NOT teleport with /xgoto — but a stranded pet
        # is E6's business; for the assist round we need the pet HERE, so
        # only use this arm if the walk brought it. Check first.
        if "giant rat" not in out:
            note("pet did not arrive (teleport strands it) — walking back")
            continue
        cmd("a spider", tag="owner engages; watching for the assist",
            drain=8.0)
        cmd("look", tag="post-round", drain=2.0, echo=False)
        sess.send(f"/xgoto {HEALER} 1", pause=1.6)
        sess.dump(1.5)
        break
note("(assist round is best-effort scripted; the raw holds whatever "
     "happened — walked approach beats teleport next iteration)")

note("=== E4: attack own pet ===")
# The pet followed our WALKS but not the /xgoto; find it standing where
# we last walked, or recharm if the trail is lost.
got = charm_rat()
if got:
    room, t_charm = got
    cmd("a rat", tag="attack own pet", drain=3.0)
    cmd("look", tag="after attacking own pet", drain=2.0)

note("=== E4: timed expiry x2 (duration 100 ticks ~ 303s) ===")
for expiry in range(2):
    got = charm_rat()
    if not got:
        note(f"expiry {expiry}: could not charm; skipping")
        continue
    room, t_charm = got
    deadline = t_charm + 420
    released = None
    mk = sess.mark()
    while time.time() < deadline:
        sess.dump(3.0)
        new = sess.since(mk)
        mk = sess.mark()
        for ln in new.splitlines():
            if "giant rat" in ln and any(w in ln.lower() for w in
                    ("no longer", "wears off", "shakes", "growls", "turns on",
                     "spell", "free")):
                released = time.time() - t_charm
                note(f"RELEASE line at +{released:.1f}s: {ln.strip()[:150]}")
                break
        if released:
            break
    if not released:
        # wording unknown — probe: does it still follow?
        exm = re.search(r"Obvious exits:\s*([a-z]+)",
                        cmd("look", echo=False), re.IGNORECASE)
        d = (exm.group(1).strip().lower() if exm else "n")[:1]
        ok, _ = pet_follows(d)
        note(f"expiry {expiry}: no release line matched by +420s; "
             f"still-following probe = {ok} (wording heuristic may have "
             f"missed it — read the raw)")

# =========================== E5: EvilInCombat(52) ========================
note("=== E5: song of foolishness — the ability-52 charge ===")
heal_full()
cmd("st", tag="st BEFORE passive-target cast")
room, _ = find_rat()
if room:
    if wait_mana(6):
        cmd("c fool rat", tag="fool at a PASSIVE rat", drain=4.0)
        cmd("look", tag="grudge check (does it come for us?)", drain=4.0)
    cmd("st", tag="st AFTER passive-target cast")
    # contrast: engaged target
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(1.5)
    heal_full()
    room, _ = find_rat(start_ix=7)
    if room and wait_mana(6):
        cmd("a rat", tag="engage first", drain=3.0)
        cmd("st", tag="st BEFORE engaged-target cast")
        cmd("c fool rat", tag="fool at the ENGAGED rat", drain=4.0)
        cmd("st", tag="st AFTER engaged-target cast")
        sess.send(f"/xgoto {HEALER} 1", pause=1.6)
        sess.dump(1.5)

# =========================== E6: give_up =================================
note("=== E6: pet give_up — 40-room walked circuit ===")
heal_full()
got = charm_rat()
if got:
    room, t_charm = got
    follows = 0
    walked = 0
    for i, d in enumerate(walk_circuit(room, 40)):
        if time.time() - t_charm > 280:
            note("charm nearing expiry; recharming for the rest of E6")
            got = charm_rat()
            if not got:
                break
            room, t_charm = got
        ok, _ = pet_follows(d)
        walked += 1
        follows += 1 if ok else 0
        if not ok:
            note(f"E6 walk: FOLLOW MISS at step {i}")
    note(f"E6 walked circuit: {follows}/{walked} follows")

note("=== E6: strand test (teleport leaves no breadcrumbs) ===")
got = charm_rat()
if got:
    room, t_charm = got
    strands = 0
    while strands < 20:
        sess.send(f"/xgoto {HEALER} 1", pause=1.6)   # 10+ rooms away
        sess.dump(60.0)                              # ~20 ticks stranded
        sess.send(f"/xgoto {room} 1", pause=1.6)
        sess.dump(1.5)
        out = cmd("look", tag=f"strand {strands}: is the pet still ours?",
                  drain=2.0)
        strands += 1
        if "giant rat" not in out:
            note(f"pet GONE after {strands} strand cycles (wandered or "
                 f"released — the raw's last lines say which)")
            break
        if time.time() - t_charm > 280:
            note("charm expiring during strand test; recharming")
            got = charm_rat()
            if not got:
                break
            room, t_charm = got

note("=== capture complete ===")
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
