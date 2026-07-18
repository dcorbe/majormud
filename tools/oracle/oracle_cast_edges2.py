#!/usr/bin/env python3
"""M5 slice-3 Task 6, run 2: close the gaps run 1 opened.

Run 1 (oracle_cast_edges.raw) found bare `c mmis` prints `You must specify
a target for that spell!` in BOTH an empty room and a room with a live
(unengaged) monster — yet the earlier guilt line came from bare
`cast magic missile` with only friendly Rayth present. Three probes:

1. Bare `c mmis` in the Weapons Shop (friendly Nathaniel present) — does
   NPC presence flip the message to the guilt line (same command form)?
2. `c mmi` in an empty room — 3-char shortname-prefix resolution
   (`c mm` failed; exact `c mmis` works; is shortname exact-only?).
3. Bare `c mmis` while melee-ENGAGED with a monster in the Arena — does
   engagement give the bare cast its target?
"""
import re, sys, time
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_cast_edges2.raw")
sess.login("Vexil")
sess.send("E")
sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-1200:])

def show(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:1500])
    print()
    return out

out = show("where", "look")
if "Weapons Shop" not in out:
    print("NOT at the Weapons Shop; bailing")
    sys.exit(1)

# --- probe 1: bare offensive cast with a friendly NPC present ---
show("health-0", "health", 1.5)
show("npc-bare-c-mmis", "c mmis", 7.0)          # KEY: guilt vs must-specify
show("health-1", "health", 1.5)

# --- probe 2: 3-char shortname prefix in an empty room ---
for step in ["s", "w", "w"]:                     # to Narrow Road
    sess.send(step)
    sess.dump(1.5)
for i in range(6):
    out = show(f"empty-check-{i}", "look", 1.5)
    if "Also here:" not in out:
        break
    sess.send(["e", "w"][i % 2])
    sess.dump(1.5)
else:
    print("no empty room found; bailing")
    sys.exit(1)
show("abbrev-c-mmi", "c mmi", 7.0)               # KEY: shortname prefix?
show("health-2", "health", 1.5)

# --- probe 3: bare offensive cast while melee-engaged ---
out = show("pre-arena-look", "look")
if "Narrow Road" not in out:
    sess.send("w")
    sess.dump(1.5)
sess.send("d")
sess.dump(2.0)

target = None
for lap in range(12):
    out = show(f"arena-look-{lap}", "look", 1.5)
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    if m:
        first = m.group(1).split(",")[0].strip()
        target = first.split()[-1]
        print(f"TARGET FOUND: {first!r} -> engaging {target!r}")
        break
    sess.dump(6.0)

if target:
    out = show("melee-engage", f"attack {target}", 6.0)
    if "Combat Engaged" in out or "You" in out:
        out = show("engaged-bare-c-mmis", "c mmis", 7.0)   # KEY
        # finish the fight with targeted casts / melee
        for i in range(10):
            if re.search(r"collapses|falls|dies|You gain \d+ experience", out):
                break
            out = show(f"kill-{i}", f"c mmis {target}", 7.0)
            if "not enough mana" in out:
                out = show(f"melee-{i}", f"attack {target}", 7.0)
        show("post-kill-look", "look", 2.0)
else:
    print("NO MONSTER SPAWNED; probe 3 not captured")

# --- home and clean exit ---
for step in ["u", "e", "e", "n"]:
    sess.send(step)
    sess.dump(1.5)
out = show("logout-room", "look")
for attempt in range(3):
    m = sess.mark()
    sess.send("x")
    sess.dump(14.0)
    out = sess.since(m)
    print(f"### exit attempt {attempt}")
    print(out[:800])
    if "saved" in out:
        break
