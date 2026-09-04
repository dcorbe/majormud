#!/usr/bin/env python3
"""Oracle's one-shot gear recovery after the emulator restart.
Strict bail rules: any monster in the arena -> coins only, leave.
HP <= 28 -> leave at once. Ends saved at the Healer, geared."""
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

def room(s):
    out = show(s, "where", "look", 1.5)
    m = re.search(r"(?:Newhaven|Temple), ([A-Za-z' ]+?)\s*\r?\n", out)
    return (m.group(1).strip() if m else "?"), out

def move(s, dir_, expect, tries=5):
    for t in range(tries):
        s.send(dir_); s.dump(1.6)
        r, out = room(s)
        if expect in r:
            return out
    print(f"MOVE FAILED {dir_} -> {expect} (at {r!r})"); sys.exit(1)

def mons_of(out):
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    return [n.strip() for n in (m.group(1).split(",") if m else [])
            if n.strip() and n.strip()[0].islower() and "healer" not in n]

O = Session(rawfile="../../re/oracle/oracle_gear_recovery.raw")
O.login("Oracle")
O.send("E"); O.dump(4.0)
r, out = room(O)
if "Healer" not in r:
    print("Oracle not at Healer; refusing to improvise"); sys.exit(1)

move(O, "e", "Narrow Road")
out = move(O, "d", "Arena")
mons = mons_of(out)
danger = bool(mons)
print("MONSTERS:", mons)
plan = ["get platinum", "get gold", "get silver", "get copper"]
if not danger:
    plan += ["get quarterstaff", "get coif", "get amulet", "get dagger",
             "get sickle", "get sickle", "get sickle", "get club",
             "get torch", "get scroll", "get scroll"]
for i, cmd in enumerate(plan):
    show(O, "grab", cmd, 0.9)
    h = hp(O)
    if h is not None and h <= 28:
        print("--- taking damage; leaving now"); break
    if i % 3 == 2:
        out = show(O, "scan", "look", 1.2)
        if mons_of(out):
            print("--- spawn; leaving with what we have"); break
show(O, "arena-final", "look", 1.6)
move(O, "u", "Narrow Road")
move(O, "w", "Healer")
for cmd in ["arm quarterstaff", "wear coif", "buy healing"]:
    show(O, "fit", cmd, 1.5)
show(O, "inv", "i", 2.5)
for a in range(4):
    m = O.mark(); O.send("x"); O.dump(16.0)
    out = O.since(m)
    print(f"### exit {a}"); print(out[:300])
    if "saved" in out:
        print("SAVED OK"); break
