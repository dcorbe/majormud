#!/usr/bin/env python3
"""M6 arena expedition, part 2: the fight. Logs the existing Mireko
Softpaw (Nekojin Mystic) back in, walks to the arena, attacks whatever
the spawner provides (parsed from Also here), and captures the full
damage-line census, kill tail, replacement spawn, and departure."""
import re, sys, time
from mudlib import Session

RAW = "../../re/oracle/oracle_m6_arena_fight.raw"
LOG = "../../re/oracle/oracle_m6_arena_fight_timing.log"

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()

def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line)
    log.write(line + "\n")
    log.flush()

def watch(seconds, label, keys):
    note(f"--- watch {label} for {seconds}s ---")
    end = time.time() + seconds
    m = sess.mark()
    while time.time() < end:
        sess.dump(1.0)
        new = sess.since(m)
        m = sess.mark()
        for line in new.splitlines():
            line = line.strip()
            if line and any(k in line for k in keys):
                note(f"  {line[:150]}")

FIGHT_KEYS = ("into the room", "just left", "just arrived", "hear movement",
              "lunges", "lashes", "punch", "swing", "kick", "chop", "jab",
              "claw", "dead", "drop to the ground", "experience", "dodge",
              "glance", "hits you", "damage")

sess.ru("Username:")
sess.send("Mireko")
sess.ru("Password:")
sess.send("test123")
sess.ru("Make your selection")
sess.send("A")
sess.ru("[MAJORMUD]:")
sess.send("E")
sess.dump(4.0)
note("logged back in")
# Re-login resumes wherever we logged out — use the board's new /xgoto
# to land in the arena deterministically (try the syntax variants).
placed = False
# Usage: /xgoto <room> [map]; it confirms without rendering — look after.
for form in ("/xgoto 2150 1",):
    mk = sess.mark()
    sess.send(form, pause=1.5)
    sess.dump(2.0)
    sess.send("look", pause=1.2)
    sess.dump(2.5)
    if "Newhaven, Arena" in sess.since(mk):
        note(f"teleported via {form!r}")
        placed = True
        break
if not placed:
    note("xgoto failed; walking from scratch is unreliable — abort")
    sess.send("x", pause=1.5)
    sess.dump(3.0)
    sys.exit(1)
note("in the arena")

def current_monster():
    m = re.findall(r"Also here: ([^.\n]+)\.", sess.clean()[-1500:])
    if not m:
        return None
    # Monsters render lowercase; players are capitalized names. Pick the
    # first lowercase entry and attack by its first word.
    for entry in m[-1].split(","):
        entry = entry.strip()
        if entry and entry[0].islower():
            return entry.split()[0]
    return None

mk = sess.mark()
sess.send("look", pause=1.2)
sess.dump(2.0)
target = current_monster()
if target is None:
    watch(30, "waiting for a spawn", FIGHT_KEYS)
    sess.send("look", pause=1.2)
    sess.dump(2.0)
    target = current_monster()
if target is None:
    note("no monster to fight; logging out")
    sess.send("x", pause=1.5)
    sess.dump(3.0)
    sys.exit(0)

note(f"=== attacking '{target}' ===")
fight_t0 = time.time()
sess.send(f"a {target}", pause=1.5)
end = time.time() + 420
m = sess.mark()
while time.time() < end:
    sess.dump(1.0)
    new = sess.since(m)
    m = sess.mark()
    for line in new.splitlines():
        line = line.strip()
        if line and any(k in line for k in FIGHT_KEYS):
            note(f"  {line[:150]}")
    if "is dead" in new:
        note(f"KILL after {time.time()-fight_t0:.1f}s")
        break
    hp = re.findall(r"\[HP=(-?\d+)", sess.clean()[-400:])
    if hp and int(hp[-1]) < 9:
        note(f"BABYSITTER: HP {hp[-1]}, fleeing")
        sess.send("u", pause=1.0)
        sess.dump(2.0)
        break

watch(30, "post-kill replacement", FIGHT_KEYS)
note("=== departing (free-attack window) ===")
sess.send("u", pause=1.5)
sess.dump(2.0)
watch(30, "upstairs: pursuit?", FIGHT_KEYS)
note("=== logout ===")
sess.send("x", pause=1.5)
sess.dump(3.0)
note("done")
log.close()
