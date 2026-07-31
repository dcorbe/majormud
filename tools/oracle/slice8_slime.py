"""M7 slice-8 carry-4d probe: the slime beast's `delay 0` (1/2333).

Question (design doc carry 4d): room delay 0 — global 5-minute default,
or NO delay (near-instant respawn per player lore)? Method: level Oracle
at the Sysop Trainer until the slime beast (170 hp) is killable, then
kill it and stopwatch the respawn with wall-clock timestamps.

Capture: re/oracle/slice8_slime.raw + timestamps on stdout.
"""
import sys, time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session

RAW = __file__.rsplit("/tools/", 1)[0] + "/re/oracle/slice8_slime.raw"


def ts():
    return time.strftime("%H:%M:%S") + f".{int(time.time() * 10) % 10}"


def sec(name):
    print(f"\n===== {name} [{ts()}] =====", flush=True)


def cmd(s, line, wait=2.2):
    m = s.mark()
    s.send(line, pause=1.6)
    s.dump(wait)
    out = s.since(m)
    print(f"[{ts()}] {line}\n{out}", flush=True)
    return out


def main():
    s = Session(rawfile=RAW)
    s.login("Kaimon")
    s.enter_game()

    sec("train up (grants HP + lives) + kit")
    cmd(s, "/xcash 20000 copper")
    cmd(s, "/xexp 600000")
    cmd(s, "/xgoto 289 1")
    for _ in range(10):
        out = cmd(s, "train", wait=3)
        if "you receive training" not in out.lower():
            break
    cmd(s, "/xgoto 2190 1")  # leave the river-pulse room fast, heal
    cmd(s, "buy healing", wait=3)
    cmd(s, "st", wait=3)

    sec("engage the slime beast (1/2333)")
    cmd(s, "/xgoto 2333 1")
    out = cmd(s, "look", wait=3)
    if "slime beast" not in out:
        print(f"[{ts()}] no slime beast present at entry; camping", flush=True)
    kills = 0
    hp_floor_hit = False
    end = time.time() + 900  # 15-minute budget
    engaged = False
    while time.time() < end and kills < 2 and not hp_floor_hit:
        s.dump(4)
        tail = s.clean()[-1200:]
        import re as _re

        hp = _re.findall(r"\[HP=(-?\d+)", tail)
        if hp and int(hp[-1]) < 25:
            print(f"[{ts()}] HP FLOOR ({hp[-1]}) — retreat/heal", flush=True)
            cmd(s, "/xgoto 2190 1")
            cmd(s, "buy healing", wait=3)
            cmd(s, "/xgoto 2333 1")
            engaged = False
            continue
        if "slime beast is dead" in tail.lower() or "the slime beast is dead" in tail.lower():
            kills += 1
            engaged = False
            print(f"[{ts()}] *** KILL #{kills} — stopwatch running for respawn", flush=True)
            # camp and timestamp the respawn
            spawn_end = time.time() + 420
            while time.time() < spawn_end:
                s.dump(3)
                t2 = s.clean()[-400:]
                if "slime beast" in t2 and ("appears" in t2 or "walks in" in t2 or "moves in" in t2):
                    print(f"[{ts()}] *** RESPAWN SEEN", flush=True)
                    break
                look = cmd(s, "look", wait=2.5)
                if "slime beast" in look:
                    print(f"[{ts()}] *** RESPAWN PRESENT (look)", flush=True)
                    break
                time.sleep(12)
            continue
        if not engaged:
            look = cmd(s, "look", wait=2.5)
            if "slime beast" in look:
                cmd(s, "a slime", wait=3)
                engaged = True
            else:
                time.sleep(10)

    sec("done")
    cmd(s, "/xgoto 2190 1")
    cmd(s, "st", wait=3)
    s.exit_game()


if __name__ == "__main__":
    main()
