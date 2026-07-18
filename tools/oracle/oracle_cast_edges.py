#!/usr/bin/env python3
"""M5 slice-3 Task 6: unmeasured cast edges.

Vexil (Human Mage L2, knows magic missile/blur/illuminate) saved out at the
Newhaven Weapons Shop. Captures:

1. Bare `cast` (and bare `c`) with no argument.
2. Abbreviation resolution: `c mm`, `c magic`, `c magic mi`, `c m` —
   shortname-prefix vs name word-prefix vs both.
3. Offensive bare cast (`c mmis`, no target) in a room WITH a live monster
   (Newhaven Arena spawns filthbugs/rats) — auto-pick like `attack`?
4. Offensive bare cast in a truly EMPTY room (no monster, no NPC) —
   "no effect" family vs the guilt line.
5. `cast blur extra trailing words` — trailing-garbage tolerance.
Bonus: `c bl` / `c b` (blur is Vexil's only b-spell).

Casts are spaced ~7s apart so the one-cast-per-round gate can't contaminate
the next probe; `look` guards every empty-room probe against wander-ins.
"""
import re, sys, time
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_cast_edges.raw")
sess.login("Vexil")
sess.send("E")
sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-1500:])

def show(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:1500])
    print()
    return out

def health():
    out = show("health-poll", "health", 1.5)
    m = re.search(r"Mana:\s*(\d+)/\s*(\d+)", out)
    return (int(m.group(1)), int(m.group(2))) if m else (0, 0)

def wait_mana(n, deadline_s=360):
    cur, mx = health()
    end = time.time() + deadline_s
    while cur < min(n, mx) and time.time() < end:
        time.sleep(20)
        cur, mx = health()
    return cur

out = show("where", "look")
if "Weapons Shop" not in out:
    print("NOT at the Weapons Shop; bailing")
    sys.exit(1)

# --- measurement 1: bare cast, no argument (safe friendly-NPC room) ---
health()
show("bare-cast", "cast", 6.0)          # KEY 1
show("bare-c", "c", 6.0)                # alias form, bonus

# --- to the paths: s = Village Entrance, w = Narrow Path, w = Narrow Road ---
for step in ["s", "w", "w"]:
    sess.send(step)
    sess.dump(1.5)

WANDER = ["e", "w"]  # ping-pong Narrow Road <-> Narrow Path
def ensure_empty(tag):
    for i in range(8):
        out = show(f"empty-check-{tag}-{i}", "look", 1.5)
        if "Also here:" not in out:
            return out
        sess.send(WANDER[i % 2])
        sess.dump(1.5)
    print("could not find an empty room; bailing")
    sys.exit(1)

# --- measurement 4: offensive bare cast in a truly empty room ---
ensure_empty("m4")
health()
show("empty-c-mmis", "c mmis", 7.0)     # KEY 4
health()

# --- measurement 2: abbreviation behavior (empty room each time) ---
for label, cmd in [("abbrev-c-mm", "c mm"),
                   ("abbrev-c-magic", "c magic"),
                   ("abbrev-c-magic-mi", "c magic mi"),
                   ("abbrev-c-m", "c m")]:
    ensure_empty(label)
    show(label, cmd, 7.0)               # KEY 2
    health()

# --- measurement 5: trailing garbage on a self-target spell ---
ensure_empty("m5")
wait_mana(4)
show("blur-trailing", "cast blur extra trailing words", 7.0)   # KEY 5
health()

# --- bonus: prefix ambiguity on blur (only b-spell in the book) ---
ensure_empty("bl")
wait_mana(4)
show("abbrev-c-bl", "c bl", 7.0)
ensure_empty("b")
wait_mana(4)
show("abbrev-c-b", "c b", 7.0)
health()

# --- measurement 3: offensive bare cast WITH a live monster (Arena) ---
wait_mana(6)
out = show("pre-arena-look", "look")
if "Narrow Road" not in out:
    sess.send("w")     # from Narrow Path back to Narrow Road
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
        print(f"TARGET FOUND: {first!r} -> casting at {target!r}")
        break
    sess.dump(6.0)     # give the spawner a beat

if target:
    out = show("monster-bare-c-mmis", "c mmis", 7.0)     # KEY 3
    for i in range(10):
        if re.search(r"collapses|dies|You gain \d+ experience", out):
            break
        cur, _ = health()
        if cur < 1:
            wait_mana(1, 120)
        out = show(f"kill-{i}", f"c mmis {target}", 7.0)
    show("post-kill-look", "look", 2.0)
else:
    print("NO MONSTER SPAWNED IN ARENA; measurement 3 not captured")

# --- home to the Weapons Shop and clean exit ---
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
