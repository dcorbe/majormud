#!/usr/bin/env python3
"""Send commands to a FIFO driver session and print new clean-log lines.
Usage: drive.py <scratchdir> <delay> <cmd1> <cmd2> ...
Each cmd is sent as 'send <cmd>' ('-' = bare CR). Prints log lines added after start.
"""
import sys, time, os

base = sys.argv[1]
delay = float(sys.argv[2])
cmds = sys.argv[3:]
log = os.path.join(base, "kai_clean.log")
fifo = os.path.join(base, "kai_cmd.fifo")
start_size = os.path.getsize(log)
with open(fifo, "w") as f:
    for c in cmds:
        f.write("send\n" if c == "-" else "send %s\n" % c)
        f.flush()
        time.sleep(delay)
time.sleep(1.5)
with open(log) as f:
    f.seek(start_size)
    print(f.read())
