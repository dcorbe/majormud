#!/usr/bin/env python3
"""M5 slice-2 Task 4 step 0: the two unmeasured `use`/`read` branches.

Vexil (Human Mage, knows magic missile/blur/illuminate) was last left in
the Newhaven Arena. Walks back to the Spell Shop, then captures:

1. `use scroll of magic missile` while the spell is ALREADY KNOWN
   (buy the free scroll first; `i` before/after pins consumption).
2. `read` the same scroll if it survived, for the read-verb variant.
3. `use zzz` / `read zzz` — nonexistent item (fall-through behavior).
4. `use <non-LearnSp item>` + `read` same — torch from the General Store.
"""
import re, sys
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_use_verbs.raw")
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

# --- navigate to the Spell Shop from wherever we wake up ---
ROUTES = {
    "Newhaven, Arena": ["u", "e", "n"],
    "Newhaven, Narrow Road": ["e", "n"],
    "Newhaven, Narrow Path": ["n"],
    "Newhaven, Spell Shop": [],
    # dungeon patrol rooms from the last expedition
    "Small Cavern": ["s", "s", "u", "e", "n"],
    "Cavern Entrance": ["s", "u", "e", "n"],
}
for attempt in range(4):
    out = show(f"where-{attempt}", "look")
    room = next((r for r in ROUTES if r in out), None)
    if room is None:
        print(f"UNRECOGNIZED ROOM on attempt {attempt}; tail above")
        if attempt == 3:
            sys.exit(1)
        sess.dump(3.0)
        continue
    if room == "Newhaven, Spell Shop":
        break
    for step in ROUTES[room]:
        sess.send(step)
        sess.dump(1.5)
else:
    print("FAILED to reach the Spell Shop")
    sys.exit(1)

# --- measurement 1: use a scroll for an already-known spell ---
show("spells-before", "spells", 2.5)
show("inv-0", "i")
show("buy-mmis", "buy scroll of magic missile")
show("inv-1-own-scroll", "i")
show("use-known-mmis", "use scroll of magic missile", 2.5)   # KEY capture
show("inv-2-after-use", "i")
show("spells-after", "spells", 2.5)
# if the scroll survived, the read variant on a known spell:
show("read-known-mmis", "read scroll of magic missile", 2.5)
show("inv-3-after-read", "i")

# --- measurement 2: nonexistent item ---
show("use-zzz", "use zzz", 2.5)
show("read-zzz", "read zzz", 2.5)

# --- measurement 3: owned non-LearnSp item (torch, General Store) ---
sess.send("s"); sess.dump(1.5)   # Narrow Path
sess.send("s"); sess.dump(1.5)   # General Store
show("store-room", "look")
show("store-list", "list", 3.5)
show("buy-torch", "buy torch")
show("inv-4-own-torch", "i")
show("use-torch", "use torch", 2.5)          # KEY capture
show("read-torch", "read torch", 2.5)
show("inv-5-after", "i")

print("### done, exiting")
m = sess.mark()
sess.send("x")
sess.dump(10.0)
print(sess.since(m)[:600])
