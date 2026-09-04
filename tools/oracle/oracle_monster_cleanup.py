#!/usr/bin/env python3
"""Cleanup, take 3. Tight rules:
- wait out any downing first (miracle -> Healer, full HP)
- clear the arena BEFORE sweeping; re-scan and HP-check between every get
- flee instantly (u, verified) if anything spawns or HP <= 20
- drop Oracle's gear at the Healer, then save Vexil at the Weapons Shop
"""
import re, sys, time
from mudlib import Session

def show(s, tag, cmd, settle=1.8):
    m = s.mark(); s.send(cmd); s.dump(settle)
    out = s.since(m)
    print(f"### {tag} ({cmd!r})"); print(out[:900]); print()
    return out

def hp(s):
    ms = re.findall(r"\[HP=(-?\d+)", s.clean())
    return int(ms[-1]) if ms else None

V = Session(rawfile="../../re/oracle/oracle_monster_attacks5.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
DEADLINE = time.time() + 25 * 60

def wait_up():
    while (hp(V) or 1) <= 0 and time.time() < DEADLINE:
        V.dump(5.0)

wait_up()

def room():
    out = show(V, "where", "look", 1.6)
    m = re.search(r"Newhaven, ([A-Za-z' ]+?)\s*\r?\n", out)
    return (m.group(1).strip() if m else "?"), out

def move(dir_, expect, tries=5):
    for t in range(tries):
        V.send(dir_); V.dump(1.6)
        if (hp(V) or 1) <= 0:
            wait_up()            # downed mid-move: wake at Healer
            if expect == "Healer":
                return room()[1]
        r, out = room()
        if expect in r:
            return out
    print(f"MOVE FAILED {dir_} -> {expect} (at {r!r})"); sys.exit(1)

def to_road_from(r):
    if "Healer" in r:            move("e", "Narrow Road")
    elif "Narrow Path" in r:     move("w", "Narrow Road")
    elif "Weapons Shop" in r:
        move("s", "Village Entrance"); move("w", "Narrow Path"); move("w", "Narrow Road")
    elif "Village Entrance" in r:
        move("w", "Narrow Path"); move("w", "Narrow Road")
    elif "Arena" in r:           move("u", "Narrow Road")
    elif "Narrow Road" not in r:
        print("unknown room", r); sys.exit(1)

def heal_full():
    move("w", "Healer")
    show(V, "heal", "buy healing", 2.0)
    move("e", "Narrow Road")

def mons_of(out):
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    return [n.strip() for n in (m.group(1).split(",") if m else [])
            if n.strip() and n.strip()[0].islower()]

r, out = room()
to_road_from(r)
heal_full()

# ---- clear-then-sweep loop ----
swept = False
while time.time() < DEADLINE and not swept:
    out = move("d", "Arena")
    mons = mons_of(out)
    if mons:
        tgt = mons[0].split()[-1]
        show(V, "engage", f"c mmis {tgt}", 3.0)
        dead_or_fled = False
        for _ in range(20):
            V.dump(2.5)
            h = hp(V)
            if h is not None and h <= 0:
                wait_up(); to_road_from(room()[0]); heal_full(); dead_or_fled = True; break
            if h is not None and h <= 20:
                move("u", "Narrow Road"); heal_full(); dead_or_fled = True; break
            if re.search(r"You gain \d+ experience", V.clean()[-400:]):
                print("--- kill confirmed"); break
        if not dead_or_fled:
            # stay for the next scan pass (maybe more monsters)
            out = show(V, "rescan", "look", 1.6)
            if mons_of(out):
                continue
        else:
            continue
    # arena clear: sweep, re-scanning every couple of gets
    print("--- sweeping")
    items = ["club", "torch", "scroll", "scroll", "coif", "sickle", "sickle",
             "sickle", "dagger", "quarterstaff", "amulet",
             "platinum", "gold", "silver", "copper"]
    aborted = False
    for i, it in enumerate(items):
        show(V, "get", f"get {it}", 0.9)
        h = hp(V)
        if h is not None and h <= 22:
            print("--- spawn pressure; fleeing mid-sweep")
            move("u", "Narrow Road"); heal_full(); aborted = True; break
        if i % 2 == 1:
            out = show(V, "midscan", "look", 1.2)
            if mons_of(out):
                print("--- monster appeared mid-sweep; fleeing")
                move("u", "Narrow Road"); heal_full(); aborted = True; break
    if not aborted:
        out = show(V, "post-sweep", "look", 1.6)
        if "You notice" not in out or all(
                k not in out.split("Obvious exits")[0].split("You notice")[-1]
                for k in ["club", "coif", "sickle", "dagger", "quarterstaff",
                          "platinum", "gold", "silver", "copper", "scroll",
                          "amulet", "torch"]):
            swept = True
        move("u", "Narrow Road")

show(V, "inv-after-sweep", "i", 2.5)
# ---- hand off Oracle's gear at the Healer ----
move("w", "Healer")
for cmd in ["drop quarterstaff", "drop coif", "drop amulet", "drop dagger",
            "drop sickle", "drop sickle", "drop sickle",
            "drop 24 platinum", "drop 43 gold"]:
    show(V, "handoff", cmd, 0.9)
show(V, "handoff-check", "look", 1.8)
show(V, "final-heal", "buy healing", 2.0)
show(V, "inv-final", "i", 2.5)

# ---- home and save ----
move("e", "Narrow Road"); move("e", "Narrow Path")
move("e", "Village Entrance"); move("n", "Weapons Shop")
for a in range(4):
    m = V.mark(); V.send("x"); V.dump(16.0)
    out = V.since(m)
    print(f"### exit {a}"); print(out[:400])
    if "saved" in out:
        print("SAVED OK"); break
