"""Seedy completion: revive Oracle (finished off by the grudged elite),
then charges 2-3 on Dhelvanen with instant /xgoto flees, then WHO (the
legal-level word) and the guardian reaction. Oracle fame is 10 already.

Capture: re/oracle/slice8_seedy3.raw.
"""
import sys, time, re

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_seedy3.raw"


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.4):
    m = s.mark()
    s.send(line, pause=1.5)
    s.dump(wait)
    out = s.since(m)
    print(out, flush=True)
    return out


def hp(s):
    m = re.findall(r"\[HP=(-?\d+)", s.clean()[-500:])
    return int(m[-1]) if m else None


def main():
    s = Session(rawfile=RAW)
    s.login("Oracle")
    s.enter_game()
    cmd(s, "st", wait=2.5)

    if (hp(s) or 1) < 1:
        sec("mortally wounded: walk into the grudge to finish it")
        cmd(s, "/xgoto 224 1", wait=2)
        end = time.time() + 300
        while time.time() < end:
            s.dump(6)
            tail = s.clean()[-700:]
            if "slain" in tail or "You have died" in tail or (hp(s) or -1) > 0:
                print("DEATH/REVIVE:\n" + tail, flush=True)
                break
            # hop between guard rooms to find a finisher
            cmd(s, "/xgoto 301 1", wait=2)
            s.dump(6)
        cmd(s, "st", wait=2.5)

    sec("heal + charges on Dhelvanen (instant host-command flee)")
    cmd(s, "/xcash 2000 copper", wait=2)
    cmd(s, "/xgoto 2190 1", wait=2)
    cmd(s, "buy healing", wait=2.5)
    for n in (2, 3):
        cmd(s, "/xgoto 1008 7", wait=2)
        out = cmd(s, "a dhelvanen", wait=1.6)
        cmd(s, "/xgoto 2190 1", wait=2)  # unbreakable flee
        if "dark cloud" in out.lower():
            print(f"** charge {n}", flush=True)
        cmd(s, "buy healing", wait=2.5)
        time.sleep(4)

    sec("who + st at (expected) Seedy")
    cmd(s, "who", wait=3)
    cmd(s, "st", wait=3)

    sec("guardian reaction while Seedy")
    cmd(s, "/xgoto 301 1", wait=2)
    cmd(s, "look", wait=3)
    s.dump(10)
    print(s.clean()[-900:], flush=True)
    cmd(s, "/xgoto 2190 1", wait=2)

    sec("exit")
    s.exit_game()


if __name__ == "__main__":
    main()
