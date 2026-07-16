#!/usr/bin/env python3
"""Complete M1 oracle capture: finish character creation (waiting out name
validation), then record gold transcripts for room display, movement,
errors, and quit."""
import time, sys, json
from mudlib import Session

OUT = "oracle_m1"

sess = Session(rawfile=f"{OUT}.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(2.0)

tail = sess.clean()[-400:]
if "choose your race" in tail or "choose a race" in tail:
    print("creating character...")
    sess.send("2")                    # Dwarf
    sess.ru("choose your class")
    sess.send("1")                    # Warrior
    sess.ru("Do you want to be Lawful?")
    sess.send("No")
    sess.ru("CP Left")
    sess.dump(2.0)
    sess.s.sendall(b"\r")
    time.sleep(0.3)
    sess.s.sendall(b"Delver\r")
    time.sleep(0.3)
    for i in range(14):
        sess.s.sendall(b"\r")
        time.sleep(0.25)
    sess.ru("Validating your name", timeout=30)
    print("waiting out name validation (up to 20 min)...")
    deadline = time.time() + 1200
    m = sess.mark()
    while time.time() < deadline:
        sess.dump(5.0)
        t = sess.since(m)
        if "Obvious exits" in t or "entered the Realm" in t or "[HP=" in t:
            print("validation complete!")
            break
    else:
        print("TIMED OUT waiting for validation; tail:")
        print(sess.since(m)[-600:].replace("\\", "")[-300:])
        sys.exit(1)

sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-1800:])

results = {}

def capture(label, line, settle=2.5):
    m = sess.mark()
    sess.send(line)
    sess.dump(settle)
    results[label] = sess.since(m)
    print(f"=== {label} ({line!r}) ===")
    print(results[label][:1200])

capture("look", "look")
capture("move_n", "n")
capture("move_back_s", "s")
capture("bad_direction", "u")
capture("unknown_command", "xyzzy")
capture("blank", "")
capture("quit", "x", settle=4.0)

with open(f"{OUT}_sections.json", "w") as f:
    json.dump(results, f, indent=1)
print("saved", f"{OUT}_sections.json")
