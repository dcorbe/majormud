"""Gang teardown: top-gangs shows the surviving record, the leader
disbands, the table empties. Capture: re/oracle/slice8_gang3.raw."""
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

BASE = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/"


def cmd(s, line, wait=2.6):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    print(s.since(m), flush=True)


def main():
    b = Session(rawfile=BASE + "slice8_gang3.raw")
    b.login("Kaimon")
    b.enter_game()
    cmd(b, "top gangs", wait=3)  # the record survived the restart
    cmd(b, "disband gang", wait=3)
    cmd(b, "top gangs", wait=3)  # emptied
    cmd(b, "leave gang", wait=3)  # gangless refusal again
    b.exit_game()


if __name__ == "__main__":
    main()
