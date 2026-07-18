#!/usr/bin/env python3
"""Cave-camp babysitter: watches the clean log, auto-reacts.

- BAD spawn (cube/golem/minotaur/ooze/beetle/skeletal/wight/shade/stone) ->
  escape route south to the beach, then exit.
- GOOD spawn (death dog / moaning spirit) -> nuke loop (c lbol <t>), polling
  HP from prompt lines; flees if HP <= 18; exits when target dies or flees.
- Prints major events to stdout for the driver transcript review.
"""
import os, re, sys, time

BASE = sys.argv[1]
LOG = os.path.join(BASE, "kai_clean.log")
FIFO = os.path.join(BASE, "kai_cmd.fifo")
DEADLINE = time.time() + float(sys.argv[2]) if len(sys.argv) > 2 else time.time() + 600

BAD = re.compile(r"(gelatinous cube|stone golem|minotaur|black ooze|tiger beetle|skeletal warrior|wight|shade)", re.I)
GOOD = re.compile(r"(death dog|moaning spirit)", re.I)
HP = re.compile(r"\[HP=(-?\d+)/")

def send(cmd, wait=0.0):
    with open(FIFO, "w") as f:
        f.write("send %s\n" % cmd if cmd else "send\n")
    if wait:
        time.sleep(wait)
    print("%.0f >> %s" % (time.time(), cmd), flush=True)

def tail_new(f):
    return f.read()

def main():
    f = open(LOG)
    f.seek(0, 2)
    last_ping = 0.0
    mode = "camp"
    target = None
    last_cast = 0.0
    hp = 56
    while time.time() < DEADLINE:
        chunk = f.read()
        if chunk:
            for m in HP.finditer(chunk):
                hp = int(m.group(1))
            if mode == "camp":
                mb = BAD.search(chunk)
                mg = GOOD.search(chunk)
                # only react to arrival/presence lines, not our own text
                if mb:
                    print("BAD SPAWN: %s -- fleeing" % mb.group(1), flush=True)
                    for c, w in [("s",1.6),("se",1.6),("s",1.6),("go crack",1.8),("s",1.6)]:
                        send(c, w)
                    print("fled to beach, exiting", flush=True)
                    return
                if mg:
                    target = "dog" if "dog" in mg.group(1).lower() else "spirit"
                    mode = "fight"
                    print("GOOD SPAWN: %s -- engaging" % mg.group(1), flush=True)
            if mode == "fight":
                if re.search(r"(dies!|is dead|falls to the ground.*dead|drops to the ground, dead)", chunk, re.I):
                    print("TARGET DEAD", flush=True)
                    return
                if re.search(r"just (left|fled)", chunk):
                    print("target left room", flush=True)
                    mode = "camp"; target = None
        if mode == "fight":
            if hp <= 18:
                print("HP LOW (%d) fleeing" % hp, flush=True)
                for c, w in [("s",1.6),("se",1.6),("s",1.6),("go crack",1.8),("s",1.6)]:
                    send(c, w)
                return
            if time.time() - last_cast > 5.6:
                send("c lbol %s" % target)
                last_cast = time.time()
        else:
            if time.time() - last_ping > 6:
                send("")          # bare CR keeps room lines flowing
                last_ping = time.time()
        time.sleep(0.7)
    print("deadline reached", flush=True)

main()
