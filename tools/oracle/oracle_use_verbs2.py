#!/usr/bin/env python3
"""M5 slice-2 Task 4 step 0, run 2: `use`/`read` on a truly inert item.

Run 1 (oracle_use_verbs.raw) used a torch, which turned out to have its
own light-source semantics (`You lit the torch.`). This run buys the free
club at the Newhaven Weapons Shop and uses/reads it. Vexil saved out at
the General Store; Weapons Shop = n, e, n from there.
"""
import sys
from mudlib import Session

sess = Session(rawfile="../../re/oracle/oracle_use_verbs2.raw")
sess.login("Vexil")
sess.send("E")
sess.dump(3.0)
print("=== ENTRY ===")
print(sess.clean()[-1200:])

def show(label, cmd, settle=2.0):
    m = sess.mark()
    sess.send(cmd)
    sess.dump(settle)
    out = sess.since(m)
    print(f"### {label} ({cmd!r})")
    print(out[:1500])
    print()
    return out

out = show("where", "look")
if "General Store" not in out:
    print("NOT at the General Store; bailing")
    sys.exit(1)

for step in ["n", "e", "n"]:
    sess.send(step)
    sess.dump(1.5)
out = show("weapons-shop", "look")
if "Weapons Shop" not in out:
    print("NOT at the Weapons Shop; bailing")
    sys.exit(1)

show("buy-club", "buy club")
show("inv-1-own-club", "i")
show("use-club", "use club", 2.5)      # KEY: inert non-LearnSp item
show("read-club", "read club", 2.5)
show("inv-2-after", "i")

print("### done, exiting")
m = sess.mark()
sess.send("x")
sess.dump(10.0)
print(sess.since(m)[:600])
