#!/usr/bin/env python3
"""M5 spell-learning expedition, session 1 (plan Task 5, checklist 1-5, 7).

Signs up a fresh BBS account (Vexil/test123), rolls a Human Mage
"Vexil Arcanum" (disposable), waits out the ~10 min name validation, then
captures: spells-at-creation, Newhaven Spell Shop listing (Too powerful
suffixes), the learn verb hunt (learn/use/read), spells-after-learning,
and the unknown-spell / no-target cast failures."""
import re, sys, time
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_spell_learning.raw")

# --- BBS account signup ---
sess.ru("Username:")
sess.send("NEW")
sess.ru("unique Username")
sess.send("Vexil")
sess.ru("strong Password:")
sess.send("test123")
sess.ru("re-enter your password")
sess.send("test123")
sess.ru("e-Mail Address:")
sess.send("vexil@example.com")
sess.ru("gender")
sess.send("M")
sess.ru("Make your selection")
sess.send("A")
sess.ru("[MAJORMUD]:")

# --- character creation ---
sess.send("E")
sess.dump(3.0)
menus = sess.clean()[-3000:]
print("=== RACE MENU ===")
print(menus)

def pick(label, deadline_text=None):
    """Find `label` in the visible menu and return its number."""
    text = sess.clean()[-3000:]
    m = re.search(r"(\d+)\s*[\).:\]]?\s*" + label, text)
    return m.group(1) if m else None

race = pick("Human") or "1"
print(f"picking race {race} (Human)")
sess.send(race)
sess.ru("class")
sess.dump(2.0)
print("=== CLASS MENU ===")
print(sess.clean()[-3000:])
klass = pick("Mage") or "12"
print(f"picking class {klass} (Mage)")
sess.send(klass)
sess.ru("Do you want to be Lawful?")
sess.send("No")
sess.ru("CP Left")
sess.dump(2.0)
print("=== STAT EDITOR ===")
print(sess.clean()[-2500:])
sess.s.sendall(b"\r")            # accept first name = Vexil
time.sleep(0.3)
sess.s.sendall(b"Arcanum\r")     # family name
time.sleep(0.3)
for i in range(20):              # CR through remaining fields; Exit=SAVE
    sess.s.sendall(b"\r")
    time.sleep(0.25)
    if "Validating your name" in sess.clean()[-500:]:
        break
    sess.dump(0.3)
sess.ru("Validating your name", timeout=30)
print("waiting out name validation (up to 20 min)...")
deadline = time.time() + 1200
m = sess.mark()
while time.time() < deadline:
    sess.dump(5.0)
    t = sess.since(m)
    if "Obvious exits" in t or "entered the Realm" in t or "[HP=" in t:
        print("validation complete!")
        break
else:
    print("TIMED OUT waiting for validation; tail:")
    print(sess.since(m)[-600:])
    sys.exit(1)

sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-2500:])

def show(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:1500])
    print()
    return out

# checklist 2: spellbook straight out of creation
show("spells-at-creation", "spells", 2.5)
show("stat-sheet", "st", 2.5)
show("exp", "exp")
show("inventory-purse", "i")

# checklist 3: Newhaven Spell Shop (room 1/2144): w then n from entrance
sess.send("w"); sess.dump(1.2)
sess.send("n"); sess.dump(1.2)
show("spellshop-room", "look")
show("spellshop-list", "list", 3.5)

# checklist 4: buy the free magic missile scroll, hunt the learn verb
show("buy-mm-scroll", "buy scroll of magic missile")
show("inv-after-buy", "i")
show("verb-learn", "learn scroll of magic missile", 2.5)
show("verb-use", "use scroll of magic missile", 2.5)
show("verb-read", "read scroll of magic missile", 2.5)
show("inv-after-learn", "i")

# checklist 5: spells after learning
show("spells-after-learning", "spells", 2.5)

# too-powerful purchases (illuminate L2, smite L3) if purse allows
show("buy-illuminate", "buy scroll of illuminate")
show("verb-learn-illuminate", "learn scroll of illuminate", 2.5)
show("verb-use-illuminate", "use scroll of illuminate", 2.5)
show("spells-after-toohigh", "spells", 2.5)

# checklist 7 (partial): unknown spell, no-target cast in an empty shop
show("cast-unknown", "cast zzz")
show("c-unknown", "c zzz")
show("cast-mm-notarget", "cast magic missile")
show("c-shortname", "c mami")
show("spells-final", "spells", 2.5)
show("health-mana", "health")

print("### session 1 done (staying logged out cleanly)")
m = sess.mark()
sess.send("x")
sess.dump(6.0)
print(sess.since(m)[:800])
