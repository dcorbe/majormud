#!/usr/bin/env python3
"""Healer services + user_can_use gate expedition (Silvermere).
Oracle Delver: Dwarf Warrior L1, rich purse, starts at the Bank of Godfrey."""
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_healer_gates.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(3.0)

def show(label, cmd, settle=1.8):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:900])
    print()
    return out

def nav(steps):
    for st in steps:
        sess.send(st)
        sess.dump(1.0)

# Bank -> Temple Healer (527).
nav(["n", "w", "w", "w", "w", "w", "w", "n"])
show("healer-room", "look")
show("healer-list", "list", 2.5)
show("buy-healing", "buy healing")
show("buy-healing-full", "buy healing")
show("buy-curing", "buy curing")
show("buy-cure-poison", "buy cure poison")
show("hp-after", "health")

# Healer -> Sarkhee's Jewellery (290): Paladin-only amulet.
nav(["s", "e", "e", "e", "e", "e", "e", "s", "e", "s", "s", "w"])
show("jewellery-room", "look")
show("jewellery-list", "list", 2.5)
show("buy-amulet", "buy silver holy amulet")
show("wear-amulet", "wear amulet")

# Jewellery -> Helfgrim's (355): MinLevel-10 scimitar.
nav(["e", "n", "n", "n", "n", "n", "n", "w"])
show("swords-list", "list", 3.0)
show("buy-scimitar", "buy serrated scimitar")
show("arm-scimitar", "arm serrated scimitar")

# Back to the bank; then Skali's (n, then back) for brass knuckles.
nav(["e", "s", "s", "s", "s", "w"])
show("bank", "look")
nav(["n", "n"])
show("skali-list", "list", 2.5)
show("buy-knuckles", "buy brass knuckles")
show("wear-knuckles", "wear brass knuckles")
nav(["s", "s"])
show("final-inv", "i")
print("### done")
