#!/usr/bin/env python3
"""Rescue: Oracle clears the arena slimes, re-arms from the floor, recovers
loot; Vexil's socket stays open to record the downed-victim view."""
import re, sys, time
sys.path.insert(0, "/home/daniel/bbs/tools/oracle")
from mudlib import Session

RAWDIR = "/home/daniel/bbs/re/oracle"

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
    return int(ms[-1]) if ms else 99

# Vexil: reconnect just to observe his own downed state
V = Session(rawfile=f"{RAWDIR}/oracle_monster_lines3.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
print("=== V entry ==="); print(V.clean()[-600:])

# Oracle: to the arena, arm up, clear slimes
O = Session(rawfile=f"{RAWDIR}/oracle_monster_lines3_obs.raw")
O.login("Oracle")
O.send("E"); O.dump(4.0)
out = show(O, "O-entry", "look")
if "Healer" not in out:
    print("Oracle not at Healer; adapting is on you");
O.send("e"); O.dump(1.5)
O.send("d"); O.dump(1.5)
show(O, "O-arena", "look")
# money first so heal runs work, then weapon+armor
for cmd in ["get platinum", "get gold", "get silver", "get copper",
            "get quarterstaff", "arm quarterstaff", "get coif", "wear coif"]:
    show(O, "O-prep", cmd, 1.5)

mV, mO = V.mark(), O.mark()
end = time.time() + 9 * 60
engaged = 0.0
while time.time() < end:
    V.dump(2.5); O.dump(2.5)
    newV = V.since(mV); mV = V.mark()
    newO = O.since(mO); mO = O.mark()
    if newV.strip(): print("[V]", newV.strip())
    if newO.strip(): print("[O]", newO.strip())

    if hp(O) <= 15:
        print(f"--- Oracle heal run (HP={hp(O)})")
        O.send("u"); O.dump(1.5)
        O.send("w"); O.dump(1.5)
        show(O, "O-heal", "buy healing", 2.5)
        O.send("e"); O.dump(1.5)
        O.send("d"); O.dump(1.5)
        mO = O.mark()
        engaged = 0.0
        continue

    if time.time() - engaged > 15:
        out = show(O, "O-look", "look", 1.5)
        mO = O.mark()
        if "slime" in out:
            show(O, "O-attack", "attack slime", 1.5)
            mO = O.mark()
            engaged = time.time()
        else:
            print("--- no slimes visible; arena clear")
            break

# loot the rest of Oracle's gear
for cmd in ["get amulet", "get sickle", "get sickle", "get sickle",
            "get dagger", "look"]:
    show(O, "O-loot", cmd, 1.5)
show(O, "O-inv", "i", 2.5)

# watch Vexil's fate for a few minutes: bleed-out, recovery, or stasis
print("=== watching Vexil (downed) ===")
watch_end = time.time() + 4 * 60
while time.time() < watch_end:
    V.dump(3.0); O.dump(3.0)
    newV = V.since(mV); mV = V.mark()
    newO = O.since(mO); mO = O.mark()
    if newV.strip(): print("[V]", newV.strip())
    if newO.strip(): print("[O]", newO.strip())
    if re.search(r"You (die|have died|are dead)|death", V.clean()[-500:], re.I):
        print("VEXIL DIED"); break
    if hp(V) > 0:
        print("VEXIL RECOVERED"); break
show(V, "V-health-final", "health", 2.0)

# Oracle logs out at the healer
O.send("u"); O.dump(1.5)
O.send("w"); O.dump(1.5)
show(O, "O-final-heal", "buy healing", 2.5)
for attempt in range(4):
    m = O.mark(); O.send("x"); O.dump(16.0)
    out = O.since(m)
    print(f"### O exit {attempt}"); print(out[:400])
    if "saved" in out: break

# Vexil: if he can act, logout too; else just report
if hp(V) > 0:
    m = V.mark(); V.send("look"); V.dump(2.0); print(V.since(m))
    for attempt in range(4):
        m = V.mark(); V.send("x"); V.dump(16.0)
        out = V.since(m)
        print(f"### V exit {attempt}"); print(out[:400])
        if "saved" in out: break
else:
    print(f"VEXIL still downed, HP={hp(V)}; leaving socket to close")
