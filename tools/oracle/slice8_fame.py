"""M7 slice-8 follow-up: rob a monster + fame walk to Seedy + guardian
reaction, at the Silvermere spawn band (orc rogue index 5, mode 0).

Oracle. Capture: re/oracle/slice8_fame.raw.
"""
import sys, time, re

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_fame.raw"

# Silvermere street rooms in the index-1..5 spawn band.
SPOTS = [(224, "Town Square"), (301, "Temple Street"), (324, "Intersection")]


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.4):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    print(out, flush=True)
    return out


def find_orc(s, budget=300):
    end = time.time() + budget
    while time.time() < end:
        for room, frag in SPOTS:
            cmd(s, f"/xgoto {room} 1", wait=2)
            out = cmd(s, "look", wait=2.5)
            if "orc rogue" in out:
                print(f"ORC AT {room}", flush=True)
                return True
        time.sleep(15)
        s.dump(2)
    return False


def main():
    s = Session(rawfile=RAW)
    s.login("Oracle")
    s.enter_game()

    sec("hunt an orc rogue in the Silvermere band")
    if not find_orc(s):
        print("NO ORC FOUND — population drained; aborting", flush=True)
        s.exit_game()
        return

    sec("greet + ask (greettxt 31)")
    cmd(s, "ask orc about shit", wait=3)

    sec("rob the orc (monster rob strings)")
    cmd(s, "rob orc", wait=3.5)
    cmd(s, "rob gold from orc", wait=3.5)

    sec("fame walk: initiate x3 with break-off")
    for n in range(3):
        cmd(s, "a orc", wait=3.5)
        cmd(s, "br", wait=3)
        out = cmd(s, "st", wait=3)
        m = re.search(r"Class:.*", out)
        print(f"-- after initiate {n + 1}", flush=True)
        # the orc may retaliate; heal margin is fine at L3 vs a 30-hp orc
        time.sleep(9)
        s.dump(2)
        look = cmd(s, "look", wait=2.5)
        if "orc rogue" not in look:
            print("orc gone (fled/killed) — re-hunt", flush=True)
            if not find_orc(s, budget=120):
                break

    sec("st + who colour (legal-level word)")
    cmd(s, "st", wait=3)

    sec("guardian reaction (walk past a guard while Seedy)")
    cmd(s, "/xgoto 224 1", wait=2)
    cmd(s, "look", wait=3)
    s.dump(8)
    print(s.clean()[-900:], flush=True)

    sec("clean exit")
    s.exit_game()


if __name__ == "__main__":
    main()
