#!/usr/bin/env python3
"""M5: monster attack lines, run 2 — rat/filthbug HIT wording.

Run 1 (oracle_monster_lines.raw) captured the acid slime family:
  hit   "The acid slime whips you with its pseudopod for 9 damage!"
        (+ follow-up "Acid burns you for 1 damage!")
  miss  "The acid slime flails at you!"
  dodge "The acid slime lashes at Vexil, but he dodges out of the way!"
but the run stalled resting Vexil (slimes burst him 29->3). This run:

  - Both sessions (Vexil victim, Oracle observer) are dumped in strict
    interleave; nobody sits unattended in the arena.
  - Recovery is `buy healing` at the Newhaven healer (full heal for
    coppers): arena -> u -> w -> buy healing -> e -> d, ~15 s round trip.
  - Acid slimes are killed on sight with magic missiles (1 mana each);
    rats / filthbugs / kobolds are left alive to swing at Vexil so their
    HIT line (never yet captured with damage) lands on record.

Raw: re/oracle/oracle_monster_lines2.raw (Vexil),
     re/oracle/oracle_monster_lines2_obs.raw (Oracle).
"""
import re, sys, time
from mudlib import Session

def show(sess, tag, cmd, settle=2.0, quiet=False):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    if not quiet:
        print(f"### {tag} ({cmd!r})")
        print(out[:1400]); print()
    return out

def hp(sess):
    ms = re.findall(r"\[HP=(\d+)", sess.clean())
    return int(ms[-1]) if ms else 99

def to_healer_and_back(sess, name):
    print(f"--- {name} healing run (HP={hp(sess)})")
    sess.send("u"); sess.dump(1.5)
    sess.send("w"); sess.dump(1.5)
    show(sess, f"{name}-buy-heal", "buy healing", 2.5)
    sess.send("e"); sess.dump(1.5)
    sess.send("d"); sess.dump(1.5)
    print(f"--- {name} back (HP={hp(sess)})")

def monsters_here(out):
    m = re.search(r"Also here: ([^.\n]+)\.", out)
    if not m:
        return []
    return [n.strip() for n in m.group(1).split(",")
            if n.strip() and n.strip()[0].islower()]

# --- Vexil ---
V = Session(rawfile="../../re/oracle/oracle_monster_lines2.raw")
V.login("Vexil")
V.send("E"); V.dump(4.0)
out = show(V, "V-entry", "look")
if "Weapons Shop" in out:
    for step in ["s", "w", "w"]:
        V.send(step); V.dump(1.5)
elif "Narrow Road" not in out:
    print("Vexil somewhere unexpected; bailing"); sys.exit(1)
# at Narrow Road: heal up first, then down
V.send("w"); V.dump(1.5)
show(V, "V-preheal", "buy healing", 2.5)
V.send("e"); V.dump(1.5)
V.send("d"); V.dump(1.5)
show(V, "V-arena", "look")

# --- Oracle (observer) ---
O = Session(rawfile="../../re/oracle/oracle_monster_lines2_obs.raw")
O.login("Oracle")
O.send("E"); O.dump(4.0)
out = show(O, "O-entry", "look")
if "Healer" not in out:
    print("Oracle not at the Healer; observer skipped"); O = None
else:
    O.send("e"); O.dump(1.5)
    O.send("d"); O.dump(1.5)
    show(O, "O-arena", "look")

# --- main loop ---
end = time.time() + 18 * 60
mV, mO = V.mark(), (O.mark() if O else 0)
last_engage = 0.0
vexil_hits = 0

while time.time() < end:
    V.dump(3.0)
    if O: O.dump(3.0)
    newV = V.since(mV); mV = V.mark()
    newO = O.since(mO) if O else ""
    if O: mO = O.mark()
    if newV.strip(): print("[V]", newV.strip())
    if newO.strip(): print("[O]", newO.strip())
    vexil_hits += len(re.findall(r"at you .*for \d+ damage|you .*for \d+ damage",
                                 newV))

    if hp(V) <= 16:
        to_healer_and_back(V, "Vexil")
        mV = V.mark()
        continue
    if O and hp(O) <= 18:
        to_healer_and_back(O, "Oracle")
        mO = O.mark()
        continue

    now = time.time()
    if now - last_engage > 20:
        out = show(V, "V-look", "look", 1.5)
        mV = V.mark()
        mons = monsters_here(out)
        slimes = [m_ for m_ in mons if "slime" in m_]
        others = [m_ for m_ in mons if "slime" not in m_]
        if slimes:
            show(V, "V-kill-slime", "c mmis slime", 5.0)
            mV = V.mark()
        elif others:
            show(V, "V-engage", f"attack {others[0].split()[-1]}", 1.5)
            mV = V.mark()
        last_engage = now

print(f"\nvexil_hit_lines~={vexil_hits}")

# --- heal, park, logout ---
def logout(sess, name, steps):
    for step in steps:
        sess.send(step); sess.dump(1.5)
    show(sess, f"{name}-logout-room", "look")
    for attempt in range(4):
        m = sess.mark()
        sess.send("x"); sess.dump(16.0)
        out = sess.since(m)
        print(f"### {name} exit {attempt}"); print(out[:500])
        if "saved" in out:
            return True
    return False

# Vexil: u to Narrow Road, w heal, then home e,e,n to Weapons Shop
V.send("u"); V.dump(1.5)
V.send("w"); V.dump(1.5)
show(V, "V-final-heal", "buy healing", 2.5)
okV = logout(V, "Vexil", ["e", "e", "e", "n"])
okO = logout(O, "Oracle", ["u", "w"]) if O else True
print(f"LOGOUT: Vexil={'ok' if okV else 'FAILED'} Oracle={'ok' if okO else 'FAILED'}")
