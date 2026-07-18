#!/usr/bin/env python3
"""M5: monster attack lines, run 4 — clear slimes, recover gear, capture
rat/filthbug HIT lines.

Lessons burned in from runs 1-2 (logs in re/oracle/oracle_monster_lines*.log):
  - HP prompt goes NEGATIVE when downed: parse \\[HP=(-?\\d+).
  - Downed chars can do NOTHING; monsters keep hitting until death at about
    -(7*maxhp), then "You have been killed! / But, due to a miracle, you
    have been saved. / You have N lives left." and you wake at the HEALER
    at full HP. A downed char must simply be waited out.
  - Disconnecting while playing = "The gods have punished you appropriately"
    (items dropped where you stood). Never kill the sockets: exit with `x`.
  - `buy healing` at the Newhaven healer = instant full heal for coppers.
  - Acid slimes burst a 29-HP mage in ~4 rounds; retreat threshold 15 was
    far too late given ~10 dmg/round from two slimes. Use 18 + fast heals.

Phases:
  1. Vexil (mmis, auto-repeats per round) + Oracle (punches) kill the two
     arena acid slimes; heal runs at HP<=18; wait out any downing.
  2. Oracle re-arms from the arena floor (his gear dropped by run-1's
     disconnect punishment): money, quarterstaff (arm), chain coif (wear),
     amulet, sickles, dagger.
  3. Idle in the arena as spawn bait: rats / filthbugs swing at Vexil
     (unarmored -> hits land -> HIT wording) and at Oracle (chain coif ->
     possible glancing family). Slimes are killed on sight. After ~8 swings
     from one monster it is missile-killed to cycle the spawn variety.
  4. Full heal, Vexil logs out at the Weapons Shop, Oracle at the Healer.

Raw: re/oracle/oracle_monster_lines4.raw (Vexil),
     re/oracle/oracle_monster_lines4_obs.raw (Oracle).
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
    return int(ms[-1]) if ms else 99

def monsters_here(out):
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    if not m:
        return []
    return [n.strip() for n in m.group(1).split(",")
            if n.strip() and n.strip()[0].islower() and "healer" not in n]

END = time.time() + 40 * 60

def wait_out_downed(sess, name):
    """Downed char: wait for the miracle, then walk Healer -> arena."""
    print(f"--- {name} is DOWN (HP={hp(sess)}); waiting for the miracle")
    while hp(sess) <= 0 and time.time() < END:
        sess.dump(4.0)
    if hp(sess) <= 0:
        return False
    print(f"--- {name} miracled back (HP={hp(sess)}); returning to arena")
    sess.dump(3.0)
    sess.send("e"); sess.dump(1.5)
    sess.send("d"); sess.dump(1.5)
    return True

def heal_run(sess, name):
    print(f"--- {name} heal run (HP={hp(sess)})")
    sess.send("u"); sess.dump(1.5)
    sess.send("w"); sess.dump(1.5)
    show(sess, f"{name}-heal", "buy healing", 2.5)
    sess.send("e"); sess.dump(1.5)
    sess.send("d"); sess.dump(1.5)

def guard(sess, name):
    """Returns True if the char needed intervention this cycle."""
    h = hp(sess)
    if h <= 0:
        wait_out_downed(sess, name)
        return True
    if h <= 18:
        heal_run(sess, name)
        return True
    return False

# --- logins: both saved at the Newhaven Healer ---
V = Session(rawfile="../../re/oracle/oracle_monster_lines4.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
out = show(V, "V-entry", "look")
if "Healer" not in out:
    print("Vexil not at the Healer; bailing"); sys.exit(1)
V.send("e"); V.dump(1.5)
V.send("d"); V.dump(1.5)
show(V, "V-arena", "look")

O = Session(rawfile="../../re/oracle/oracle_monster_lines4_obs.raw")
O.login("Oracle")
O.send("E"); O.dump(4.0)
out = show(O, "O-entry", "look")
if "Healer" not in out:
    print("Oracle not at the Healer; bailing"); sys.exit(1)
O.send("e"); O.dump(1.5)
O.send("d"); O.dump(1.5)
show(O, "O-arena", "look")

mV, mO = V.mark(), O.mark()

def pump(quiet=False):
    global mV, mO
    V.dump(3.0); O.dump(3.0)
    newV = V.since(mV); mV = V.mark()
    newO = O.since(mO); mO = O.mark()
    if not quiet:
        if newV.strip(): print("[V]", newV.strip())
        if newO.strip(): print("[O]", newO.strip())
    return newV, newO

# --- phase 1: kill the slimes ---
print("===== PHASE 1: slime clearance =====")
last_cast = 0.0
while time.time() < END:
    pump()
    if guard(V, "Vexil"):  mV = V.mark(); continue
    if guard(O, "Oracle"): mO = O.mark(); continue
    if time.time() - last_cast > 14:
        out = show(V, "V-look", "look", 1.5); mV = V.mark()
        if not any("slime" in m_ for m_ in monsters_here(out)):
            print("--- slimes cleared")
            break
        show(V, "V-mmis", "c mmis slime", 2.0); mV = V.mark()
        show(O, "O-punch", "attack slime", 1.5); mO = O.mark()
        last_cast = time.time()

# --- phase 2: Oracle re-arms from the floor ---
print("===== PHASE 2: gear recovery =====")
for cmd in ["get platinum", "get gold", "get silver", "get copper",
            "get quarterstaff", "arm quarterstaff", "get coif", "wear coif",
            "get amulet", "get sickle", "get sickle", "get sickle",
            "get dagger"]:
    if hp(O) <= 0 and not wait_out_downed(O, "Oracle"):
        break
    show(O, "O-loot", cmd, 1.5)
show(O, "O-inv", "i", 2.5)
show(O, "O-room-after-loot", "look", 2.0)
mV, mO = V.mark(), O.mark()

# --- phase 3: rat / filthbug swings at both ---
print("===== PHASE 3: spawn bait =====")
phase3_end = min(END, time.time() + 18 * 60)
last_engage = 0.0
swings = {}          # monster name -> victim-line count seen by Vexil
target = None
while time.time() < phase3_end:
    newV, _ = pump()
    if guard(V, "Vexil"):  mV = V.mark(); continue
    if guard(O, "Oracle"): mO = O.mark(); continue
    if target:
        swings[target] = swings.get(target, 0) + len(
            re.findall(r" at you|whips you|bites you|claws you", newV))
    if time.time() - last_engage > 18:
        out = show(V, "V-look", "look", 1.5); mV = V.mark()
        mons = monsters_here(out)
        slimes = [m_ for m_ in mons if "slime" in m_]
        others = [m_ for m_ in mons if "slime" not in m_]
        if slimes:
            show(V, "V-mmis-slime", "c mmis slime", 2.0); mV = V.mark()
        elif others:
            name = others[0]
            word = name.split()[-1]
            if name != target:
                target = name
            if swings.get(target, 0) >= 8:
                print(f"--- {target!r} quota reached; cycling spawn")
                show(V, "V-mmis-cycle", f"c mmis {word}", 2.0); mV = V.mark()
            else:
                show(V, "V-engage", f"attack {word}", 1.5); mV = V.mark()
        last_engage = time.time()
print("swing tally:", swings)

# --- phase 4: heal, park, logout ---
print("===== PHASE 4: logout =====")
def logout(sess, name):
    for attempt in range(4):
        m = sess.mark()
        sess.send("x"); sess.dump(16.0)
        out = sess.since(m)
        print(f"### {name} exit {attempt}"); print(out[:500])
        if "saved" in out:
            return True
    return False

for sess, name in [(V, "Vexil"), (O, "Oracle")]:
    if hp(sess) <= 0:
        wait_out_downed(sess, name)   # wakes at the Healer, walks to arena
heal_run(V, "Vexil")                  # ends back in the arena, full HP
V.send("u"); V.dump(1.5)
for step in ["e", "e", "n"]:
    V.send(step); V.dump(1.5)
show(V, "V-logout-room", "look")
okV = logout(V, "Vexil")
O.send("u"); O.dump(1.5)
O.send("w"); O.dump(1.5)
show(O, "O-final-heal", "buy healing", 2.5)
show(O, "O-logout-room", "look")
okO = logout(O, "Oracle")
print(f"LOGOUT: Vexil={'ok' if okV else 'FAILED'} Oracle={'ok' if okO else 'FAILED'}")
