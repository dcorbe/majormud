"""Slice-8 recon: login check, character state, verb-recognition smoke.

No captures written — this is a pre-expedition sanity pass.
"""
import sys

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mudlib import Session


def main():
    s = Session()
    s.login("Oracle")
    s.dump(2)
    m = s.mark()
    s.send("st")
    s.dump(2.5)
    print("=== ST ===")
    print(s.since(m))
    m = s.mark()
    s.send("/xwhere")
    s.dump(2)
    print("=== WHERE ===")
    print(s.since(m))
    m = s.mark()
    s.send("exp")
    s.dump(2)
    print("=== EXP ===")
    print(s.since(m))
    s.send("=x")  # exit game cleanly
    s.dump(3)


if __name__ == "__main__":
    main()
