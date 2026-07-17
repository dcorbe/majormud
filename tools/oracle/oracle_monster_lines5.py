#!/usr/bin/env python3
"""M5: monster attack lines, run 5 (final) — rat/filthbug hits, single session.

Prior state (runs 1-4, logs+raws in re/oracle/oracle_monster_lines*):
  - Acid slime family fully measured (hit/miss/dodge, victim+observer,
    acid rider, downing, death-miracle at -7x maxhp, disconnect item drop).
  - Two arena acid slimes made the room unfarmable; an MBBSEmu restart
    despawned them and the combined loot pile PERSISTED on the arena floor.

This run, Vexil solo (29 HP mage; Oracle sits out — 2 lives left):
  1. Loot the whole arena pile (Vexil's own kit + Oracle's gear + money).
  2. Drop Oracle's gear at the Healer (his save room; short handoff run
     re-arms him later).
  3. Bait the arena: rats / filthbugs / kobolds get ~10 swings each while
     Vexil stands unarmored (their HIT wording has never been captured);
     acid slimes are NOT fought: flee up, heal, wait, re-enter.
  4. Guards: heal run at HP<=18 (buy healing, 2cp/HP, one room away);
     immediate flee if a slime is present; hard stop + logout at deadline.

Raw: re/oracle/oracle_monster_attacks.raw
"""
import re, sys, time
from mudlib import Session

def show(sess, tag, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {tag} ({cmd!r})")
    print(out[:1400]); print()
    return out

def hp(sess):
    ms = re.findall(r"\[HP=(-?\d+)", sess.clean())
    return int(ms[-1]) if ms else None

def monsters_here(out):
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    if not m:
        return []
    return [n.strip() for n in m.group(1).split(",")
            if n.strip() and n.strip()[0].islower() and "healer" not in n]

V = Session(rawfile="../../re/oracle/oracle_monster_attacks.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
out = show(V, "entry", "look")
if "Healer" in out:
    V.send("e"); V.dump(1.5)
    V.send("d"); V.dump(1.5)
elif "Arena" not in out:
    print("unexpected start room; bailing"); sys.exit(1)
out = show(V, "arena", "look")
if "slime" in out:
    print("slimes are back already; bailing"); V.send("u"); sys.exit(1)

# --- 1: loot everything ---
for cmd in ["get platinum", "get gold", "get silver", "get copper",
            "get club", "get torch", "get scroll", "get scroll",
            "get amulet", "get coif", "get sickle", "get sickle",
            "get sickle", "get dagger", "get quarterstaff"]:
    show(V, "loot", cmd, 1.2)
show(V, "loot-check", "look", 2.0)
show(V, "inv", "i", 2.5)

# --- 2: drop Oracle's gear at the Healer ---
V.send("u"); V.dump(1.5)
V.send("w"); V.dump(1.5)
for cmd in ["drop quarterstaff", "drop coif", "drop amulet", "drop dagger",
            "drop sickle", "drop sickle", "drop sickle", "drop 24 platinum",
            "drop 43 gold"]:
    show(V, "handoff", cmd, 1.2)
show(V, "handoff-check", "look", 2.0)
show(V, "heal-top", "buy healing", 2.0)
V.send("e"); V.dump(1.5)
V.send("d"); V.dump(1.5)

# --- 3: bait loop ---
END = time.time() + 20 * 60
mV = V.mark()
last_look = 0.0
target = None
swings = {}
def flee_heal_return(reason):
    global mV, target
    print(f"--- flee ({reason}, HP={hp(V)})")
    V.send("u"); V.dump(1.5)
    V.send("w"); V.dump(1.5)
    show(V, "heal", "buy healing", 2.0)
    V.send("e"); V.dump(1.5)
    V.send("d"); V.dump(1.5)
    target = None
    mV = V.mark()

while time.time() < END:
    V.dump(3.5)
    new = V.since(mV); mV = V.mark()
    if new.strip(): print("[V]", new.strip())
    h = hp(V)
    if h is not None and h <= 0:
        # downed: nothing works; wait out the miracle, walk back
        print(f"--- DOWN at {h}; waiting for miracle")
        while (hp(V) or -1) <= 0 and time.time() < END + 300:
            V.dump(5.0)
        print(f"--- miracled (HP={hp(V)}); items dropped where I fell")
        V.send("e"); V.dump(1.5)
        V.send("d"); V.dump(1.5)
        mV = V.mark()
        continue
    if h is not None and h <= 18:
        flee_heal_return("low hp")
        continue
    if target:
        swings[target] = swings.get(target, 0) + len(
            re.findall(r" at you|s you .*for \d+ damage|s you!", new))
    if time.time() - last_look > 17:
        out = show(V, "scan", "look", 1.5); mV = V.mark()
        mons = monsters_here(out)
        if any("slime" in m_ for m_ in mons):
            # a single slime is killable from full HP (mmis auto-repeats);
            # the <=18 guard bails us out if the rolls go bad
            if (hp(V) or 0) >= 25:
                show(V, "slime-kill", "c mmis slime", 2.0); mV = V.mark()
            else:
                flee_heal_return("slime spawn, hp low")
        elif mons:
            name = mons[0]
            if name != target:
                target = name
                swings.setdefault(target, 0)
            if swings.get(target, 0) >= 10:
                print(f"--- quota for {target!r}; killing")
                show(V, "kill", f"c mmis {name.split()[-1]}", 2.0)
                mV = V.mark()
            else:
                show(V, "engage", f"attack {name.split()[-1]}", 1.5)
                mV = V.mark()
        last_look = time.time()

print("swing tally:", swings)

# --- 4: heal, home, logout ---
if (hp(V) or 0) <= 0:
    while (hp(V) or -1) <= 0:
        V.dump(5.0)
    show(V, "post-miracle", "look", 2.0)
else:
    V.send("u"); V.dump(1.5)
    V.send("w"); V.dump(1.5)
    show(V, "final-heal", "buy healing", 2.0)
    V.send("e"); V.dump(1.5)
for step in ["e", "e", "n"]:
    V.send(step); V.dump(1.5)
show(V, "logout-room", "look")
for a in range(4):
    m = V.mark(); V.send("x"); V.dump(16.0)
    out = V.since(m)
    print(f"### exit {a}"); print(out[:400])
    if "saved" in out:
        print("SAVED OK"); break
