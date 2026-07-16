#!/usr/bin/env python3
"""Room graph from the WG3-NT SQLite `room` table.

Key = (mapnumber, roomnumber). Direction d (0..9) = N,S,E,W,NE,NW,SE,SW,U,D.
Exit d: dest room = roomexit_<d+1> (>0); dest map = para1_<d+1> when roomtype_<d+1>==8
(map-change portal) else the room's own mapnumber.
"""
import sqlite3
from collections import deque

DB = "/home/daniel/bbs/re/mmud_wgnt.sqlite"
DIRS = ["N","S","E","W","NE","NW","SE","SW","U","D"]
OPP  = {0:1,1:0,2:3,3:2,4:7,7:4,5:6,6:5,8:9,9:8}

def load():
    db = sqlite3.connect(DB)
    cols = ["mapnumber","roomnumber","name"] \
         + ["roomexit_%d"%i for i in range(1,11)] \
         + ["roomtype_%d"%i for i in range(1,11)] \
         + ["para1_%d"%i    for i in range(1,11)]
    rooms = {}
    for row in db.execute("SELECT %s FROM room" % ",".join(cols)):
        mp, rm, name = row[0], row[1], row[2]
        ex = row[3:13]; ty = row[13:23]; pa = row[23:33]
        if mp < 1 or mp > 999 or rm < 1:
            continue                              # skip placeholders
        exits = {}
        for d in range(10):
            dest = ex[d]
            if dest is None or dest <= 0:
                continue
            dmap = pa[d] if ty[d] == 8 else mp    # type 8 = map change
            exits[d] = {"dest": (dmap, dest), "type": ty[d]}
        rooms[(mp, rm)] = {"name": name, "exits": exits}
    return rooms

def analyze(rooms):
    total = sum(len(r["exits"]) for r in rooms.values())
    resolved = sum(1 for r in rooms.values() for e in r["exits"].values() if e["dest"] in rooms)
    recip = 0
    for k, r in rooms.items():
        for d, e in r["exits"].items():
            dst = rooms.get(e["dest"])
            if dst and OPP[d] in dst["exits"] and dst["exits"][OPP[d]]["dest"] == k:
                recip += 1
    adj = {}
    for k, r in rooms.items():
        adj.setdefault(k, set())
        for e in r["exits"].values():
            if e["dest"] in rooms:
                adj[k].add(e["dest"]); adj.setdefault(e["dest"], set()).add(k)
    seen, comps = set(), []
    for k in rooms:
        if k in seen: continue
        q = deque([k]); seen.add(k); n = 0
        while q:
            x = q.popleft(); n += 1
            for y in adj.get(x, ()):
                if y not in seen: seen.add(y); q.append(y)
        comps.append(n)
    comps.sort(reverse=True)
    return dict(rooms=len(rooms), maps=len(set(m for m,_ in rooms)),
                exits=total, resolved=resolved, resolved_pct=round(100*resolved/total,1),
                reciprocal=recip, reciprocal_pct=round(100*recip/total,1),
                components=len(comps), largest=comps[0], top=comps[:6])

if __name__ == "__main__":
    rooms = load()
    for k, v in analyze(rooms).items():
        print("%-14s %s" % (k, v))
    print("\nStarting area (Map 1, Room 1):")
    r = rooms.get((1,1))
    if r:
        print("  %s" % r["name"])
        for d, e in sorted(r["exits"].items()):
            dst = rooms.get(e["dest"])
            print("    %-3s type=%-2d -> %s %s" % (DIRS[d], e["type"], e["dest"], dst["name"] if dst else "?"))
