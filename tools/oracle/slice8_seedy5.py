"""Seedy final: two tight-flee charges on elite guardsmen (fame 10 -> 30),
then WHO/ST at Seedy + the guardian reaction. Attack and flee inside ~2 s
via host-command goto. Capture: re/oracle/slice8_seedy5.raw."""
import sys, time, re

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_seedy5.raw"
ROOMS = [224, 301, 324]


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.2, pause=1.5):
    m = s.mark()
    s.send(line, pause=pause)
    s.dump(wait)
    out = s.since(m)
    print(out, flush=True)
    return out


def main():
    s = Session(rawfile=RAW)
    s.login("Oracle")
    s.enter_game()
    charges = 0
    tries = 0
    sec("tight-flee charges")
    while charges < 2 and tries < 12:
        tries += 1
        for room in ROOMS:
            cmd(s, f"/xgoto {room} 1", wait=1.8)
            look = cmd(s, "look", wait=2.0)
            if "elite" not in look.lower():
                continue
            out = cmd(s, "a elite", wait=1.2)
            cmd(s, "/xgoto 2190 1", wait=1.6)  # instant unbreakable flee
            cmd(s, "buy healing", wait=2.2)
            if "dark cloud" in out.lower():
                charges += 1
                print(f"** charge {charges}", flush=True)
            time.sleep(3)
            if charges >= 2:
                break
    sec("who + st (Seedy word)")
    cmd(s, "who", wait=3)
    cmd(s, "st", wait=3)
    sec("guardian reaction")
    cmd(s, "/xgoto 301 1", wait=1.8)
    cmd(s, "look", wait=2.5)
    s.dump(8)
    print(s.clean()[-900:], flush=True)
    cmd(s, "/xgoto 2190 1", wait=1.8)
    cmd(s, "buy healing", wait=2.2)
    sec("exit")
    s.exit_game()


if __name__ == "__main__":
    main()
