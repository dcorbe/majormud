#!/usr/bin/env python3
"""Slice-8 oracle expedition: measure the monster Dodge(0x22) parry on the
PLAYER-attacks-monster path (`re/docs/charm.md` 8.2, "THE BIG ONE").

The port turns a connecting swing into a zero-damage parry with chance
`min(95, parry*10 / (accuracy/8))` (`crates/mud-core/src/combat.rs`), recovered
from the 16-bit disassembly and never checked against a capture.  167 of 1101
templates carry Dodge, so an error there is amplified across a sixth of the
bestiary.

Design
------
Oracle Delver (Dwarf Warrior, Str 50 / Agl 30, class combat factor 6) hunts
giant bats through the 130 map-1 rooms whose group-8 spawn band admits an
index-1 template (the caves under Newhaven).  The two level-1 templates there
are:

    giant bat   #71  AC 10  DR 1  Dodge(0x22) = 20   <- treatment
    grey spider #30  AC 20  DR 2  no Dodge           <- CONTROL (see below)

The deeper rooms also admit higher-index group-8 templates, all of which would
kill this character, so everything but the target is left on sight.

The wielded wooden hammer is 1..1 damage and Str 50 adds no bonus, so against
the bat's DR 1 (and the spider's DR 2) every connect lands at exactly 0 damage.
The target therefore never dies (bar a rare crit) and every connecting,
unparried swing renders as a GLANCE.

MEASURED 2026-07-26: the board words result 3 DISTINCTLY on this path --

    You swing at giant bat who dodges your attack!

against the plain to-hit miss `You swing at giant bat!`.  So the parry rate is
counted directly rather than inferred, and each swing falls in exactly one of:

    miss   = the to-hit roll failed              -> (1 - h)
    dodge  = connected but parried  (result 3)   -> h * p
    glance = connected, unparried, 0 damage      -> h * (1 - p)

Our port renders result 3 as the plain miss (game.rs 9683-9690), which that
capture shows is wrong -- an ORACLE-VERIFY there says exactly this needed a
player-view capture.

Several accuracy configurations put several points on the parry step function
`min(95, dodge*10 / floor(accuracy/8))`, which any rival formula has to
reproduce jointly.  See CONFIGS below for the lever.

Field lessons this script encodes (all learned the hard way, 2026-07-26):
  * Death drops the ENTIRE inventory, so the kit is re-summoned every run.
  * The character's defensive parry is NEGATIVE at Agl 30 / Chm 30, so monsters
    connect ~99% of the time; the kit adds what little Dodge is available
    ungated (displacer fur cloak 5, bone charm 1) plus +30 max HP.
  * Worn AC gives far less damage soak than the port models -- a grey spider
    still bit for 9 through a displayed AC of 13 -- so the character is NOT
    immortal and the run leans on healer cycles instead.
  * `/xgoto` is a host-side write and works even while mortally wounded, which
    makes it the only reliable escape; a walked flee can be broken by the free
    attack a monster gets on movement.

Usage:  python3 oracle_dodge_parry.py <config> [swing-target] [minutes]
"""
import re
import sys
import time

from mudlib import Session

# Every piece is accuracy 0 and ungated (no MinLevel(135), reqstr 0), so the
# attacker accuracy stays exactly computable.  Slots are distinct.
# Slots 4 and 8 are deliberately left EMPTY: the negative-accuracy items that
# set the experiment's accuracy live there (malachite ring slot 4, smoky black
# talisman slot 8), and a worn kit piece in the same slot would displace them.
KIT = [
    "gilded robes",         # slot 11 torso   AC 70          wt 120
    "chain coif",           # slot  2 head    AC 45          wt 160
    "displacer fur cloak",  # slot  7 cloak   AC 10 Dodge 5  wt  80
    "beaded belt",          # slot 10 belt    HP +10         wt  50
    "violet orchid",        # slot 16         HP +10         wt   1
]
WEAPON = "wooden hammer"          # 1..1 damage, accuracy 0, wt 50

# Coins weigh about a third of a unit each, which makes the purse the cleanest
# encumbrance lever available: `/xcash` sets it exactly and nothing else about
# the character changes.  (Discovered by accident -- 4000 copper carried for
# healing money silently pushed a "light" run to 65% encumbrance, i.e. into the
# heavy configuration.)  Copper also pays the healer at 2cp/HP.
# ~600 copper weighs ~200 and funds many full heals at 2cp/HP.
COPPER = 600

# Every map-1 room whose group-8 spawn band admits an index-1 template, i.e.
# everywhere a giant bat can appear.  The 13 webbed rooms alone are far too
# spider-heavy to find one (a 3.5-minute sweep of them turned up zero bats).
ROOMS = [
    1451, 1452, 1453, 1454, 1455, 1456, 1457, 1458, 1459, 1460, 1461, 1462,
    1463, 1464, 1465, 1466, 1467, 1468, 1469, 1470, 1471, 1472, 1473, 1474,
    1475, 1476, 1477, 1478, 1480, 1481, 1482, 1483, 1484, 1485, 1486, 1487,
    1488, 1489, 1490, 1491, 1492, 1493, 1494, 1495, 1496, 1497, 1498, 1499,
    1500, 1501, 1502, 1503, 1504, 1505, 1506, 1507, 1508, 1509, 1510, 1511,
    1512, 1514, 1515, 1516, 1517, 1518, 1519, 1520, 1521, 1522, 1523, 1524,
    1525, 1526, 1527, 1528, 1529, 1530, 1539, 1540, 1560, 1561, 1562, 1563,
    1564, 1565, 1566, 1567, 1568, 1569, 1570, 1571, 1572, 1595, 1596, 1597,
    1598, 1600, 1601, 1602, 1603, 1604, 1606, 1607, 1608, 1609, 1610, 1612,
    1613, 1614, 1615, 1616, 1617, 1618, 1619, 1620, 1621, 1622, 1623, 1624,
    1625, 1626, 1627, 1628, 1629, 1630, 1631, 1632, 1633, 2311,
]
HEALER = 2190                     # Newhaven healer, map 1

# Targets.  The spider is the CONTROL that fixes what the board's
# "... who dodges your attack!" line actually means: it carries no Dodge(0x22)
# at all, so if that line is result 3 (the parry) it must never appear against
# one, whereas if it is really the to-hit miss it should appear at ~30% (AC 20
# against accuracy 43).  Nothing else in the transcript separates those two
# readings.  The hammer floors to 0 damage through the spider's DR 2 as well,
# so it is just as immortal a punching bag as the bat.
#
# The accuracy lever is worn NEGATIVE-accuracy gear, not level: `ratings` is
# the summed accuracy of the weapon and everything worn, `skill = ratings`
# (floored to 1 only when ratings is exactly 0), and accuracy carries
# `2 * (skill/2)`.  Three such items are ungated (no MinLevel) and sit in
# distinct slots, so they stack:
#
#     smoky black talisman  -20  slot 8   wt  50
#     malachite ring        -12  slot 4   wt  15
#     tower shield           -6  slot 12  wt 500  (heavy -- watch the enc band)
#
# That decouples the accuracy the experiment needs from the level the
# character needs to survive, which level 1 could not supply.
CONFIGS = {
    # name        target         Dodge  extra negative-accuracy gear
    "acc-high":  ("giant bat",   20,    []),
    "acc-mid":   ("giant bat",   20,    ["smoky black talisman"]),
    "acc-low":   ("giant bat",   20,    ["smoky black talisman",
                                         "malachite ring"]),
    "control":   ("grey spider",  0,    ["smoky black talisman"]),
}
# accuracy contributions of the extra gear, for the prediction arithmetic
GEAR_ACCURACY = {"smoky black talisman": -20, "malachite ring": -12,
                 "tower shield": -6}
HOSTILES = ("grey spider", "dark cultist", "dark cleric", "dark priest",
            "dark paladin", "hellhound", "cave worm", "tentacled abomination")

if len(sys.argv) < 2 or sys.argv[1] not in CONFIGS:
    sys.exit(f"usage: {sys.argv[0]} <{'|'.join(CONFIGS)}> [swings] [minutes]")
CONFIG = sys.argv[1]
TARGET, TARGET_DODGE, EXTRA = CONFIGS[CONFIG]
NOUN = TARGET.split()[-1]
RATINGS = sum(GEAR_ACCURACY[i] for i in EXTRA)   # every KIT piece is 0
REQUIRE_LIGHT_ENC = True
# Never avoid the thing we came to fight.
AVOID = tuple(h for h in HOSTILES if h != TARGET)
SWING_TARGET = int(sys.argv[2]) if len(sys.argv) > 2 else 400
MINUTES = float(sys.argv[3]) if len(sys.argv) > 3 else 30.0

RAW = f"../../re/oracle/oracle_dodge_parry_{CONFIG}.raw"
LOG = f"../../re/oracle/oracle_dodge_parry_{CONFIG}_timing.log"

sess = Session(rawfile=RAW)
log = open(LOG, "w")
t0 = time.time()


def note(msg):
    line = f"{time.time()-t0:9.3f} {msg}"
    print(line, flush=True)
    log.write(line + "\n")
    log.flush()


def cmd(text, tag=None, pause=1.6, drain=2.0, echo=True):
    """One paced command; >= 1.5 s keeps us under the flood-control cutoff."""
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


def flat(text):
    """Collapse whitespace so item names survive the inventory's line wrap.

    `i` wraps mid-name -- "smoky\nblack talisman (Neck)" -- so a plain
    `"smoky black talisman (" in inv` is False and every membership test
    silently fails. That is how a stale accuracy item stayed worn through a
    config change AND slipped past the guard meant to catch exactly that.
    """
    return " ".join(text.split())


# --- swing classification -------------------------------------------------
# Permissive by design: the point is to discover how the board words a parry,
# so anything naming the target that is not one of the three known shapes is
# flagged NOVEL rather than dropped.  The raw file remains authoritative.
RE_HIT = re.compile(r"^You (?:critically )?\w+ .*?\bfor (-?\d+) damage!")
RE_GLANCE = re.compile(r"^Your .*?\bglances off")
# MEASURED 2026-07-26: the board words result 3 apart from a plain miss --
# "You swing at giant bat who dodges your attack!".  Must be tested BEFORE
# RE_MISS, whose `[^!]*` would otherwise swallow it.
RE_DODGE = re.compile(r"^You \w+(?: at)? .*? who dodges your attack!$")
RE_MISS = re.compile(r"^You \w+(?: at)? [^!]*!$")
# The status prompt is written inline ahead of game text and several game
# lines can share one physical line ("[HP=22]:You punch giant bat!"), so
# split on it; and match the noun on a word boundary, or room prose steals
# swings ("prepa*rat*ions").
PROMPT = re.compile(r"\[HP=-?\d+\]:")
NOUN_RE = re.compile(rf"\b{NOUN}\b")
counts = {"hit": 0, "glance": 0, "dodge": 0, "miss": 0, "novel": 0}


def classify(line):
    if not NOUN_RE.search(line):
        return None
    if line.startswith(("The ", "A ", "An ")):
        return None                     # the monster's own swings at me
    if RE_HIT.match(line):
        return "hit"
    if RE_GLANCE.match(line):
        return "glance"
    if RE_DODGE.match(line):
        return "dodge"
    if RE_MISS.match(line):
        return "miss"
    if any(w in line for w in ("Also here", "just arrived", "just left",
                               "wanders", "into the room", "is dead",
                               "You killed", "experience", "You notice")):
        return None
    return "novel"


def swings():
    return sum(counts.values())


def drain_and_count(seconds=1.0):
    mk = sess.mark()
    sess.dump(seconds)
    new = sess.since(mk)
    for physical in new.splitlines():
        for line in PROMPT.split(physical):
            line = line.strip()
            if not line:
                continue
            kind = classify(line)
            if kind:
                counts[kind] += 1
                note(f"  {'NOVEL' if kind == 'novel' else kind:6s}| {line[:150]}")
    return new


# --- login and staging ----------------------------------------------------
sess.login("Oracle")
sess.send("E")
sess.dump(5.0)
note(f"=== config {CONFIG}: target {TARGET!r} ===")

# Self-recovery.  A mortally wounded character cannot be healed by ANY verb,
# and bleeding out unattended from deep negatives takes the better part of an
# hour, so if a previous run left the character down, walk it into a spider and
# let it die: revival is instant, at full HP, and costs one life (of nine).
if (hp() or 0) < 0:
    note("=== character is down; dying deliberately to revive ===")
    deadline_r = time.time() + 900
    ix = 0
    while (hp() or 0) < 0 and time.time() < deadline_r:
        sess.send(f"/xgoto {[1567, 1570, 1572, 1563][ix % 4]} 1", pause=1.6)
        ix += 1
        sess.dump(1.2)
        out = cmd("look", tag="death room", drain=1.8, echo=False)
        if "grey spider" not in out and "giant bat" not in out:
            continue
        for _ in range(40):
            sess.dump(2.0)
            if (hp() or 0) > 0:
                break
    note(f"=== recovered at HP {hp()} ===")
    sess.dump(2.0)

inv = flat(cmd("i", tag="inventory (pre-staging)"))

# Gear PERSISTS between runs, so a previous config's accuracy item is still
# worn unless it is taken off. That silently turned an "acc-high" run into a
# duplicate of "acc-mid" once -- the talisman was still round the character's
# neck and the accuracy was 23, not the 43 the config assumed.
for item in GEAR_ACCURACY:
    if item not in EXTRA and f"{item} (" in inv:
        cmd(f"remove {item}", tag=f"remove {item}", drain=1.4)

for item in KIT + EXTRA + [WEAPON]:
    if item not in inv:
        cmd(f"sysop summon {item}", tag=f"summon {item}", drain=1.4)
for item in KIT + EXTRA:
    if f"{item} (" not in inv:
        cmd(f"wear {item}", tag=f"wear {item}", drain=1.4)
if f"{WEAPON} (" not in inv:
    cmd(f"wield {WEAPON}", tag=f"wield {WEAPON}", drain=1.4)

# Coins weigh (500 gold = 166), so fix the purse: enough copper to fund healing
# but a known, constant weight contribution.
for denom in ("runic", "platinum", "gold", "silver"):
    cmd(f"/xcash 0 {denom}", tag=f"zero {denom}", drain=1.0)
cmd(f"/xcash {COPPER} copper", tag="purse", drain=1.0)

# The analyzer's inputs: `st` gives Str/Agl/level, `i` gives the encumbrance
# percent that sets `skill`, and both pin the configuration in the transcript.
stats = cmd("st", tag="STATS (accuracy inputs)")
inv = flat(cmd("i", tag="INVENTORY (encumbrance)"))

# Trust the board, not the config: re-derive `ratings` from what is actually
# WORN (an item renders as "<name> (Slot)") and refuse to collect if it
# disagrees with the configuration we think we are running.
worn_ratings = sum(v for item, v in GEAR_ACCURACY.items() if f"{item} (" in inv)
if worn_ratings != RATINGS:
    sys.exit(f"{CONFIG} expects ratings {RATINGS} but the character is wearing "
             f"gear worth {worn_ratings}; staging failed")
m = re.search(r"Encumbrance:\s*(\d+)/(\d+)[^\[]*\[(\d+)%\]", inv)
if not m:
    sys.exit("could not read encumbrance; refusing to run blind")
enc = int(m.group(3))
note(f"ENCUMBRANCE {m.group(1)}/{m.group(2)} = {enc}%")

# The design rests on landing on a known step of floor(accuracy/8), so derive
# the accuracy from what the board actually reports rather than trusting the
# staging -- a stray 4000-copper purse already moved one run from the light
# configuration into the heavy one without a word.
lv = re.search(r"Level:\s*(\d+)", stats)
LEVEL = int(lv.group(1)) if lv else 1
STR_, AGL, COMBAT = 50, 30, 6


def tdiv(a, b):
    """Rust's `/` on integers truncates toward zero; Python's `//` floors.
    The difference is live here -- (Agl-50)/6 is -3 in Rust, -4 in Python."""
    q = abs(a) // abs(b)
    return -q if (a < 0) != (b < 0) else q


# `skill = ratings` unless ratings is exactly 0, where it floors to 1; the
# low-encumbrance bonus is then added on top and vanishes at enc >= 33.
skill = 1 if RATINGS == 0 else RATINGS
if enc < 33:
    skill += 15 - tdiv(enc, 10)


def isqrt(n):
    r = 0
    while (r + 1) * (r + 1) <= n:
        r += 1
    return r


accuracy = (tdiv(STR_ - 50, 3)
            + 2 * ((COMBAT - 1) * isqrt(LEVEL) + 2 * COMBAT
                   + tdiv(LEVEL, 2) + tdiv(skill, 2) - 2)
            + tdiv(AGL - 50, 6))
# `calculate_attack`: accuracy < 9 forces the chance to 0 outright (the floor
# is on the ACCURACY, not on the shifted denominator), and the cap is 95.
den = accuracy // 8
if TARGET_DODGE == 0:
    predicted = 0.0
elif accuracy < 9 or den == 0:
    predicted = 0.0
else:
    predicted = min(95, TARGET_DODGE * 10 // den) / 100
note(f"level {LEVEL}, ratings {RATINGS}, skill {skill}, accuracy {accuracy}, "
     f"floor(acc/8) = {den}, predicted parry {predicted}")
if REQUIRE_LIGHT_ENC and enc >= 33:
    sys.exit(f"{CONFIG} needs enc < 33, got {enc}% -- drop weight")
if not REQUIRE_LIGHT_ENC and enc < 33:
    sys.exit(f"{CONFIG} needs enc >= 33, got {enc}% -- add weight")
maxhp = re.search(r"Hits:\s*(-?\d+)/(\d+)", stats)
MAXHP = int(maxhp.group(2)) if maxhp else 35
# Two floors.  Fighting a bat is cheap (2..5 a bite) so it can run low, but
# SWEEPING is what actually kills this character: every hostile room lands a
# free attack on entry, and a dark cultist hits for 2..10.  Two runs died in
# the sweep, not the fight, so rooms are only entered near full health.
FLOOR = max(14, int(MAXHP * 0.25))
SWEEP_FLOOR = int(MAXHP * 0.70)
note(f"max HP {MAXHP}; fight floor {FLOOR}, sweep floor {SWEEP_FLOOR}")


def heal_cycle():
    note("=== heal cycle ===")
    sess.send(f"/xgoto {HEALER} 1", pause=1.6)
    sess.dump(1.5)
    out = cmd("buy healing", tag="buy healing", drain=2.5)
    h = hp()
    note(f"HP after healing = {h}")
    if "mortally" in out or (h is not None and h <= 0):
        # Nothing recovers a downed character; looping here just burns the
        # clock while the character bleeds toward the -200 threshold.
        note("MORTALLY WOUNDED - healing impossible; aborting the run")
        return None
    return h


# --- hunt -----------------------------------------------------------------
deadline = t0 + MINUTES * 60
room_ix = 0
engaged = False
target_room = None
last_report = 0.0

while swings() < SWING_TARGET and time.time() < deadline:
    h = hp()
    if h is not None and h <= 0:
        note("MORTALLY WOUNDED - aborting the run")
        break
    if h is not None and h < (FLOOR if engaged else SWEEP_FLOOR):
        if heal_cycle() is None:
            break
        # The sandbag hammer cannot kill the bat, so it is still standing
        # where we left it; going back beats re-sweeping, which costs more HP
        # (and heal cycles) in hostile rooms than the fights themselves do.
        if target_room is not None:
            note(f"returning to the {NOUN} in room {target_room}")
            sess.send(f"/xgoto {target_room} 1", pause=1.6)
            sess.dump(1.0)
            back = cmd("look", tag=f"back to {target_room}", drain=1.8, echo=False)
            if TARGET in back and not any(a in back for a in AVOID):
                sess.send(f"a {NOUN}", pause=1.6)
                sess.dump(1.5)
                quiet = 0
                continue
            note("target no longer there; resuming the sweep")
            target_room = None
        engaged = False
        continue

    if not engaged:
        room = ROOMS[room_ix % len(ROOMS)]
        room_ix += 1
        sess.send(f"/xgoto {room} 1", pause=1.6)
        sess.dump(1.0)
        out = cmd("look", tag=f"room {room}", drain=1.8, echo=False)
        hostile = next((a for a in AVOID if a in out), None)
        if hostile:
            note(f"room {room}: {hostile} present - leaving")
            continue
        if TARGET not in out:
            continue
        note(f"=== engaging {TARGET} in room {room} ===")
        target_room = room
        sess.send(f"a {NOUN}", pause=1.6)
        sess.dump(1.5)
        engaged = True
        quiet = 0

    new = drain_and_count(1.0)

    if "is dead" in new or "You killed" in new:
        note(f"KILL at {swings()} swings - relocating")
        target_room = None
        engaged = False
        continue
    intruder = next((a for a in AVOID if a in new), None)
    if intruder:
        note(f"{intruder} arrived - disengaging")
        engaged = False
        continue

    quiet = quiet + 1 if not new.strip() else 0
    if quiet >= 8:
        probe = cmd("look", tag="probe", drain=1.6, echo=False)
        if TARGET not in probe:
            note("target gone - relocating")
            engaged = False
        quiet = 0

    if time.time() - last_report > 60:
        last_report = time.time()
        conn = counts["hit"] + counts["glance"] + counts["dodge"]
        n = swings()
        note(f"PROGRESS {n}/{SWING_TARGET} {counts} "
             f"connect={conn/n:.3f}" if n else "PROGRESS 0")

n = swings()
conn = counts["hit"] + counts["glance"] + counts["dodge"]
note(f"=== DONE: {n} swings {counts} ===")
if n:
    note(f"connect (glance+hit) = {conn}/{n} = {conn/n:.4f}")
cmd("x", tag="logout", drain=4.0)
note("done")
log.close()
