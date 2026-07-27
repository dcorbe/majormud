#!/usr/bin/env python3
"""Offline analyzer for the Dodge-parry expedition raws.

Reads `re/oracle/oracle_dodge_parry_<config>.raw`, classifies every swing the
player made, and compares the observed connect rate against the port's parry
model.  Prints to stdout only -- it never touches the board or the repo.

Channel model (see oracle_dodge_parry.py for why the sandbag hammer makes this
clean).  With a weapon whose damage floors to 0 against the target's DR, every
connecting unparried swing renders as a glance, and -- MEASURED 2026-07-26 --
the board words a parry apart from a plain miss, so each swing falls in
exactly one bucket:

    miss   = the to-hit roll failed              -> (1 - h)
    dodge  = connected but parried  (result 3)   -> h * p
    glance = connected, unparried, 0 damage      -> h * (1 - p)
    hit    = ~0, only crits get through the DR

That makes the parry rate a DIRECT measurement over connecting swings,

    p_hat = dodge / (dodge + glance + hit)

with an exact (Clopper-Pearson) 95% interval and no dependence on the to-hit
model at all.  The to-hit rate is reported alongside as an independent check
against `threshold = clamp(100 - defense^2 / (accuracy^2/14/10), 10, 99)`,
where defense is the target's AC -- the two were entangled before the dodge
line was found, and are not any more.

The verdict lists every candidate step `min(95, dodge*10/d)` for integer
denominators d, so a wrong-but-adjacent accuracy is visible rather than hidden.

Usage: python3 oracle_dodge_stats.py <raw> [--accuracy N] [--dodge N] [--ac N]
"""
import argparse
import re
import sys
from math import comb

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def resolve_backspaces(text):
    """The board injects junk-char + BS pairs inside words (anti-bot).  Apply
    backspace semantics before any matching, or counts drift."""
    out = []
    for ch in text:
        if ch == "\x08":
            if out:
                out.pop()
        else:
            out.append(ch)
    return "".join(out)


def clean(raw_bytes):
    text = raw_bytes.decode("cp437", "replace")
    text = ANSI.sub("", text).replace("\x00", "")
    # Resolve per line so a backspace cannot eat across a newline.
    return "\n".join(resolve_backspaces(l) for l in text.splitlines())


def clopper_pearson(k, n, alpha=0.05):
    """Exact binomial interval by bisection on the Beta/binomial tail."""
    if n == 0:
        return (0.0, 1.0)

    def binom_cdf(p, k, n):
        return sum(comb(n, i) * p**i * (1 - p)**(n - i) for i in range(k + 1))

    def solve(target, lo_side):
        lo, hi = 0.0, 1.0
        for _ in range(200):
            mid = (lo + hi) / 2
            if lo_side:                      # P(X >= k) = alpha/2
                val = 1 - binom_cdf(mid, k - 1, n) if k > 0 else 1.0
                if val < target:
                    lo = mid
                else:
                    hi = mid
            else:                            # P(X <= k) = alpha/2
                val = binom_cdf(mid, k, n)
                if val > target:
                    lo = mid
                else:
                    hi = mid
        return (lo + hi) / 2

    low = 0.0 if k == 0 else solve(alpha / 2, True)
    high = 1.0 if k == n else solve(alpha / 2, False)
    return (low, high)


def to_hit(accuracy, defense):
    """The port's to-hit threshold, integer arithmetic throughout."""
    den = accuracy * accuracy // 14 // 10
    if den == 0:
        return 5 / 100
    return min(max(100 - defense * defense // den, 10), 99) / 100


ap = argparse.ArgumentParser()
# Several raws POOL into one estimate, which is not a convenience: a block run
# at one accuracy is often split across raws by a death or an abort (acc-mid is
# 25 swings and acc-mid2 is 12, and only the pooled 37 says anything), and
# summing them by hand is how the design doc came to quote a connect rate no
# single file supports.  Pool only raws that share a configuration.
ap.add_argument("raw", nargs="+")
ap.add_argument("--noun", default="bat", help="target noun in the swing lines")
ap.add_argument("--accuracy", type=int, default=41)
ap.add_argument("--dodge", type=int, default=20)
ap.add_argument("--ac", type=int, default=10, help="target armour class")
args = ap.parse_args()

text = "\n".join(clean(open(p, "rb").read()) for p in args.raw)

RE_HIT = re.compile(r"^You (?:critically )?(\w+) (.*?) for (-?\d+) damage!$")
RE_GLANCE = re.compile(r"^Your (.*?) glances off(.*)$")
# MEASURED 2026-07-26: the board DOES word result 3 apart from a plain miss --
# "You swing at giant bat who dodges your attack!".  This must be tested
# before RE_MISS, whose `[^!]*` would otherwise swallow it.
RE_DODGE = re.compile(r"^You (\w+(?: at)?) (.*?) who dodges your attack!$")
RE_MISS = re.compile(r"^You (\w+(?: at)?) ([^!]*)!$")
SKIP = ("Also here", "just arrived", "just left", "wanders", "into the room",
        "is dead", "You killed", "experience", "You notice", "sysop summon",
        "You are now", "You say")
# The echoed attack command ("a bat") is not a swing.
ECHO = re.compile(r"^a(?:tt?a?c?k?)? \w+$", re.I)

counts = {"hit": 0, "glance": 0, "dodge": 0, "miss": 0, "novel": 0}
novel_lines = []
# The status prompt is written inline ahead of game text, and several game
# lines can share one physical line: "[HP=22]:You punch giant rat for 5
# damage!".  Split on the prompt so each segment is one logical line.
PROMPT = re.compile(r"\[HP=-?\d+\]:")
# Word-boundary, or prose swallows swings ("prepa*rat*ions" matched "rat").
NOUN_RE = re.compile(rf"\b{re.escape(args.noun)}\b")
for physical in text.splitlines():
    for line in PROMPT.split(physical):
        line = line.strip()
        if not line or not NOUN_RE.search(line):
            continue
        if any(s in line for s in SKIP) or ECHO.match(line):
            continue
        if line.startswith(("The ", "A ", "An ")):
            continue                   # the monster swinging at the player
        if RE_HIT.match(line):
            counts["hit"] += 1
        elif RE_GLANCE.match(line):
            counts["glance"] += 1
        elif RE_DODGE.match(line):
            counts["dodge"] += 1
        elif RE_MISS.match(line):
            counts["miss"] += 1
        else:
            counts["novel"] += 1
            if len(novel_lines) < 25:
                novel_lines.append(line)

n = sum(counts.values())
# A swing that connected: it either landed, glanced off armour, or was
# parried.  Only the plain miss is a to-hit failure.
connect = counts["hit"] + counts["glance"] + counts["dodge"]
print(f"raw          : {', '.join(args.raw)}")
print(f"swings       : {n}")
for k in ("hit", "glance", "dodge", "miss", "novel"):
    share = f"{counts[k]/n:6.3f}" if n else "   n/a"
    print(f"  {k:8s}   : {counts[k]:5d}  {share}")
if novel_lines:
    print("\nUNCLASSIFIED lines (candidate parry wording):")
    for l in novel_lines:
        print(f"  | {l[:150]}")

if not n:
    sys.exit("no swings found")

h = to_hit(args.accuracy, args.ac)
lo, hi = clopper_pearson(connect, n)
print(f"\nto-hit, predicted h (accuracy {args.accuracy} vs AC {args.ac}) = {h:.4f}")
print(f"to-hit, observed        = {connect}/{n} = {connect/n:.4f}"
      f"  95% CI [{lo:.4f}, {hi:.4f}]")

# M7 carry 4b.  `build_monster_defender` feeds the RAW template `ac` column
# into the fighter word the player's own side reaches only after a divide by
# ten.  Defense enters the threshold squared, so a high-AC target at low
# accuracy separates the two readings by most of the range -- which is what
# the grey-spider control run is for.  Printed as candidates against the
# interval, in the same shape as the parry steps below.
print("\nreadings of the target's `ac` column (defense enters squared):")
for label, defense in (("port as written", args.ac),
                       ("template ac / 10", args.ac // 10)):
    cand = to_hit(args.accuracy, defense)
    inside = "  <== consistent" if lo <= cand <= hi else "  excluded"
    print(f"  {label:18s} defense {defense:4d} -> {cand:6.3f}{inside}")
# An accuracy error moves the same prediction, so the two are only separable
# when one reading sits outside the interval and the other inside.
print("  (an accuracy error shifts these too -- see the accuracy sweep)")
for acc in range(max(9, args.accuracy - 16), args.accuracy + 17, 4):
    cand = to_hit(acc, args.ac)
    inside = "  <== consistent" if lo <= cand <= hi else ""
    star = " *ours*" if acc == args.accuracy else ""
    print(f"    accuracy {acc:3d} -> {cand:6.3f}{star}{inside}")

# The board words result 3 distinctly, so the parry rate is measured directly
# among connecting swings -- no reliance on the to-hit model at all.
if connect:
    p_lo, p_hi = clopper_pearson(counts["dodge"], connect)
    p_hat = counts["dodge"] / connect
    print(f"\nPARRY (measured directly, dodge / connecting swings)")
    print(f"  p = {counts['dodge']}/{connect} = {p_hat:.4f}"
          f"  95% CI [{p_lo:.4f}, {p_hi:.4f}]")
else:
    p_hat, p_lo, p_hi = 0.0, 0.0, 1.0
    print("\nno connecting swings; parry undetermined")

if args.dodge == 0:
    # The control targets carry no Dodge(0x22), so every candidate step is 0
    # and the table below says nothing.  Such a run measures to-hit alone --
    # that is the whole point of picking a target without the parry channel.
    print("\ntarget carries no Dodge(0x22); parry table omitted "
          "(this run measures to-hit only)")
    sys.exit(0)

print(f"\ncandidate steps for Dodge {args.dodge} (parry = min(95, dodge*10/d)):")
for d in range(1, 16):
    cand = min(95, args.dodge * 10 // d) / 100
    inside = "  <== consistent" if p_lo <= cand <= p_hi else ""
    star = " *predicted*" if d == args.accuracy // 8 else ""
    print(f"  d={d:2d} (accuracy {8*d}-{8*d+7:3d}) -> {cand:6.3f}{star}{inside}")
null_excluded = not (p_lo <= 0.0 <= p_hi)
print(f"\nno-parry null (p = 0): "
      f"{'EXCLUDED' if null_excluded else 'NOT excluded'} by the CI")
