"""M7 slice-8 two-character gang program (design doc §Slice 8; gangs.md §10).

Phase 1 (--phase1): Oracle creates, invites; Kaimon joins; roster / top
gangs / broadgang string pins; deed shop LIST (GHouseDeed price); gang
shop LIST (type 0xb, shop 136 at 15/973); .HSE render at 15/850.

Phase 2 (--phase2, after a board restart): both re-login, roster + st —
gang membership, fame, quest state all survived.

Capture: re/oracle/slice8_gang.raw (phase 1) / slice8_gang2.raw (phase 2).
Gang-create gate is exp >= 100000 — granted via /xexp.
"""
import sys, time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

BASE = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/"
GANG = "Slice Eight"


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.4, tag=""):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    print(f"[{tag}] " + out, flush=True)
    return out


def enter(user, raw):
    s = Session(rawfile=raw)
    s.login(user)
    s.enter_game()
    return s


def phase1():
    a = enter("Oracle", BASE + "slice8_gang.raw")
    b = enter("Kaimon", None)  # one raw is enough; A holds the capture

    sec("exp gates")
    cmd(a, "/xexp 150000", tag="A")
    cmd(b, "/xexp 150000", tag="B")

    sec("create + duplicate/refusal probes")
    cmd(a, f"create gang {GANG}", wait=3, tag="A")
    cmd(b, f"create gang {GANG}", wait=3, tag="B")  # dup-name refusal
    cmd(a, "create gang None", wait=3, tag="A")     # literal-None refusal

    sec("invite / join / roster")
    out = cmd(a, "invite Kaimon", wait=3, tag="A")
    if "say" in out or "not" in out.lower():
        cmd(a, "invite Kaimon Sable", wait=3, tag="A")
    b.dump(3)
    print("[B tail] " + b.clean()[-500:], flush=True)
    cmd(b, f"join gang {GANG}", wait=3, tag="B")
    a.dump(3)
    print("[A tail] " + a.clean()[-500:], flush=True)
    # roster/gangs/gang-chat said for a GANGLESS char (abbrevs pass 2) —
    # settle whether membership makes them parse.
    cmd(a, "roster", wait=3, tag="A")
    cmd(a, f"roster {GANG}", wait=3, tag="A")
    cmd(a, "gang roster", wait=3, tag="A")
    cmd(a, "gangs", wait=3, tag="A")
    cmd(a, "gang hello there", wait=3, tag="A")
    cmd(a, "guild hello there", wait=3, tag="A")

    sec("top gangs")
    cmd(a, "top gangs", wait=3, tag="A")
    cmd(a, "top 5 gangs", wait=3, tag="A")

    sec("broadgang (lead-in colour pin)")
    cmd(a, "broadgang hello from slice eight", wait=3, tag="A")
    b.dump(3)
    print("[B gets] " + b.clean()[-500:], flush=True)
    cmd(b, "gangpath test message", wait=3, tag="B")

    sec("deed shop (GHouseDeed price)")
    cmd(a, "/xgoto 732 15", tag="A")
    cmd(a, "look", wait=3, tag="A")
    cmd(a, "list", wait=4, tag="A")
    cmd(a, "buy deed", wait=3, tag="A")  # price/refusal line either way

    sec("gang shop LIST (type 0xb, shop 136)")
    cmd(a, "/xgoto 973 15", tag="A")
    cmd(a, "look", wait=3, tag="A")
    cmd(a, "list", wait=4, tag="A")
    cmd(a, "stock", wait=3, tag="A")     # bare-syntax line
    cmd(a, "markup", wait=3, tag="A")

    sec(".HSE render (15/850, WCC85015.HSE)")
    cmd(a, "/xgoto 850 15", tag="A")
    cmd(a, "look", wait=4, tag="A")

    sec("clean exits (state persists for phase 2)")
    a.exit_game()
    b.exit_game()


def phase2():
    a = enter("Oracle", BASE + "slice8_gang2.raw")
    sec("post-restart: gang + fame + inventory survive")
    cmd(a, "roster", wait=3, tag="A")
    cmd(a, "st", wait=3, tag="A")
    cmd(a, "i", wait=3, tag="A")
    b = enter("Kaimon", None)
    cmd(b, "roster", wait=3, tag="B")
    sec("teardown: leave + disband")
    cmd(b, "leave gang", wait=3, tag="B")
    cmd(a, "disband gang", wait=3, tag="A")
    cmd(a, "roster", wait=3, tag="A")
    a.exit_game()
    b.exit_game()


if __name__ == "__main__":
    if "--phase2" in sys.argv:
        phase2()
    else:
        phase1()
