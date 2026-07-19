#!/usr/bin/env python3
"""M6 arena expedition: fresh Nekojin Mystic vs the living Newhaven Arena.

Rolls a disposable account/character (Mireko/test123, "Mireko Softpaw"),
then captures, with wall-clock timestamps:
  1. Narrow Road parking — the adjacent-room spawn rumbles ("You hear
     movement below you!") and their cadence (the arena is spawn-type 2:
     our port predicts ~89% per 5 s kick toward the player count — but
     with no player IN the arena, phase B never rolls it; only the ~5%
     phase-A neighbour pass reaches it, so rumbles should be sparse).
  2. Arena parking — spawn arrival lines + cadence with a player present
     (phase B: fast), the custom movemsg texts ("sneaks/oozes into the
     room from nowhere" — the arena has no plain compass exit).
  3. A full kobold-thief fight as a mystic: damage line census for the
     20 HP + 2/30 s regen math, dodge traffic, the kill tail (coin drop
     lines, exp), and the immediate replacement spawn.
  4. Departure (flee free-attack window) and a short upstairs park
     (pursuit up the type-0 exit is legal for zone-6 monsters? Narrow
     Road is zone 0 — the leash should HOLD them in the arena).
Babysitter: flees up and logs out if HP drops below 9.
"""
import re, sys, time
from mudlib import Session

RAW = "../../re/oracle/oracle_m6_arena_neko.raw"
LOG = "../../re/oracle/oracle_m6_arena_neko_timing.log"

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()

def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line)
    log.write(line + "\n")
    log.flush()

def watch(seconds, label):
    """Passive capture; timestamp every interesting line as it arrives."""
    note(f"--- watch {label} for {seconds}s ---")
    end = time.time() + seconds
    m = sess.mark()
    while time.time() < end:
        sess.dump(1.0)
        new = sess.since(m)
        m = sess.mark()
        for line in new.splitlines():
            line = line.strip()
            if not line:
                continue
            if any(k in line for k in (
                "into the room", "just left", "just arrived", "hear movement",
                "lunges", "lashes", "punch", "swing", "kick", "chop", "jab",
                "dead", "drop to the ground", "experience", "dodge", "glance",
                "You are", "HP=",
            )):
                note(f"  {line[:150]}")
    return sess.clean()

def hp_now():
    m = re.findall(r"\[HP=(-?\d+)", sess.clean()[-400:])
    return int(m[-1]) if m else None

# --- signup: try login first; "Invalid Credentials" => create via NEW ---
sess.ru("Username:")
sess.send("Mireko")
sess.ru("Password:")
sess.send("test123")
sess.dump(2.5)
if "Invalid Credentials" in sess.clean()[-600:]:
    note("no such account; creating via NEW")
    sess.ru("Username:")
    sess.send("NEW")
    sess.ru("unique Username")
    sess.send("Mireko")
    sess.ru("strong Password:")
    sess.send("test123")
    sess.ru("re-enter your password")
    sess.send("test123")
    sess.ru("e-Mail Address:")
    sess.send("mireko@example.com")
    sess.ru("gender")
    sess.send("F")
else:
    note("account exists; logged in")
sess.ru("Make your selection")
sess.send("A")
sess.ru("[MAJORMUD]:")
sess.send("E")
sess.dump(3.0)

tail = sess.clean()[-3000:]
if "choose a race" in tail:
    m = re.search(r"\[(\d+)\]\s*Nekojin", tail)
    race = m.group(1) if m else "12"
    note(f"race {race} (Nekojin)")
    sess.send(race)
    sess.ru("class")
    sess.dump(2.0)
    m = re.search(r"\[(\d+)\]\s*Mystic", sess.clean()[-3000:])
    klass = m.group(1) if m else "15"
    note(f"class {klass} (Mystic)")
    sess.send(klass)
    sess.ru("Lawful?")
    sess.send("No")
    sess.ru("CP Left")
    sess.dump(2.0)
    sess.s.sendall(b"\r")  # accept first name = Mireko
    time.sleep(0.3)
    sess.s.sendall(b"Softpaw\r")
    time.sleep(0.3)
    for _ in range(20):
        sess.s.sendall(b"\r")
        time.sleep(0.3)
        if "Validating your name" in sess.clean()[-500:]:
            break
        sess.dump(0.3)
    sess.ru("Validating your name", timeout=30)
    note("waiting out name validation (up to 20 min)...")
    deadline = time.time() + 1200
    m = sess.mark()
    while time.time() < deadline:
        sess.dump(5.0)
        t = sess.since(m)
        if "Obvious exits" in t or "[HP=" in t:
            note("validation complete")
            break
    else:
        note("TIMED OUT in validation")
        sys.exit(1)

sess.dump(2.0)
note("=== character sheet ===")
mk = sess.mark()
sess.send("st", pause=1.2)
sess.dump(2.5)
for line in sess.since(mk).splitlines():
    if line.strip():
        note(f"  st| {line.strip()[:120]}")

# --- Narrow Road first: remote rumbles from the arena below ---
sess.send("w", pause=1.5); sess.dump(1.5)
sess.send("w", pause=1.5); sess.dump(1.5)
note("=== parked on Narrow Road (above the arena) ===")
watch(75, "narrow-road rumbles")

# --- descend to the arena ---
sess.send("d", pause=1.5)
sess.dump(2.0)
note("=== in the arena ===")
watch(40, "arena spawn arrivals")

# --- fight whatever is here, preferring the thief ---
tail = sess.clean()[-1200:]
target = "kobold" if "kobold thief" in tail else ("slime" if "slime" in tail else None)
if target is None:
    watch(30, "waiting for a spawn")
    tail = sess.clean()[-1200:]
    target = "kobold" if "kobold thief" in tail else ("slime" if "slime" in tail else None)
if target:
    note(f"=== attacking {target} ===")
    fight_t0 = time.time()
    sess.send(f"a {target}", pause=1.5)
    end = time.time() + 420
    m = sess.mark()
    while time.time() < end:
        sess.dump(1.0)
        new = sess.since(m)
        m = sess.mark()
        for line in new.splitlines():
            line = line.strip()
            if line and any(k in line for k in (
                "punch", "swing", "kick", "chop", "jab", "lunges", "lashes",
                "dead", "drop to the ground", "experience", "dodge",
                "glance", "into the room",
            )):
                note(f"  {line[:150]}")
        if "is dead" in new:
            note(f"KILL after {time.time()-fight_t0:.1f}s")
            break
        hp = hp_now()
        if hp is not None and hp < 9:
            note(f"BABYSITTER: HP {hp}, fleeing")
            sess.send("u", pause=1.0)
            sess.dump(2.0)
            break
else:
    note("nothing spawned to fight")

# --- post-kill: watch the replacement, then depart and test pursuit ---
watch(30, "post-kill replacement")
note("=== departing (free-attack window) ===")
sess.send("u", pause=1.5)
sess.dump(2.0)
watch(30, "upstairs: does anything follow?")

note("=== logout ===")
sess.send("x", pause=1.5)
sess.dump(3.0)
note("done")
log.close()
