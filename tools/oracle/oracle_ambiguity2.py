#!/usr/bin/env python3
"""Second ambiguity batch: direction prefixes, q, exi, sta, ai, and
candidate hidden verbs (tel, to, tra, hi, hel, ho)."""
from mudlib import Session

sess = Session(rawfile="oracle_ambiguity2.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(3.0)

def probe(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd); sess.dump(settle)
    out = sess.since(m)
    first = [l for l in out.splitlines() if l.strip()][:3]
    print(f"### {label} ({cmd!r}): {first}")

probe("q", "q")
probe("qu", "qu")
probe("no", "no")          # north vs northeast/northwest?
probe("nor", "nor")
probe("north", "north")    # full word (may move; healer has only east exit)
probe("sou", "sou")
probe("ea", "ea")          # east vs eat? (healer HAS an east exit; may move!)
probe("exi", "exi")
probe("exp", "exp")
probe("sta", "sta")
probe("stat", "stat")
probe("ai", "ai")
probe("tel", "tel")
probe("to", "to")
probe("tra", "tra")
probe("trai", "trai")
probe("hi", "hi")
probe("hel", "hel")
probe("ho", "ho")
probe("g", "g")
probe("ge", "ge")
probe("get", "get")
print("### done")
