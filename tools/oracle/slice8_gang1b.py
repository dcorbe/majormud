"""M7 slice-8 gang program, corrective pass (roles fixed):
Kaimon is the LEADER (created "Slice Eight" in phase 1 — Oracle's /xexp
was level-capped). Gang invite is INVITE MEMBER (plain INVITE proved to
be the party verb). Oracle joins, roster/gang-chat probed AS MEMBERS,
broadgang broadcast captured on the RECEIVER's raw (lead-in colour),
promote/demote, .HSE success render (WCC85015.HSE installed), and a
real picklock attempt at 15/850's closed south door.

Gang survives this pass — phase 2 (post-restart) checks persistence.
Capture: re/oracle/slice8_gang1b.raw (Oracle) + _recv side prints.
"""
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

BASE = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/"


def sec(name):
    print(f"\n===== {name} =====", flush=True)


def cmd(s, line, wait=2.6, tag=""):
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


def main():
    a = enter("Oracle", BASE + "slice8_gang1b.raw")
    b = enter("Kaimon", None)

    sec("invite member / join / joined-announce")
    cmd(b, "invite member Oracle", wait=3, tag="B")
    cmd(b, "inv member Oracle", wait=3, tag="B")  # min-abbrev with pattern
    a.dump(3)
    print("[A sees] " + a.clean()[-400:], flush=True)
    cmd(a, "join gang Slice Eight", wait=3, tag="A")
    b.dump(3)
    print("[B sees] " + b.clean()[-400:], flush=True)

    sec("roster / gangs / gang chat AS MEMBERS")
    for probe in ("roster", "roster Slice Eight", "gangs", "gang hello", "guild hello"):
        cmd(b, probe, wait=3, tag="B")
    cmd(a, "roster", wait=3, tag="A")

    sec("broadgang broadcast (receiver raw carries the colour)")
    cmd(b, "broadgang greetings from the close-out", wait=3, tag="B")
    a.dump(4)
    print("[A recv] " + a.clean()[-400:], flush=True)
    cmd(a, "broadgang reply from oracle", wait=3, tag="A")
    b.dump(4)
    print("[B recv] " + b.clean()[-400:], flush=True)

    sec("promote / demote (Lieutenant strings)")
    cmd(b, "promote Oracle", wait=3, tag="B")
    a.dump(3)
    print("[A sees] " + a.clean()[-400:], flush=True)
    cmd(b, "demote Oracle", wait=3, tag="B")
    a.dump(3)
    print("[A sees] " + a.clean()[-400:], flush=True)
    cmd(b, "uninvite member Kaimon", wait=3, tag="B")  # self-remove refusal?

    sec(".HSE success render (file installed)")
    cmd(a, "/xgoto 850 15", tag="A")
    cmd(a, "look", wait=4, tag="A")

    sec("picklock at a real closed door (south)")
    cmd(a, "open s", wait=3, tag="A")
    cmd(a, "pi s", wait=3.5, tag="A")
    cmd(a, "picklock s", wait=3.5, tag="A")

    sec("clean exits (gang survives for the restart check)")
    a.exit_game()
    b.exit_game()


if __name__ == "__main__":
    main()
