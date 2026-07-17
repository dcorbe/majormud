#!/usr/bin/env python3
"""M5 spell-learning expedition, session 2 (plan Task 5, checklist 4, 7, 8).

Vexil Arcanum (Human Mage L1, purse patched to 50g 20s 50c) starts at the
Newhaven Spell Shop. Captures: read-vs-use consumption control (blur),
paid scroll purchase, the too-high learn attempt (smite L3 — illuminate is
left untouched so the later train test stays clean), blur self-casts with
the prompt's mana drain, the insufficient-mana failure, then a dungeon
patrol to cast magic missile at a live monster for the success strings."""
import re, sys, time
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_spell_cast.raw")
sess.login("Vexil")
sess.send("E")
sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-2000:])

def show(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:1500])
    print()
    return out

show("where", "look")
show("purse", "i")

# read-vs-use consumption control on blur (free scroll)
show("buy-blur", "buy scroll of blur")
show("inv-1-own-blur", "i")
show("read-owned-blur", "read scroll of blur", 2.5)
show("inv-2-after-read", "i")
show("use-blur", "use scroll of blur", 2.5)
show("inv-3-after-use", "i")
show("spells-with-blur", "spells", 2.5)

# shelf-read check: we own no smite scroll yet
show("read-shelf-smite", "read scroll of smite", 2.5)

# paid buy + too-high learn attempt (smite is level 3; we are level 1)
show("buy-smite", "buy scroll of smite")
show("use-smite-toohigh", "use scroll of smite", 2.5)
show("inv-4-after-toohigh", "i")
show("spells-after-toohigh", "spells", 2.5)
show("cast-smit-unlearned", "c smit")

# blur self-casts: mana 12 -> 0 in three casts, then insufficient mana
show("cast-blur-1", "c blur", 2.5)
show("cast-blur-2", "c blur", 2.5)
show("cast-blur-3", "c blur", 2.5)
show("cast-blur-nomana", "c blur", 2.5)
show("health-drained", "health")

# walk to the dungeon; wait for 1 mana, then hunt something to missile
for step in ["s", "w", "d", "n", "n"]:
    sess.send(step)
    sess.dump(1.2)
print("=== AT SMALL CAVERN ===")
print(sess.clean()[-800:])

def mana():
    out = show("mana-poll", "health", 1.5)
    m = re.search(r"Mana:\s*(\d+)/", out)
    return int(m.group(1)) if m else 0

deadline = time.time() + 300
while time.time() < deadline and mana() < 2:
    time.sleep(20)

# patrol loop: cavern <-> entrance <-> arena, casting at anything found
patrol = ["s", "s", "u", "d", "n", "n"]  # 2156->2152->2150->2146->2150->2152->2156
target = None
for lap in range(6):
    out = show(f"patrol-look-{lap}", "look", 1.5)
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    if m:
        first = m.group(1).split(",")[0].strip()
        target = first.split()[-1]
        print(f"TARGET FOUND: {first!r} -> casting at {target!r}")
        break
    step = patrol[lap % len(patrol)]
    sess.send(step)
    sess.dump(1.5)

if target:
    for i in range(8):
        out = show(f"cast-mmis-{i}", f"c mmis {target}", 6.0)
        if "You do not" in out or "don't see" in out:
            break
        if re.search(r"dies|dead|slain|silver|copper|experience", out, re.I):
            show("post-kill-look", "look", 2.0)
            break
        if mana() < 1:
            print("out of mana mid-hunt")
            break
    show("exp-after-kill", "exp")
else:
    print("NO TARGET FOUND on patrol; skipping missile capture")
    show("exp-final", "exp")

show("final-health", "health")
m = sess.mark()
sess.send("x")
sess.dump(8.0)
print("### quit")
print(sess.since(m)[:600])
