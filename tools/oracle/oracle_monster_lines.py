#!/usr/bin/env python3
"""M5: monster attack line wording — hit vs miss vs glance, victim + observer.

Dual session:
  - Vexil (Human Mage L2, 29 HP, unarmored, punches) is the VICTIM: walks
    from the Weapons Shop to the Newhaven Arena (s, w, w, d) and engages
    whatever spawns (giant rat family / nasty filthbug / ...).
  - Oracle (Dwarf Warrior, 35 HP, chain coif) is the OBSERVER: walks from
    the Newhaven Healer (e, d) into the same Arena and stands there. Any
    swing at Vexil gives Oracle the third-person line; any swing at Oracle
    gives Vexil the third-person line AND may stage a glance-off-armor line.

Prior transcripts show plain "The giant rat lunges at you!" with NO HP loss,
so that is presumed the MISS line; a real HIT (with damage) has never been
captured. This run idles through many rounds, tallying per-monster lines and
correlating with the live [HP=..] prompt. After ~8 swings from one monster
Vexil kills it with magic missiles (1 mana each) to cycle spawn variety.

Safety: Vexil retreats up and rests below 15 HP; Oracle below 18 HP.

Raw: re/oracle/oracle_monster_lines.raw (Vexil),
     re/oracle/oracle_monster_lines_obs.raw (Oracle).
"""
import re, sys, time
from collections import defaultdict
from mudlib import Session

def show(sess, tag, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {tag} ({cmd!r})")
    print(out[:1500])
    print()
    return out

def hp(sess):
    ms = re.findall(r"\[HP=(\d+)", sess.clean())
    return int(ms[-1]) if ms else 99

# --- Vexil to the Arena ---
V = Session(rawfile="../../re/oracle/oracle_monster_lines.raw")
V.login("Vexil")
V.send("E"); V.dump(3.0)
out = show(V, "V-start", "look")
if "Weapons Shop" not in out:
    print("Vexil NOT at the Weapons Shop; bailing")
    sys.exit(1)
for step in ["s", "w", "w", "d"]:
    V.send(step); V.dump(1.5)
show(V, "V-arena", "look")

# --- Oracle to the Arena (observer) ---
O = Session(rawfile="../../re/oracle/oracle_monster_lines_obs.raw")
O.login("Oracle")
O.send("E"); O.dump(3.0)
out = show(O, "O-start", "look")
if "Healer" not in out:
    print("Oracle NOT at the Healer; observer skipped")
    O = None
else:
    for step in ["e", "d"]:
        O.send(step); O.dump(1.5)
    show(O, "O-arena", "look")

# --- main capture loop ---
SWING = re.compile(r"^\[?[^:]*:?(The .+ (?:at you|at Vexil|at Oracle).*)$")
tally = defaultdict(lambda: [0, 0])   # first word(s) of line -> [hits, misses]
swings_this_target = 0
target = None
last_engage = 0.0
end = time.time() + 22 * 60
mV, mO = V.mark(), (O.mark() if O else 0)

def rest(sess, name, until):
    print(f"--- {name} retreating to rest (HP={hp(sess)})")
    sess.send("u"); sess.dump(2.0)
    while hp(sess) < until and time.time() < end:
        time.sleep(15)
        show(sess, f"{name}-rest-health", "health", 1.5)
    sess.send("d"); sess.dump(2.0)
    print(f"--- {name} back in the arena (HP={hp(sess)})")

while time.time() < end:
    V.dump(4.0)
    if O: O.dump(4.0)
    newV = V.since(mV); mV = V.mark()
    if O:
        newO = O.since(mO); mO = O.mark()
    else:
        newO = ""
    if newV.strip(): print("[V]", newV.strip())
    if newO.strip(): print("[O]", newO.strip())

    for line in newV.splitlines():
        if " at you" in line:
            key = ("HIT " if re.search(r"for \d+ damage", line) else "MISS/? ")
            tally[key + line.split(":The ")[-1][:40]][0] += 1
            swings_this_target += 1

    if hp(V) <= 14:
        rest(V, "Vexil", 25)
        swings_this_target = 0
        continue
    if O and hp(O) <= 17:
        rest(O, "Oracle", 28)
        continue

    now = time.time()
    if now - last_engage > 25:
        out = show(V, "V-look", "look", 1.5)
        m = re.search(r"Also here: ([^.\n]+)\.", out)
        monsters = []
        if m:
            for n in m.group(1).split(","):
                n = n.strip()
                if n and n[0].islower():          # players are capitalized
                    monsters.append(n)
        if monsters:
            newt = monsters[0].split()[-1]
            if newt != target:
                target, swings_this_target = newt, 0
            if swings_this_target >= 8:
                print(f"--- enough swings from {target!r}; killing it")
                for i in range(4):
                    out = show(V, f"V-kill-{i}", f"c mmis {target}", 6.0)
                    if re.search(r"falls|collapses|dies|You gain \d+ exp", out):
                        break
                target, swings_this_target = None, 0
            else:
                show(V, "V-engage", f"attack {target}", 1.5)
        last_engage = now

print("\n===== SWING TALLY =====")
for k in sorted(tally):
    print(f"{tally[k][0]:3d}  {k}")

# --- clean logout, both, back at safe rooms ---
def logout(sess, name, home_steps):
    for step in home_steps:
        sess.send(step); sess.dump(1.5)
    show(sess, f"{name}-logout-room", "look")
    for attempt in range(4):
        m = sess.mark()
        sess.send("x")
        sess.dump(16.0)
        out = sess.since(m)
        print(f"### {name} exit attempt {attempt}")
        print(out[:600])
        if "saved" in out:
            return True
    return False

# Vexil: arena -> u (Narrow Road) -> e (Narrow Path) -> e (Village Entrance)
#        -> n (Weapons Shop)
okV = logout(V, "Vexil", ["u", "e", "e", "n"])
okO = logout(O, "Oracle", ["u", "w"]) if O else True   # arena -> road -> healer
print(f"LOGOUT: Vexil={'ok' if okV else 'FAILED'} Oracle={'ok' if okO else 'FAILED'}")
