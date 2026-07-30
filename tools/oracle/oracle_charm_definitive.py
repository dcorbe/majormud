#!/usr/bin/env python3
"""The definitive charm verification: SUPPRESSION, not arrivals.

Every earlier detector false-positived — attack lines counted as
follows, then pursuit arrivals counted as follows. The unambiguous
signal is the charm's own state write: `mon+0x116` suppresses the pet's
attacks against the owner. Protocol:

  1. Find a room whose Also-here holds EXACTLY ONE kobold (no instance
     ambiguity: 'c char kobold' must hit the one we watch).
  2. Wait for it to attack us (aggression 30 obliges quickly). Its
     attacks prove hostility.
  3. Sing until its attacks CEASE for 25 s (5+ combat rounds). Ceasing
     while we stand in melee range is suppression — the charm landed.
  4. Then, with a CONFIRMED pet: walk 3 rooms (follow wording), the
     strand test (give_up), and the timed expiry (duration + release
     wording), polling every 20 s with the suppression probe: if it
     attacks again, the charm has ended — that timestamp is the expiry
     even if no release line printed.

Usage: python3 oracle_charm_definitive.py [account] [password]
"""
import os
import re
import sys
import time

from mudlib import Session

ACCOUNT = sys.argv[1] if len(sys.argv) > 1 else "Bard"
PASSWORD = sys.argv[2] if len(sys.argv) > 2 else "test123"

RAW = "../../re/oracle/oracle_charm_definitive.raw"
LOG = "../../re/oracle/oracle_charm_definitive_timing.log"
sfx = 2
while os.path.exists(RAW):
    RAW = f"../../re/oracle/oracle_charm_definitive{sfx}.raw"
    LOG = f"../../re/oracle/oracle_charm_definitive{sfx}_timing.log"
    sfx += 1

HEALER = 2190
KOBOLD_ROOMS = [722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732,
                733, 734, 745, 746, 747, 748, 749, 750, 751]

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()


def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line, flush=True)
    log.write(line + "\n")
    log.flush()


def cmd(text, tag=None, pause=1.6, drain=2.0, echo=True):
    mk = sess.mark()
    sess.send(text, pause=pause)
    sess.dump(drain)
    out = sess.since(mk)
    if echo:
        note(f"--- {tag or text} ---")
        for line in out.splitlines():
            if line.strip():
                note(f"    | {line.rstrip()[:160]}")
    return out


def hp():
    m = re.findall(r"\[HP=(-?\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


def mana():
    m = re.findall(r"MA=(\d+)", sess.clean()[-600:])
    return int(m[-1]) if m else None


def wait_mana(need, cap=200):
    end = time.time() + cap
    while (mana() or 0) < need and time.time() < end:
        sess.dump(4.0)
    return (mana() or 0) >= need


def heal_full():
    cmd(f"/xgoto {HEALER} 1", tag="to healer", echo=False)
    cmd("buy healing", tag="heal", drain=2.5, echo=False)
    note(f"HP {hp()} MA {mana()}")


ATTACK_RE = re.compile(r"kobold .*\byou\b.*damage|kobold (?:lunges|swings|stabs) at you",
                       re.IGNORECASE)


def attacked_in(text):
    return [ln.strip() for ln in text.splitlines()
            if ATTACK_RE.search(ln) and not ln.strip().startswith("You")]


def one_kobold_room(start=0):
    for k in range(len(KOBOLD_ROOMS)):
        room = KOBOLD_ROOMS[(start + k) % len(KOBOLD_ROOMS)]
        sess.send(f"/xgoto {room} 6", pause=1.6)
        sess.dump(10.0)
        out = cmd("look", drain=1.8, echo=False)
        j = out.find("Also here")
        if j < 0:
            continue
        here = out[j:out.find("\n", j) + 1]
        if here.count("kobold") == 1 and "warrior" not in here:
            return room
    return None


def suppressed(window=25.0, label=""):
    """True if no kobold attack lands on us within the window."""
    mk = sess.mark()
    end = time.time() + window
    while time.time() < end:
        sess.dump(2.5)
    atk = attacked_in(sess.since(mk))
    for ln in atk[:2]:
        note(f"  {label}ATTACK| {ln[:130]}")
    return not atk


sess.login(ACCOUNT, PASSWORD)
sess.send("E")
sess.dump(5.0)
tail = sess.clean()[-500:]
if "choose" in tail.lower() and "race" in tail.lower():
    sys.exit("account at creation screen")
st = cmd("st", tag="STATS on entry")
lm = re.search(r"Lives/CP:\s*(\d+)", st)
if lm and int(lm.group(1)) <= 3:
    sys.exit(f"only {lm.group(1)} lives — train first")
cmd("/xcash 3000 copper", tag="purse")
heal_full()

room = one_kobold_room()
if room is None:
    sys.exit("no single-kobold room found")
note(f"=== single kobold in room {room}; waiting for it to prove hostility ===")
mk = sess.mark()
hostile = False
end = time.time() + 60
while time.time() < end:
    sess.dump(3.0)
    if attacked_in(sess.since(mk)):
        hostile = True
        break
if not hostile:
    note("it never attacked; poking it once to set the grudge")
    cmd("a kobold", tag="poke", drain=4.0)
note("=== hostility established; singing until its attacks CEASE ===")

charmed_at = None
for attempt in range(30):
    if (hp() or 99) < 30:
        heal_full()
        sess.send(f"/xgoto {room} 6", pause=1.6)
        sess.dump(2.0)
    if not wait_mana(4):
        note("mana stalled")
        break
    out = cmd("c char kobold", tag=f"sing {attempt}", drain=2.0)
    if "resist" in out.lower():
        note("  RESIST")
        continue
    if "You do not see" in out:
        note("  target gone; rehoming")
        room = one_kobold_room()
        if room is None:
            break
        continue
    if suppressed(25.0, "post-sing "):
        charmed_at = time.time()
        note(f"=== CHARM CONFIRMED BY SUPPRESSION (sing {attempt}) ===")
        break
    note("  still attacking — charm did not land")

if charmed_at is None:
    note("NO CONFIRMED CHARM after 30 sings; see the raw")
else:
    # --- follow wording: 3 walked rooms -----------------------------------
    for i in range(3):
        exm = re.search(r"Obvious exits:\s*([a-z]+)", cmd("look", echo=False),
                        re.IGNORECASE)
        d = (exm.group(1).strip().lower() if exm else "n")
        d = {"north": "n", "south": "s", "east": "e", "west": "w"}.get(d, d[:1])
        mk = sess.mark()
        sess.send(d, pause=1.6)
        sess.dump(4.0)
        for ln in sess.since(mk).splitlines():
            if "kobold" in ln and not ln.strip().startswith("You"):
                note(f"  WALK{i}| {ln.strip()[:140]}")
    # --- expiry with the suppression stopwatch ----------------------------
    note("=== expiry watch: suppression probes every ~25s ===")
    while time.time() - charmed_at < 480:
        if not suppressed(25.0):
            note(f"EXPIRY: attacks resumed at +{time.time()-charmed_at:.0f}s "
                 f"from the confirmed charm")
            break
        note(f"  still suppressed at +{time.time()-charmed_at:.0f}s")
    else:
        note("still suppressed at +480s (duration exceeds the window?)")

note("=== definitive capture complete ===")
sess.send(f"/xgoto {HEALER} 1", pause=1.6)
sess.dump(1.5)
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
