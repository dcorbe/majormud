"""M7 slice-8: rob strings + fame walk to Seedy + guardian flip, using
Silvermere guardsmen as the passive targets (orc population drained).

Method: one initiate per FRESH guard (a grudge-holding guard charges
nothing on re-attack), /xgoto flee between rounds (unbreakable), heal at
the Newhaven healer. WHO before/after pins the legal-level word.

Oracle (L3, 53 hp, 3 lives). Capture: re/oracle/slice8_seedy.raw.
"""
import sys, time, re

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_seedy.raw"


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.4):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    print(out, flush=True)
    return out


def heal(s):
    cmd(s, "/xgoto 2190 1", wait=2)
    cmd(s, "buy healing", wait=3)


def hp(s):
    m = re.findall(r"\[HP=(-?\d+)", s.clean()[-400:])
    return int(m[-1]) if m else 999


def main():
    s = Session(rawfile=RAW)
    s.login("Oracle")
    s.enter_game()

    sec("baseline: heal + who (legal-level word absent at Neutral)")
    heal(s)
    cmd(s, "who", wait=3)

    sec("rob a guardsman (thievery 0 — the fail/noticed strings)")
    cmd(s, "/xgoto 224 1", wait=2)
    out = cmd(s, "look", wait=2.5)
    cmd(s, "rob guardsman", wait=3.5)
    cmd(s, "rob gold from guardsman", wait=3.5)
    cmd(s, "st", wait=2.5)

    sec("fame walk: one initiate per fresh guard, flee between")
    stops = [(224, "elite"), (301, "elite"), (301, "templar"), (324, "elite"), (224, "elite")]
    charges = 0
    for room, target in stops:
        if charges >= 3:
            break
        cmd(s, f"/xgoto {room} 1", wait=2)
        look = cmd(s, "look", wait=2.5)
        if target.split()[0] not in look.lower():
            continue
        out = cmd(s, f"a {target}", wait=3.5)
        if "dark cloud" in out.lower():
            charges += 1
            print(f"** charge {charges}", flush=True)
        heal(s)  # flee via /xgoto + heal the return fire
        if hp(s) < 20:
            heal(s)
    cmd(s, "st", wait=3)
    cmd(s, "who", wait=3)

    sec("guardian reaction while Seedy (fresh elite guardsman)")
    cmd(s, "/xgoto 301 1", wait=2)
    cmd(s, "look", wait=3)
    s.dump(10)
    print(s.clean()[-1000:], flush=True)
    heal(s)

    sec("exit")
    cmd(s, "st", wait=3)
    s.exit_game()


if __name__ == "__main__":
    main()
