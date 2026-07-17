#!/usr/bin/env python3
"""M5 slice-4 Task 1: interactive FIFO-driven session for the blur
duration expedition (tick length, refresh semantics, st line).

Unlike the scripted expeditions, this driver takes commands from a FIFO
so the operator can adapt mid-session, and it timestamps every received
line (millisecond wall clock) into an append-only clean log — the tick
measurement IS the timestamp delta between the cast line and the
`The effects of blur wear off.` line.

Commands (one per line into the FIFO):
  send <text>   -> text + CRLF
  send          -> bare CRLF
  raw <escaped> -> unicode-escaped bytes verbatim (e.g. `raw Zinvar\r`)
  quit          -> flush, close raw capture, exit

Raw capture: re/oracle/oracle_blur_duration.raw
Clean log:   <scratch>/blur_clean.log  (lines: "<epoch> RX|PART|TX <text>")
FIFO:        <scratch>/blur_cmd.fifo
"""
import os, re, select, socket, sys, time

BASE = sys.argv[1] if len(sys.argv) > 1 else "."
RAW = "/home/daniel/bbs/re/oracle/oracle_blur_duration.raw"
LOG = f"{BASE}/blur_clean.log"
FIFO = f"{BASE}/blur_cmd.fifo"

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


def clean(raw: bytes) -> str:
    t = ANSI.sub("", raw.decode("cp437", "replace")).replace("\x00", "")
    out = []
    for ch in t:
        if ch == "\x08":
            if out and out[-1] != "\n":
                out.pop()
        else:
            out.append(ch)
    return "".join(out)


def main():
    if os.path.exists(FIFO):
        os.remove(FIFO)
    os.mkfifo(FIFO)
    ffd = os.open(FIFO, os.O_RDWR | os.O_NONBLOCK)  # RDWR: no EOF spin
    sock = socket.create_connection(("127.0.0.1", 2327), timeout=10)
    sock.setblocking(False)
    rawf = open(RAW, "ab")
    logf = open(LOG, "a", buffering=1)

    buf = b""       # raw bytes since connect
    emitted = 0     # chars of clean(buf) already logged
    last_rx = time.time()
    pending = b""   # partial FIFO command

    def emit(force=False, ts=None):
        nonlocal emitted
        c = clean(buf)
        if len(c) < emitted:          # backspace shrank past our offset
            emitted = len(c)
        new = c[emitted:]
        if not new:
            return
        t = ts if ts is not None else time.time()
        nl = new.rfind("\n")
        if nl >= 0:
            for ln in new[: nl + 1].split("\n")[:-1]:
                logf.write(f"{t:.3f} RX {ln}\n")
            emitted += nl + 1
            new = new[nl + 1 :]
        if force and new:
            logf.write(f"{t:.3f} PART {new}\n")
            emitted += len(new)

    logf.write(f"{time.time():.3f} !! driver connected\n")
    while True:
        r, _, _ = select.select([sock, ffd], [], [], 0.5)
        now = time.time()
        if sock in r:
            try:
                data = sock.recv(8192)
            except (BlockingIOError, InterruptedError):
                data = None
            if data == b"":
                emit(force=True)
                logf.write(f"{now:.3f} !! socket closed by peer\n")
                break
            if data:
                buf += data
                rawf.write(data)
                rawf.flush()
                last_rx = now
                emit(ts=now)
        if ffd in r:
            try:
                pending += os.read(ffd, 4096)
            except (BlockingIOError, InterruptedError):
                pass
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                cmd = line.decode("utf-8", "replace").rstrip("\r")
                if cmd == "quit":
                    emit(force=True)
                    logf.write(f"{time.time():.3f} !! driver quit\n")
                    rawf.close()
                    return
                elif cmd.startswith("raw "):
                    payload = (
                        cmd[4:].encode().decode("unicode_escape").encode("latin-1")
                    )
                    sock.sendall(payload)
                    logf.write(f"{time.time():.3f} TXRAW {cmd[4:]}\n")
                elif cmd == "send" or cmd.startswith("send "):
                    payload = cmd[5:] if cmd.startswith("send ") else ""
                    sock.sendall(payload.encode("cp437", "replace") + b"\r\n")
                    logf.write(f"{time.time():.3f} TX {payload}\n")
                elif cmd:
                    logf.write(f"{time.time():.3f} !! unknown cmd: {cmd}\n")
        if buf and (now - last_rx) > 1.0:
            emit(force=True, ts=last_rx)


if __name__ == "__main__":
    main()
