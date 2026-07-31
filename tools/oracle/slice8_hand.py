"""M7 slice-8 hand-session program, part 1 (design doc §Slice 8).

Oracle (Dwarf Warrior, WCCSYSOP): heal, ANSI toggle, SET WARNING pair,
rob a monster, fame walk to Seedy + guardian reaction, the Dhelvanen
quest step (ask a question + complete a step), orc-rogue walk-in watch.

Capture: re/oracle/slice8_hand.raw. Sections marked on stdout for the
analysis pass. Flood control: >=1.6 s per send.
"""
import sys, time, re

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_hand.raw"


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.2):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    print(out, flush=True)
    return out


def goto(s, room, map_, name_frag):
    out = cmd(s, f"/xgoto {room} {map_}", wait=2.5)
    # /xgoto prints nothing itself; look at the room render.
    out += cmd(s, "look", wait=2.5)
    if name_frag.lower() not in out.lower():
        print(f"!! goto {room} {map_}: expected {name_frag!r}", flush=True)
    return out


def main():
    s = Session(rawfile=RAW)
    s.login("Oracle")
    s.enter_game()
    print(s.clean()[-1200:], flush=True)

    sec("A heal + baseline st")
    cmd(s, "/xcash 4000 copper")
    goto(s, 2190, 1, "Healer")
    cmd(s, "buy healing", wait=3)
    cmd(s, "st", wait=3)

    sec("B ansi toggle probes")
    cmd(s, "set")           # the SET summary list
    cmd(s, "set ansi")      # our M7 server-edge toggle's live counterpart
    cmd(s, "ansi")          # bare form
    cmd(s, "set ansi")      # toggle back if it took

    sec("C set warning pair")
    cmd(s, "set warning off")
    cmd(s, "set warning on")
    cmd(s, "set warning off")

    sec("D rob an orc rogue (Narrow Road)")
    goto(s, 2146, 1, "Narrow Road")
    # find or wait briefly for an orc rogue, then rob it
    for _ in range(6):
        out = cmd(s, "look", wait=2.5)
        if "orc rogue" in out:
            break
        time.sleep(8)
        s.dump(2)
    cmd(s, "rob orc", wait=3)
    cmd(s, "rob gold from orc", wait=3)

    sec("E greet + ask the orc rogue (greettxt 31)")
    cmd(s, "ask orc about shit", wait=3)

    sec("F fame walk to Seedy (attack passive orc x3, disengage)")
    for n in range(3):
        cmd(s, "a orc", wait=3)
        cmd(s, "br", wait=2.5)  # break off
        cmd(s, "st", wait=2.5)
        if n < 2:
            # each initiate-against-passive charges 10; re-passive wait
            time.sleep(10)
            s.dump(2)
    cmd(s, "st", wait=3)

    sec("G guardian reaction while Seedy (gate guard)")
    goto(s, 2151, 1, "Narrow Road")
    cmd(s, "look", wait=3)
    s.dump(6)
    print(s.clean()[-800:], flush=True)

    sec("H Dhelvanen quest: ask a question + complete the step")
    cmd(s, "sysop summon spider silk", wait=4)
    cmd(s, "i", wait=3)
    goto(s, 1008, 7, "Luxurious")
    cmd(s, "ask dhelvanen about help", wait=3)
    cmd(s, "give spider silk to Dhelvanen", wait=4)
    cmd(s, "i", wait=3)

    sec("I orc-rogue walk-in watch (wander lines)")
    goto(s, 2146, 1, "Narrow Road")
    end = time.time() + 180
    seen = False
    while time.time() < end:
        s.dump(5)
        tail = s.clean()[-600:]
        if re.search(r"orc rogue (walks|moves|arrives)", tail):
            print("WALK-IN SEEN:\n" + tail, flush=True)
            seen = True
            break
    if not seen:
        print("no walk-in inside 180 s (population-dependent)", flush=True)

    sec("Z status + clean exit")
    cmd(s, "st", wait=3)
    s.exit_game()


if __name__ == "__main__":
    main()
