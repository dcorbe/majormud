#!/usr/bin/env python3
"""Survey: rooms reachable from Silvermere Docks (1,33) and casting monsters
that can spawn there (region/level match), for the slice-6 oracle expedition."""
import sqlite3
from collections import deque

DB = "/home/daniel/bbs/re/mmud_wgnt.sqlite"
DIRS = ["N","S","E","W","NE","NW","SE","SW","U","D"]
START = (1, 33)
BLOCKED_TYPES = {15}   # level gate — refuses L8 Zinvar
MAXDEPTH = 40

db = sqlite3.connect(DB)
cols = ["mapnumber","roomnumber","name","monstertype","minindex","maxindex","nummons","type"] \
     + ["roomexit_%d"%i for i in range(1,11)] \
     + ["roomtype_%d"%i for i in range(1,11)] \
     + ["para1_%d"%i for i in range(1,11)]
rooms = {}
for row in db.execute("SELECT %s FROM room" % ",".join(cols)):
    mp, rm = row[0], row[1]
    if mp < 1 or mp > 999 or rm < 1: continue
    ex, ty, pa = row[8:18], row[18:28], row[28:38]
    exits = {}
    for d in range(10):
        if ex[d] and ex[d] > 0:
            dmap = pa[d] if ty[d] == 8 else mp
            exits[d] = ((dmap, ex[d]), ty[d])
    rooms[(mp,rm)] = dict(name=row[2], region=row[3], lo=row[4], hi=row[5],
                          nummons=row[6], rtype=row[7], exits=exits)

# BFS
dist = {START: 0}
prev = {}
q = deque([START])
exit_types_seen = {}
while q:
    cur = q.popleft()
    if dist[cur] >= MAXDEPTH: continue
    for d, (dest, t) in rooms[cur]["exits"].items():
        exit_types_seen[t] = exit_types_seen.get(t, 0) + 1
        if t in BLOCKED_TYPES: continue
        if dest in rooms and dest not in dist:
            dist[dest] = dist[cur] + 1
            prev[dest] = (cur, DIRS[d], t)
            q.append(dest)

print("reachable rooms (<=%d steps): %d" % (MAXDEPTH, len(dist)))
print("exit types seen:", sorted(exit_types_seen.items()))

# regions reachable
regions = {}
for k, dd in dist.items():
    r = rooms[k]
    if r["region"]:
        key = (r["region"], r["lo"], r["hi"])
        if key not in regions or dd < regions[key][0]:
            regions[key] = (dd, k, r["name"])
print("\nspawn regions reachable (region, lvl lo-hi) -> (dist, nearest room, name):")
for key, v in sorted(regions.items(), key=lambda x: x[1][0]):
    print("  region %3d lvl %2d-%2d  dist %2d  %s %s" % (key[0], key[1], key[2], v[0], v[1], v[2]))

# casters per region
mcols = ["number","name","\"group\"","\"index\"","hitpoints","ac","dr","mr","undead"] \
      + ["attacktype_%d"%i for i in range(1,6)] \
      + ["attackaccuspell_%d"%i for i in range(1,6)] \
      + ["attackper_%d"%i for i in range(1,6)] \
      + ["attackminhcastper_%d"%i for i in range(1,6)] \
      + ["attackmaxhcastlvl_%d"%i for i in range(1,6)]
mons = list(db.execute("SELECT %s FROM monster" % ",".join(mcols)))
print("\ncasting monsters spawnable in reachable regions:")
for m in mons:
    num, name, grp, lvl, hp, ac, dr, mr, und = m[:9]
    at = m[9:14]; sp = m[14:19]; per = m[19:24]; cper = m[24:29]; clvl = m[29:34]
    forms = [(i, sp[i], cper[i], clvl[i]) for i in range(5) if at[i] == 2]
    if not forms: continue
    for (reg, lo, hi), (dd, rk, rn) in sorted(regions.items(), key=lambda x: x[1][0]):
        if grp == reg and lo <= lvl <= hi:
            fdesc = " ".join("spell%d=%d %d%% L%d" % (i+1, s, p, l) for i, s, p, l in forms)
            # melee forms for damage assessment
            melee = [(i, per[i]) for i in range(5) if at[i] == 1]
            print("  #%d %-24s L%-2d hp%-4d ac%-3d dr%-2d mr%-3d region %d dist %d via %s (%s) | %s" %
                  (num, name, lvl, hp, ac, dr, mr, reg, dd, rk, rn, fdesc))

# print path helper for a given room
import sys
def path(k):
    p = []
    while k != START:
        c, d, t = prev[k]
        p.append((d, t, k))
        k = c
    return list(reversed(p))

if len(sys.argv) > 2:
    tgt = (int(sys.argv[1]), int(sys.argv[2]))
    for d, t, k in path(tgt):
        print(d, "type", t, "->", k, rooms[k]["name"])
