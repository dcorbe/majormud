#!/usr/bin/env python3
"""The charm expedition's trimmed tail: one timed expiry with tight
timestamps, E5's two foolishness casts, and the E6 strand test.

Everything E4 needed is already banked in oracle_charm_lifecycle6.raw
(charm success/resist wording, Also-here rendering, a 10-room follow
walk, the assist round, attack-own-pet). This script deliberately skips
the phases that were killing the Bard (long walks and fights in
aggression-30 country) and keeps combat exposure to the casts alone.

Usage: python3 oracle_charm_finish.py [account] [password]
"""
import os
import re
import sys
import time

from mudlib import Session

ACCOUNT = sys.argv[1] if len(sys.argv) > 1 else "Bard"
PASSWORD = sys.argv[2] if len(sys.argv) > 2 else "test123"

RAW = "../../re/oracle/oracle_strand.raw"
LOG = "../../re/oracle/oracle_strand_timing.log"
sfx = 2
while os.path.exists(RAW):
    RAW = f"../../re/oracle/oracle_strand{sfx}.raw"
    LOG = f"../../re/oracle/oracle_strand{sfx}_timing.log"
    sfx += 1

HEALER = 2190
KOBOLD_ROOMS = [722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732,
                733, 734, 745, 746, 747, 748, 749, 750, 751]
FOLLOW_RE = re.compile(
    r"(moves into the room|just arrived|walks into the room|follows you)",
    re.IGNORECASE)

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


def wait_mana(need, cap=180):
    end = time.time() + cap
    while (mana() or 0) < need and time.time() < end:
        sess.dump(4.0)
    return (mana() or 0) >= need


def heal_full():
    cmd(f"/xgoto {HEALER} 1", tag="to healer", echo=False)
    cmd("buy healing", tag="heal", drain=2.5, echo=False)
    note(f"HP {hp()} MA {mana()}")


def find_kobold(start=0):
    for k in range(len(KOBOLD_ROOMS)):
        room = KOBOLD_ROOMS[(start + k) % len(KOBOLD_ROOMS)]
        sess.send(f"/xgoto {room} 6", pause=1.6)
        sess.dump(4.0)
        out = cmd("look", drain=1.8, echo=False)
        if "kobold" in out:
            return room
    return None


def charm_here(max_casts=30):
    """Cast until success; verify by ONE walk-out/arrival check."""
    for attempt in range(max_casts):
        if not wait_mana(4):
            return False
        out = cmd("c char kobold", tag=f"cast char {attempt}", drain=2.5)
        if "resist" in out.lower():
            note("  RESIST")
            continue
        if "fail" in out.lower():
            continue
        mk = sess.mark()
        exm = re.search(r"Obvious exits:\s*([a-z]+)", cmd("look", echo=False),
                        re.IGNORECASE)
        d = (exm.group(1).strip().lower() if exm else "n")
        d = {"north": "n", "south": "s", "east": "e", "west": "w"}.get(d, d[:1])
        back = {"n": "s", "s": "n", "e": "w", "w": "e"}.get(d, "s")
        sess.send(d, pause=1.6)
        sess.dump(4.0)
        arrived = any("kobold" in ln and FOLLOW_RE.search(ln)
                      for ln in sess.since(mk).splitlines())
        sess.send(back, pause=1.6)
        sess.dump(3.0)
        if arrived:
            note("CHARMED (arrival-verified)")
            return True
    return False


sess.login(ACCOUNT, PASSWORD)
sess.send("E")
sess.dump(5.0)
tail = sess.clean()[-500:]
if "choose" in tail.lower() and "race" in tail.lower():
    sys.exit("account at creation screen")
st = cmd("st", tag="STATS on entry")
lm = re.search(r"Lives/CP:\s*(\d+)", st)
if lm and int(lm.group(1)) <= 3:
    sys.exit(f"only {lm.group(1)} lives — train first")
cmd("/xcash 3000 copper", tag="purse")
heal_full()

SKIP_EXPIRY = True
# --- one tight expiry (SKIPPED in the strand-only variant) -----------------
note("=== strand-only run ===")
room = None
if False:
    t_charm = time.time()
    deadline = t_charm + 480
    mk = sess.mark()
    while time.time() < deadline:
        sess.dump(8.0)
        new = sess.since(mk)
        mk = sess.mark()
        for ln in new.splitlines():
            if "kobold" in ln and not ln.strip().startswith("You"):
                note(f"  +{time.time()-t_charm:5.1f}s | {ln.strip()[:140]}")
        # follow probe every ~60s: cheap single-room walk
        if int(time.time() - t_charm) % 60 < 10:
            exm = re.search(r"Obvious exits:\s*([a-z]+)",
                            cmd("look", echo=False), re.IGNORECASE)
            d = (exm.group(1).strip().lower() if exm else "n")[:1]
            back = {"n": "s", "s": "n", "e": "w", "w": "e"}.get(d, "s")
            mk2 = sess.mark()
            sess.send(d, pause=1.6)
            sess.dump(3.5)
            still = any("kobold" in ln and FOLLOW_RE.search(ln)
                        for ln in sess.since(mk2).splitlines())
            sess.send(back, pause=1.6)
            sess.dump(2.5)
            note(f"  follow probe at +{time.time()-t_charm:5.1f}s: {still}")
            if not still:
                note(f"RELEASED between the last two probes "
                     f"(<= +{time.time()-t_charm:.0f}s)")
                break
else:
    note("no charm for the expiry block")

note("=== E5 skipped (already captured) ===")
if False:
    note("=== E5: song of foolishness ===")
heal_full()
cmd("st", tag="st BEFORE passive cast")
room = None
if room and wait_mana(6):
    cmd("c fool kobold", tag="fool at a PASSIVE kobold", drain=4.0)
    cmd("look", tag="grudge check", drain=4.0)
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(1.5)
    cmd("st", tag="st AFTER passive cast")
heal_full()
room = None
if room and wait_mana(6):
    cmd("a kobold", tag="engage first", drain=4.0)
    cmd("st", tag="st BEFORE engaged cast", drain=2.0)
    if wait_mana(6):
        cmd("c fool kobold", tag="fool at the ENGAGED kobold", drain=4.0)
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(1.5)
    cmd("st", tag="st AFTER engaged cast")

# --- E6: strand test -------------------------------------------------------
note("=== E6: strand test (teleport-only; no walking) ===")
heal_full()
room = find_kobold()
if room and charm_here():
    t_charm = time.time()
    for strand in range(8):
        sess.send(f"/xgoto {HEALER} 1", pause=1.6)
        sess.dump(45.0)                    # ~15 stranded ticks
        sess.send(f"/xgoto {room} 6", pause=1.6)
        sess.dump(1.5)
        out = cmd("look", tag=f"strand {strand}", drain=2.0)
        if "kobold" not in out:
            note(f"pet GONE after {strand+1} strand cycles")
            break
        if time.time() - t_charm > 270:
            note("charm nearing expiry; ending the strand test here")
            break
else:
    note("no charm for the strand test")

note("=== finish capture complete ===")
sess.send(f"/xgoto {HEALER} 1", pause=1.6)
sess.dump(1.5)
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
