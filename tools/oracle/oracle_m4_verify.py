#!/usr/bin/env python3
"""M4 VERIFY-tag expedition: paid-buy/sell wordings, price column format,
out-of-stock, 1H/2H suffixes, worn suffix, arm/wield/wear abbreviations.

Oracle Delver starts in the Bank of Godfrey (1,297) with a patched purse
(20 plat, 500 gold, 50 silver, 50 copper). Routes are BFS over map 1.
"""
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_m4_verify.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(3.0)

def show(label, cmd, settle=1.6):
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

show("purse-check", "i")

# Bank -> General Store (334).
nav(["n", "e", "e", "e", "e", "s"])
show("general-store", "look")
show("general-list", "list", 2.5)
show("buy-lantern", "buy lantern")
show("inv-after-buy", "i")
show("sell-lantern", "sell lantern")
show("inv-after-sell", "i")

# General Store -> Helfgrim's Blades (355).
nav(["n", "w", "w", "n", "w", "n", "n", "w"])
show("helfgrim", "look")
show("sword-list", "list", 3.0)
show("buy-dagger", "buy dagger")
# arm/wield/equip abbreviation probes (dagger in inventory, staff armed).
show("arm-2", "ar dagger")
show("arm-3", "arm dagger")
show("wield-2", "wi quarterstaff")
show("wield-3", "wie dagger")
show("equip-2", "eq quarterstaff")
# out-of-stock: sickle max 5.
for i in range(5):
    show(f"buy-sickle-{i+1}", "buy sickle")
show("buy-sickle-out", "buy sickle")

# Helfgrim's -> Skali's front room (305).
nav(["e", "s", "s", "w"])
show("skali", "look")
show("armour-list", "list", 2.5)
show("buy-coif", "buy chain coif")
# wear/remove abbreviation probes.
show("wear-2", "we coif")
show("wear-3", "wea coif")
show("inv-worn", "i")
show("remove-3", "rem coif")
show("remove-2", "re coif")
show("wear-full", "wear coif")

# Back to the bank (s, s); tidy up.
nav(["s", "s"])
show("bank-again", "look")
show("final-inv", "i")
print("### done")
