#!/usr/bin/env python3
"""M5: monster attack lines, run 5b — funded heals, tight guards.

Run 5 captured the kobold thief pair:
  hit  "The nasty kobold thief stabs you for 2 damage!"
  miss "The nasty kobold thief lunges at you with their shortsword, but you
        dodge!"
but Vexil entered at 8 HP and was downed while looting; his coins are back
on the arena floor. This run: wait out any downing first, ALWAYS heal to
full before fighting, re-grab the coin pile on every arena entry so `buy
healing` stays funded, flee at 20, and never re-enter below 25.

Still wanted: giant-rat and filthbug HIT lines (their misses are already on
record). Kobold swings are also welcome until its quota, then mmis it.

Raw: re/oracle/oracle_monster_attacks2.raw
"""
import re, sys, time
from mudlib import Session

def show(sess, tag, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {tag} ({cmd!r})")
    print(out[:1200]); print()
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

V = Session(rawfile="../../re/oracle/oracle_monster_attacks2.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
END = time.time() + 22 * 60

def wait_miracle():
    print(f"--- downed (HP={hp(V)}); waiting out the miracle")
    while (hp(V) or -1) <= 0 and time.time() < END + 240:
        V.dump(5.0)
    print(f"--- back up (HP={hp(V)}) at the Healer")

if (hp(V) or 1) <= 0:
    wait_miracle()
out = show(V, "entry", "look")
if "Healer" not in out and "Arena" not in out:
    print("unexpected room; bailing"); sys.exit(1)
at_healer = "Healer" in out

def heal():
    # from arena to healer and back; from healer just heal (stay)
    show(V, "heal", "buy healing", 2.0)

def to_arena():
    V.send("e"); V.dump(1.5)
    V.send("d"); V.dump(1.5)

def to_healer():
    V.send("u"); V.dump(1.5)
    V.send("w"); V.dump(1.5)

if not at_healer:
    to_healer()
heal()
to_arena()

def grab_money():
    out = show(V, "money-scan", "look", 1.5)
    for kind in ["platinum", "gold", "silver", "copper"]:
        if kind in out.split("Also here:")[0]:
            show(V, "grab", f"get {kind}", 1.2)

grab_money()

mV = V.mark()
last_look = 0.0
target = None
swings = {}
while time.time() < END:
    V.dump(3.5)
    new = V.since(mV); mV = V.mark()
    if new.strip(): print("[V]", new.strip())
    h = hp(V)
    if h is not None and h <= 0:
        wait_miracle()
        heal()          # top up any bleed remnant (free if full)
        to_arena()
        grab_money()    # coins fell where we died
        target = None
        mV = V.mark()
        continue
    if h is not None and h <= 20:
        print(f"--- flee at HP={h}")
        to_healer()
        heal()
        to_arena()
        target = None
        mV = V.mark()
        continue
    if target:
        swings[target] = swings.get(target, 0) + new.count(" at you") + \
            len(re.findall(r"s you[ ,!]", new))
    if time.time() - last_look > 17:
        out = show(V, "scan", "look", 1.5); mV = V.mark()
        mons = monsters_here(out)
        slimes = [m_ for m_ in mons if "slime" in m_]
        if slimes and len(mons) == len(slimes):
            show(V, "slime-kill", "c mmis slime", 2.0); mV = V.mark()
        elif mons:
            pick = [m_ for m_ in mons if "slime" not in m_][0]
            if pick != target:
                target = pick
                swings.setdefault(target, 0)
            if swings[target] >= 12:
                print(f"--- quota for {target!r}; killing")
                show(V, "kill", f"c mmis {pick.split()[-1]}", 2.0)
                mV = V.mark()
            else:
                show(V, "engage", f"attack {pick.split()[-1]}", 1.5)
                mV = V.mark()
        last_look = time.time()

print("swing tally:", swings)

# haul everything home: pick the pile clean, drop Oracle's gear at Healer
show(V, "end-look", "look", 2.0)
for cmd in ["get club", "get torch", "get scroll", "get scroll",
            "get amulet", "get coif", "get sickle", "get sickle",
            "get sickle", "get dagger", "get quarterstaff",
            "get platinum", "get gold", "get silver", "get copper"]:
    show(V, "sweep", cmd, 1.0)
to_healer()
heal()
for cmd in ["drop quarterstaff", "drop coif", "drop amulet", "drop dagger",
            "drop sickle", "drop sickle", "drop sickle", "drop 24 platinum"]:
    show(V, "handoff", cmd, 1.0)
show(V, "handoff-check", "look", 2.0)
show(V, "inv-final", "i", 2.5)

# home: healer -> e Narrow Road -> e Narrow Path -> e Village Entrance -> n
for step in ["e", "e", "e", "n"]:
    V.send(step); V.dump(1.5)
show(V, "logout-room", "look")
for a in range(4):
    m = V.mark(); V.send("x"); V.dump(16.0)
    out = V.since(m)
    print(f"### exit {a}"); print(out[:400])
    if "saved" in out:
        print("SAVED OK"); break
