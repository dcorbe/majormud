#!/usr/bin/env python3
"""Probe directional word abbreviations. At the Healer (only exit: east),
'There is no exit in that direction!' = verb resolved; 'You say' = not.
No 'q' anywhere near this script."""
from mudlib import Session

sess = Session(rawfile="oracle_directions.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(3.0)

def ensure_healer():
    m = sess.mark()
    sess.send("look"); sess.dump(1.5)
    t = sess.since(m)
    if "Healer" in t:
        return True
    if "Narrow Road" in t:
        sess.send("w"); sess.dump(1.5)
        return ensure_healer()
    if "Arena" in t:
        sess.send("u"); sess.dump(1.5)
        return ensure_healer()
    print("### LOST:", t[:200])
    return False

def probe(cmd, settle=1.5):
    if not ensure_healer():
        return
    m = sess.mark()
    sess.send(cmd); sess.dump(settle)
    out = [l for l in sess.since(m).splitlines() if l.strip()]
    verdict = "?"
    joined = " ".join(out)
    if "no exit in that direction" in joined:
        verdict = "RESOLVED-as-direction"
    elif "You say" in joined:
        verdict = "SAY"
    elif "Healer" in joined or "Obvious exits" in joined:
        verdict = "MOVED-or-look?"
    print(f"{cmd!r:10} -> {verdict:24} | {out[:2]}")

# Non-east direction words (safe at the Healer).
for c in ["no", "nor", "nort", "north",
          "so", "sou", "sout", "south",
          "we", "wes", "west",
          "do", "dow", "down",
          "up",
          "northe", "northeas", "northeast",
          "southw", "southwe", "southwest"]:
    probe(c)

# East family last — these may move (Healer's real exit).
print("--- east family (moves expected if resolved) ---")
probe("ea")
probe("eas")
probe("east")
print("### done")
