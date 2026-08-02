#!/usr/bin/env python3
"""Render an ASCII/ANSI map of the Silvermere Slums from the WG3-NT room table.

Geometry is derived, not drawn: rooms are laid out by walking the exit graph
from the Slum Entrance (#1072) and stepping one grid cell per compass exit.
The walk closes with zero coordinate conflicts over 160 rooms, so the grid is
the game's own geometry.

Cell -> 2 columns x 1 row.  Open street = blank, building mass = block fill.
"""
import sqlite3
from collections import deque

DB = "/home/daniel/bbs/re/mmud_wgnt.sqlite"
DXY = {0: (0, -1), 1: (0, 1), 2: (1, 0), 3: (-1, 0),
       4: (1, -1), 5: (-1, -1), 6: (1, 1), 7: (-1, 1)}
START = 1072
# unreachable dev leftovers (nothing exits into them) and a same-named alley
# that belongs to Brass Street, not the slums
EXCLUDE = {69, 383, 1311, 2332}

# room -> (legend number, label).  The number is stamped on the slum-side room
# that opens onto the feature.
POI = {
    1072: (1,  "Slum Gates (Noble St.)"),
    1075: (2,  "Unholy Spell Shop"),
    1187: (3,  "Grungy Shop (Slum Store)"),
    1188: (4,  "Training Room"),
    1182: (5,  "Strange Mansion"),
    1224: (6,  "Black House"),
    1169: (7,  "Overgrown Garden"),
    1104: (8,  "Warehouse Door"),
    1119: (9,  "Abandoned Warehouse"),
    1173: (10, "Collapsed Building"),
    1098: (11, "Western Road out"),
    1139: (12, "Small Dirt Path"),
}
SEWER = {1084, 1096, 1103, 1143, 1147, 1190, 1223}
SHOP_ROOM = 2324          # Grungy Shop interior — a building, not a street cell


def load():
    db = sqlite3.connect(DB)
    db.row_factory = sqlite3.Row
    rooms = {}
    for r in db.execute("select * from room where mapnumber=1"):
        ex = {}
        for d in range(10):
            dest, ty = r[f"roomexit_{d+1}"], r[f"roomtype_{d+1}"]
            if dest and dest > 0:
                ex[d] = ((r[f"para1_{d+1}"] if ty == 8 else 1), dest)
        rooms[r["roomnumber"]] = dict(name=r["name"], ex=ex, shop=r["shopnum"])
    return rooms


def layout(rooms):
    region = {n for n, v in rooms.items()
              if ("Slum" in v["name"] or v["name"] in ("Grungy Shop", "Dark Alley"))
              and n not in EXCLUDE}
    pos, q = {START: (0, 0)}, deque([START])
    while q:
        n = q.popleft()
        x, y = pos[n]
        for d, (dm, dst) in rooms[n]["ex"].items():
            if d > 7 or dm != 1 or dst not in region or dst in pos:
                continue
            dx, dy = DXY[d]
            pos[dst] = (x + dx, y + dy)
            q.append(dst)
    return pos


# colour class -> ANSI SGR
SGR = {"wall": "1;30", "poi": "1;33", "sewer": "0;32", "alley": "0;36",
       "street": "0", "title": "1;36", "box": "0;34", "item": "0;35",
       "key": "1;37"}


def render(rooms, pos):
    """Return the map as rows of (text, colour-class) segments."""
    inv = {v: k for k, v in pos.items()}
    xs = [p[0] for p in pos.values()]
    ys = [p[1] for p in pos.values()]
    X0, X1, Y0, Y1 = min(xs), max(xs), min(ys), max(ys)
    W = (X1 - X0 + 1) * 2

    grid = []
    for y in range(Y0, Y1 + 1):
        row = [("██", "wall")]
        for x in range(X0, X1 + 1):
            n = inv.get((x, y))
            if n is None or n == SHOP_ROOM:
                row.append(("██", "wall"))              # building mass
            elif n in POI:
                row.append((f"{POI[n][0]:>2}", "poi"))
            elif n in SEWER:
                row.append(("≡ ", "sewer"))
            elif rooms[n]["name"] == "Dark Alley":
                row.append(("· ", "alley"))
            else:
                row.append(("  ", "street"))            # open street
        row.append(("██", "wall"))
        grid.append(row)

    title = "T h e   S l u m s"
    pad = (W - len(title)) // 2
    out = [[("██", "wall"), (" " * pad, "street"), (title, "title"),
            (" " * (W - pad - len(title)), "street"), ("██", "wall")],
           [("█" * (W + 4), "wall")]]
    out += grid
    out.append([("█" * (W + 4), "wall")])

    IW = 30                                             # legend inner width
    bar = lambda l, r: [(l + "═" * IW + r, "box")]
    def lrow(s, cls="item"):
        return [("║", "box"), (s.ljust(IW), cls), ("║", "box")]
    legend = [bar("╔", "╗"), lrow("   S i l v e r m e r e", "title"),
              bar("╠", "╣")]
    for _, (num, lbl) in sorted(POI.items(), key=lambda kv: kv[1][0]):
        legend.append(lrow(f" {num:>2}. {lbl}"))
    legend += [bar("╠", "╣"),
               lrow("  ≡  sewer access", "key"),
               lrow("  ·  dark alley", "key"),
               lrow("  ██ buildings", "key"),
               bar("╠", "╣"),
               lrow("  north is up", "key"),
               bar("╚", "╝")]

    rows = []
    for i in range(max(len(out), len(legend))):
        left = out[i] if i < len(out) else [(" " * (W + 4), "street")]
        right = legend[i] if i < len(legend) else []
        rows.append(left + [("  ", "street")] + right)
    return rows


def to_text(rows):
    return ["".join(t for t, _ in r).rstrip() for r in rows]


def to_ansi(rows):
    lines = []
    for r in rows:
        s, cur = "", None
        for text, cls in r:
            if not text:
                continue
            if cls != cur:                    # only emit SGR on a class change
                s += "\x1b[%sm" % SGR[cls]
                cur = cls
            s += text
        lines.append(s + "\x1b[0m")
    return lines


if __name__ == "__main__":
    import sys
    rooms = load()
    rows = render(rooms, layout(rooms))
    out = to_ansi(rows) if "--ansi" in sys.argv else to_text(rows)
    for line in out:
        print(line)
