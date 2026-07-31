"""Clean a slice-8 raw (ANSI strip + backspace semantics) and print a
command -> first-response-line table. Usage: slice8_analyze.py FILE.raw
"""
import re, sys


def clean(path):
    d = open(path, "rb").read().decode("cp437", "replace")
    d = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", d)
    d = d.replace("\x00", "")
    out = []
    for ch in d:
        if ch == "\x08":
            if out:
                out.pop()
        else:
            out.append(ch)
    return "".join(out)


def main(path):
    text = clean(path)
    # split on the inline status prompt
    parts = re.split(r"\[HP=-?\d+(?:/MA=-?\d+)?\]:\s*", text)
    for p in parts:
        p = p.strip()
        if not p:
            continue
        lines = [l.rstrip() for l in p.splitlines() if l.strip()]
        if not lines:
            continue
        cmd, resp = lines[0], lines[1:3]
        print(f"{cmd!r:<28} -> {' | '.join(resp)!r}")


if __name__ == "__main__":
    main(sys.argv[1])
