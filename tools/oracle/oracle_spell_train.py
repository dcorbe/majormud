#!/usr/bin/env python3
"""M5 spell-learning expedition, session 3 (plan Task 5, checklist 6, 7).

Vexil Arcanum (Human Mage L1, exp patched to 1500) trains a level at the
Newhaven Adventurer's Guild (room 1/2147, shop 38 type 8) and answers the
trainer-grant question: does `spells` gain the level-2 mage spell
(illuminate) that was deliberately never learned from a scroll?

Then: smite scroll (L3) reuse at L2 (still too powerful), illuminate scroll
learn at L2 (gate is level), a cast of illuminate, and a blur-drain loop to
capture the insufficient-mana failure string."""
import re, sys, time
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_spell_train.raw")
sess.login("Vexil")
sess.send("E")
sess.dump(3.5)
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

where = show("where", "look")
show("exp-patched", "exp")
show("spells-pre-train", "spells", 2.5)

# navigate to the Adventurer's Guild (1/2147)
if "Arena" in where:
    steps = ["u", "n"]
elif "Narrow Road" in where:
    steps = ["n"]
elif "Spell Shop" in where:
    steps = ["s", "w", "n"]
elif "Village Entrance" in where:
    steps = ["w", "w", "n"]
else:
    print("UNKNOWN start room; trying arena route")
    steps = ["u", "n"]
for st in steps:
    sess.send(st)
    sess.dump(1.5)
show("guild-room", "look")

# checklist 6: train, then inspect the book
show("train", "train", 3.5)
show("spells-post-train", "spells", 2.5)
show("stats-post-train", "st", 2.5)
show("exp-post-train", "exp")
show("train-again-noexp", "train", 2.5)

# back to the spell shop (2147 s-> 2146, e-> 2143, n-> 2144)
for st in ["s", "e", "n"]:
    sess.send(st)
    sess.dump(1.5)
show("shop-again", "look")
show("list-at-L2", "list", 3.5)

# smite (level 3) is still too powerful at level 2
show("use-smite-at-L2", "use scroll of smite", 2.5)

# illuminate (level 2) should now be learnable -> pins the gate to level
show("buy-illuminate", "buy scroll of illuminate")
show("use-illuminate-at-L2", "use scroll of illuminate", 2.5)
show("spells-with-illu", "spells", 2.5)
show("cast-illu", "c illu", 3.0)

# drain mana with blur until the insufficient-mana failure appears
fail = None
for i in range(14):
    out = show(f"drain-blur-{i}", "c blur", 6.5)
    if "You cast blur" in out or "already cast" in out:
        continue
    fail = out
    break
if fail is None:
    print("NEVER hit the insufficient-mana failure!")
show("health-after-drain", "health")
show("spells-final", "spells", 2.5)

m = sess.mark()
sess.send("x")
sess.dump(9.0)
print("### quit")
print(sess.since(m)[:600])
