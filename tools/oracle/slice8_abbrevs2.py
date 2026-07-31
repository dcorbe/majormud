"""M7 slice-8 min-abbrev pass 2: argument-bearing probes for the verbs
the DLL's compiled tree matches as multi-token patterns (bare forms fall
to say). Kaimon (Mystic, exp 0, gangless) — the create/leave/gang gates
refuse safely, proving parse without mutating anything.

Capture: re/oracle/slice8_abbrevs2.raw.
"""
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_abbrevs2.raw"

PROBES = [
    # picklock: direction argument (no door in the Blank Room)
    "pi n", "pic n", "pick n", "pickl n", "picklock n", "pick lock n",
    # disarm with argument
    "di n", "dis n", "disarm n",
    # gang chat / guild
    "ga hello", "gan hello", "gang hello", "gu hello", "gui hello",
    "guild hello",
    # create gang (exp-0 gate refuses)
    "cr gang Probe", "cre gang Probe", "crea gang Probe", "create gang Probe",
    # leave gang (gangless refuses)
    "le gang", "lea gang", "leav gang", "leave gang",
    # broadgang / gangpath / roster / gangs
    "broa hello", "broad hello", "broadg hello", "broadgang hello",
    "gangp test", "gangpa test", "gangpath test",
    "ros", "rost", "roste", "roster",
    "gangs",
    # top gangs + the unshipped verbs from pass 1
    "to gangs", "top gangs",
    "r", "re", "rea", "ready",
    "ma", "map",
    "pr", "pro",
]


def cmd(s, line, wait=2.2):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    return s.since(m)


def main():
    s = Session(rawfile=RAW)
    s.login("Kaimon")
    s.enter_game()
    cmd(s, "/xgoto 733 15")
    cmd(s, "look", wait=2.5)

    for p in PROBES:
        out = cmd(s, p)
        first = next((l for l in out.splitlines()[1:] if l.strip()), "")
        print(f"{p!r:<22} -> {first!r}", flush=True)

    # ask with a target present (healer NPC)
    cmd(s, "/xgoto 2190 1")
    cmd(s, "look", wait=2.5)
    for p in ("as healer hello", "ask healer hello"):
        out = cmd(s, p, wait=3)
        first = next((l for l in out.splitlines()[1:] if l.strip()), "")
        print(f"{p!r:<22} -> {first!r}", flush=True)

    s.exit_game()


if __name__ == "__main__":
    main()
