#!/usr/bin/env python3
"""Slice-8 E1: joint (connect, parry) likelihood over every dodge-parry
block — the tool that replaces per-file eyeballing for carry 4b.

The 2026-07-28 re-fit sharpened the acc-mid conflict: on the same 37
swings, to-hit wants true accuracy >= computed+1 while the parry channel
wants <= computed+0. So the fit scans a two-parameter grid:

    delta  — a constant error in OUR accuracy derivation (true = ours + delta)
    w      — a constant offset on the monster defense word
             (true defense = template ac + w; the "word[2] we have not
             traced" hypothesis)

For each (delta, w) it scores every block's connect count as a binomial
under the corrected engine model (strict roll over genrdn(1,100) = [1,99],
clamp over every arm) and every Dodge block's parry count under
min(95, dodge*10 // ((acc)//8)) / 100, and reports the joint
log-likelihood ranking plus a per-block consistency table for the top
candidates.  The kobold pair (a3/a4) varies accuracy against a FIXED
defense, which is what separates delta from w; the b1/b2 parry cliffs
pin delta alone (w never enters the parry channel).

BLOCKS lists every raw by config; missing files are skipped with a note,
so the tool is runnable at any stage of the campaign.

Usage: python3 oracle_accuracy_fit.py           (from tools/oracle/)
"""
import glob
import os
import re
import sys
from math import comb, log

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def resolve_backspaces(text):
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
    return "\n".join(resolve_backspaces(l) for l in text.splitlines())


PROMPT = re.compile(r"\[HP=-?\d+\]:")
RE_HIT = re.compile(r"^You (?:critically )?\w+ .*?\bfor (-?\d+) damage!")
RE_GLANCE = re.compile(r"^Your .*?\bglances off")
RE_DODGE = re.compile(r"^You \w+(?: at)? .*? who dodges your attack!$")
RE_MISS = re.compile(r"^You \w+(?: at)? [^!]*!$")


def census(paths, noun):
    noun_re = re.compile(rf"\b{noun}\b(?! warrior)")
    counts = {"hit": 0, "glance": 0, "dodge": 0, "miss": 0}
    for path in paths:
        text = clean(open(path, "rb").read())
        for physical in text.splitlines():
            for line in PROMPT.split(physical):
                line = line.strip()
                if not line or not noun_re.search(line):
                    continue
                if line.startswith(("The ", "A ", "An ")):
                    continue
                if RE_HIT.match(line):
                    counts["hit"] += 1
                elif RE_GLANCE.match(line):
                    counts["glance"] += 1
                elif RE_DODGE.match(line):
                    counts["dodge"] += 1
                elif RE_MISS.match(line):
                    counts["miss"] += 1
    return counts


def tdiv(a, b):
    q = abs(a) // abs(b)
    return -q if (a < 0) != (b < 0) else q


def p_connect(acc, defense):
    d = tdiv(tdiv(acc * acc, 14), 10)
    t = 5 if d == 0 else 100 - tdiv(defense * defense, d)
    t = min(99, max(10, t))
    return (t - 1) / 99


def p_parry(acc, dodge):
    if dodge == 0:
        return 0.0
    den = acc // 8
    if acc < 9 or den == 0:
        return 0.0
    return min(95, dodge * 10 // den) / 100


def log_binom(k, n, p):
    p = min(max(p, 1e-9), 1 - 1e-9)
    return log(comb(n, k)) + k * log(p) + (n - k) * log(1 - p)


# config -> (glob pattern, noun, computed accuracy, dodge, template ac)
BLOCKS = {
    "acc-high": ("oracle_dodge_parry_acc-high*.raw", "bat", 43, 20, 10),
    "acc-mid":  ("oracle_dodge_parry_acc-mid*.raw",  "bat", 23, 20, 10),
    "control":  ("oracle_dodge_parry_control.raw", "spider", 23,  0, 20),
    "a1":       ("oracle_dodge_parry_a1*.raw",       "bat", 43, 20, 10),
    "a2":       ("oracle_dodge_parry_a2*.raw",       "bat", 33, 20, 10),
    "a3":       ("oracle_dodge_parry_a3*.raw",    "kobold", 39,  0, 30),
    "a4":       ("oracle_dodge_parry_a4*.raw",    "kobold", 43,  0, 30),
    "b1":       ("oracle_dodge_parry_b1*.raw",       "bat", 23, 20, 10),
    "b2":       ("oracle_dodge_parry_b2*.raw",       "bat", 21, 20, 10),
}
ORACLE_DIR = "../../re/oracle"

data = []
for name, (pat, noun, acc, dodge, ac) in BLOCKS.items():
    paths = sorted(glob.glob(os.path.join(ORACLE_DIR, pat)))
    if not paths:
        print(f"[skip] {name}: no raws yet ({pat})")
        continue
    c = census(paths, noun)
    n = sum(c.values())
    connect = c["hit"] + c["glance"] + c["dodge"]
    if n == 0:
        print(f"[skip] {name}: raws present but zero swings parsed")
        continue
    data.append((name, acc, dodge, ac, n, connect, c["dodge"]))
    print(f"[block] {name}: {len(paths)} raw(s), swings {n}, "
          f"connect {connect}/{n}, parried {c['dodge']}/{connect if connect else 1}")

if not data:
    sys.exit("no blocks to fit")

print("\njoint log-likelihood over (delta, w):")
grid = []
for delta in range(-1, 6):
    for w in range(-6, 3):
        ll = 0.0
        for (_, acc, dodge, ac, n, connect, parried) in data:
            pc = p_connect(acc + delta, ac + w)
            ll += log_binom(connect, n, pc)
            if dodge and connect:
                pp = p_parry(acc + delta, dodge)
                ll += log_binom(parried, connect, pp)
        grid.append((ll, delta, w))
grid.sort(reverse=True)
for ll, delta, w in grid[:8]:
    print(f"  delta={delta:+d}  w={w:+d}   logL = {ll:8.2f}")

best = grid[0]
print(f"\nper-block detail at the top candidates:")
for ll, delta, w in grid[:3]:
    print(f"\n  (delta={delta:+d}, w={w:+d}, logL {ll:.2f})")
    for (name, acc, dodge, ac, n, connect, parried) in data:
        pc = p_connect(acc + delta, ac + w)
        line = (f"    {name:9s} connect {connect:3d}/{n:3d} = {connect/n:.3f}"
                f"  model {pc:.3f}")
        if dodge and connect:
            pp = p_parry(acc + delta, dodge)
            line += (f"   parry {parried:3d}/{connect:3d} = {parried/connect:.3f}"
                     f"  model {pp:.3f}")
        print(line)
print("\n(NB: a good w should NOT be trusted from bat blocks alone — only the"
      "\n kobold pair varies accuracy at fixed defense. If no cell fits every"
      "\n block, the fault is formula shape, and the per-block table says"
      "\n which channel breaks where.)")
