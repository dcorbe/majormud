#!/usr/bin/env python3
"""Create the Bard account + character for the E4/E5/E6 charm expedition.

MBBSEmu signup: type NEW at the Username prompt (MenuRoutines.cs 186),
then username/password/email prompts. Then into MajorMUD: race menu by
number, CLASS MENU PARSED for 'Bard' (its number varies by listing),
not Lawful, default first name + surname, then the ~20-minute name
validation.

The character trains to TWELVE later (kobold charmlvl 12 — rats, the
charmlvl-1 target, are structurally absent from this board's world).

Usage: python3 oracle_create_bard.py
"""
import re
import sys
import time

from mudlib import Session, MBBS_HOST, MBBS_PORT
import socket

RAW = "../../re/oracle/oracle_create_bard.raw"

sess = Session(rawfile=RAW)

def say(m):
    print(f"{time.strftime('%T')} {m}", flush=True)

# --- signup -----------------------------------------------------------------
sess.ru("Username:")
sess.send("NEW")
sess.dump(3.0)
tail = sess.clean()[-800:]
say("after NEW: " + " ".join(tail[-200:].split()))

# The signup flow prompts in order; answer whatever arrives.
answers = [
    ("sername", "Bard"),          # desired username
    ("assword", "test123"),       # password
    ("assword", "test123"),       # confirm
    ("mail", "bard@example.com"), # email
]
for needle, reply in answers:
    deadline = time.time() + 20
    while needle not in sess.clean()[-300:] and time.time() < deadline:
        sess.dump(1.0)
    if needle not in sess.clean()[-300:]:
        say(f"prompt {needle!r} never arrived; tail: "
            + " ".join(sess.clean()[-200:].split()))
        break
    sess.send(reply)
    sess.dump(2.0)

sess.dump(4.0)
tail = sess.clean()[-600:]
say("post-signup tail: " + " ".join(tail[-250:].split()))

# Some flows drop straight to the main menu; others want a fresh login.
if "Make your selection" not in tail:
    if "Username" in tail:
        sess.send("Bard")
        sess.ru("Password:")
        sess.send("test123")
        sess.ru("Make your selection")

sess.send("E")
sess.dump(5.0)
tail = sess.clean()[-800:]
say("in module: " + " ".join(tail[-250:].split()))

# --- character creation -------------------------------------------------
if "valid race" in tail or "choose your race" in tail or "choose a race" in tail:
    sess.send("?")
    sess.dump(3.0)
    menu = sess.clean()[-1200:]
    say("race menu: " + " ".join(menu[-400:].split()))
    m = re.search(r"(\d+)\s*[).\]]?\s*Human", menu)
    race = m.group(1) if m else "1"
    say(f"race pick: {race}")
    sess.send(race)
    sess.ru("choose your class", timeout=20)
    sess.send("?")
    sess.dump(3.0)
    menu = sess.clean()[-1500:]
    say("class menu: " + " ".join(menu[-500:].split()))
    m = re.search(r"(\d+)\s*[).\]]?\s*Bard", menu)
    if not m:
        sys.exit("no Bard in the class menu — read the raw and adapt")
    say(f"Bard is class {m.group(1)}")
    sess.send(m.group(1))
    sess.ru("Do you want to be Lawful?", timeout=20)
    sess.send("No")                      # E5 needs the evil charge to land
    sess.ru("CP Left", timeout=20)
    sess.dump(2.0)
    sess.s.sendall(b"\r")
    time.sleep(0.3)
    sess.s.sendall(b"Minstrel\r")
    time.sleep(0.3)
    for i in range(14):
        sess.s.sendall(b"\r")
        time.sleep(0.25)
    sess.ru("Validating your name", timeout=40)
    say("waiting out name validation (up to 20 min)...")
    deadline = time.time() + 1200
    mk = sess.mark()
    while time.time() < deadline:
        sess.dump(5.0)
        t = sess.since(mk)
        if "Obvious exits" in t or "entered the Realm" in t or "[HP=" in t:
            say("validation complete!")
            break
    else:
        sys.exit("TIMED OUT waiting for validation")
else:
    say("not at race selection — maybe the character already exists")

sess.dump(3.0)
mk = sess.mark()
sess.send("st", pause=1.6)
sess.dump(2.5)
print(sess.since(mk))
say("BARD CREATED")
