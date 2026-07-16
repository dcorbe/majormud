#!/usr/bin/env python3
"""Oracle: attack-syntax permutations against the arena kobold.

Between engagements we flee upward; if Oracle dies we re-navigate from
the Healer (e, d). Prints a labeled transcript per permutation.
"""
import time
from mudlib import Session

sess = Session(rawfile="oracle_attack_syntax.raw")
sess.login("Oracle")
sess.send("E")
sess.dump(3.0)

def where():
    m = sess.mark()
    sess.send("look"); sess.dump(1.5)
    return sess.since(m)

def to_arena():
    t = where()
    for _ in range(4):
        if "Arena" in t:
            return True
        if "Healer" in t:
            sess.send("e"); sess.dump(1.2)
            sess.send("d"); sess.dump(1.2)
        elif "Narrow Road" in t:
            sess.send("d"); sess.dump(1.2)
        else:
            return False
        t = where()
    return "Arena" in t

def attempt(label, cmd, settle=4.0):
    if not to_arena():
        print(f"### {label}: NAVIGATION LOST"); return
    m = sess.mark()
    sess.send(cmd); sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:600])
    print()
    # Disengage: flee upward (or wait out a death/respawn).
    if "miracle" in out:
        sess.dump(3.0)
        return
    sess.send("u"); sess.dump(2.0)
    tail = sess.clean()[-300:]
    if "mortally wounded" in tail:
        # Wait to die and respawn rather than hang.
        deadline = time.time() + 240
        while time.time() < deadline:
            sess.dump(5.0)
            if "miracle" in sess.clean()[-500:]:
                break

attempt("bare-a-auto-pick", "a")
attempt("full-name", "a kobold thief")
attempt("partial-single", "a kob")
attempt("at-prefix", "at thief")
attempt("att-prefix", "att kobold")
attempt("attack-bare", "attack")
attempt("unresolvable", "a gnome", settle=2.5)

# --- ambiguous verb prefixes (which command wins?) ---
def probe(label, cmd, settle=2.5):
    m = sess.mark()
    sess.send(cmd); sess.dump(settle)
    print(f"### probe {label} ({cmd!r})")
    print(sess.since(m)[:500])
    print()

# Get somewhere safe first (up out of the arena if possible).
sess.send("u"); sess.dump(2.0)
probe("st", "st")           # status vs stat vs stealth?
probe("ex", "ex")           # exp vs exit?
probe("he", "he")           # health vs help?
probe("t", "t")             # train vs telepath vs top?
probe("tr", "tr")           # train vs track?
probe("h", "h")             # health vs help vs hide?
probe("lo", "lo")           # look?
probe("q", "q")             # quit?
print("### done")
