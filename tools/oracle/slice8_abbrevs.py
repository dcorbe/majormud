"""M7 slice-8 min-abbrev prober + MA swing strings (command.rs 187-289).

Kaimon (Mystic). Every marked verb is probed BARE (no argument) from
1-char prefixes up to the full word in an empty room (15/733 Blank Room)
— a recognized prefix answers with the verb's own syntax/refusal line, an
unrecognized one echoes `You say "<prefix>"`. Bare probes cannot mutate
state (create/join/stock etc. refuse on missing args before any gate).

Then: kai ST line, wealth, hide/search/sneak strings, and a short MA
round (kick/jumpkick/punch swing verbs) at the Small Cavern rats.

Capture: re/oracle/slice8_abbrevs.raw.
"""
import sys, time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_abbrevs.raw"

# verb -> prefixes worth probing (stop once two consecutive recognize;
# probing every length pins the exact minimum).
PROBES = [
    "punch", "backstab", "kick", "jumpkick", "use", "read", "ask",
    "spells", "powers", "set", "sneak", "hide", "rob", "picklock",
    "search", "disarm", "invoke", "inventory",
    "gang", "guild", "create", "join", "invite", "uninvite", "unstock",
    "promote", "demote", "disband", "leave", "stock", "markup", "top",
]


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.0):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    return out


def main():
    s = Session(rawfile=RAW)
    s.login("Kaimon")
    s.enter_game()
    print(s.clean()[-800:], flush=True)

    sec("empty-room setup")
    cmd(s, "/xgoto 733 15")
    print(cmd(s, "look", wait=2.5), flush=True)

    sec("min-abbrev probes")
    results = {}
    for verb in PROBES:
        marks = []
        for n in range(1, len(verb) + 1):
            prefix = verb[:n]
            out = cmd(s, prefix, wait=1.8)
            said = f'You say "{prefix}' in out
            marks.append("S" if said else "V")
            print(f"{verb:<10} {prefix:<10} {'say' if said else 'VERB'}", flush=True)
            # once recognized, later (longer) prefixes are recognized too
            # in a prefix model — but probe them all anyway for evidence.
        results[verb] = "".join(marks)
    print("\nRESULTS " + str(results), flush=True)

    sec("kai st + wealth + stealth strings")
    print(cmd(s, "st", wait=3), flush=True)
    print(cmd(s, "wealth", wait=2.5), flush=True)
    print(cmd(s, "sneak", wait=2.5), flush=True)
    print(cmd(s, "hide", wait=2.5), flush=True)
    print(cmd(s, "search", wait=2.5), flush=True)

    sec("MA swing verbs at the rat cavern")
    cmd(s, "/xgoto 2156 1")
    print(cmd(s, "look", wait=2.5), flush=True)
    for swing in ("kick rat", "jumpkick rat", "punch rat", "kick rat"):
        out = cmd(s, swing, wait=3.5)
        print(out, flush=True)
    print(cmd(s, "br", wait=2.5), flush=True)
    cmd(s, "/xgoto 733 15")

    sec("clean exit")
    s.exit_game()


if __name__ == "__main__":
    main()
