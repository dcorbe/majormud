"""Shared driver for MBBSEmu MajorMUD oracle sessions."""
import socket, time, re, sys

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")

class Session:
    def __init__(self, host="127.0.0.1", port=2327, rawfile=None):
        self.s = socket.create_connection((host, port), timeout=10)
        self.t = b""
        self.raw = open(rawfile, "wb") if rawfile else None

    def clean(self, b=None):
        data = self.t if b is None else b
        return ANSI.sub("", data.decode("cp437", "replace")).replace("\x00", "")

    def ru(self, needle, timeout=15):
        deadline = time.time() + timeout
        while needle not in self.clean():
            self.s.settimeout(max(0.1, deadline - time.time()))
            try:
                c = self.s.recv(8192)
            except socket.timeout:
                print(f"TIMEOUT at {needle!r}; tail:\n{self.clean()[-2500:]}")
                sys.exit(1)
            if not c:
                print(f"CLOSED at {needle!r}; tail:\n{self.clean()[-2500:]}")
                sys.exit(1)
            self.t += c
            if self.raw:
                self.raw.write(c)

    def send(self, line, pause=0.4):
        time.sleep(pause)
        self.s.sendall(line.encode() + b"\r\n")

    def dump(self, seconds=2.5):
        self.s.settimeout(0.4)
        end = time.time() + seconds
        while time.time() < end:
            try:
                c = self.s.recv(8192)
                if not c:
                    break
                self.t += c
                if self.raw:
                    self.raw.write(c)
            except socket.timeout:
                pass

    def mark(self):
        return len(self.clean())

    def since(self, mark):
        return self.clean()[mark:]

    def login(self, user, password="test123"):
        self.ru("Username:")
        self.send(user)
        self.ru("Password:")
        self.send(password)
        self.ru("Make your selection")
        self.send("A")
        self.ru("[MAJORMUD]:")
