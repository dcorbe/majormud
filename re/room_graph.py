#!/usr/bin/env python3
"""MajorMUD world-room graph from WCCMP001.VIR.

Record layout confirmed against the open-source Nightmare-Redux field map:
  record starts at page+6 (6-byte data-page header); page size 1536, rec len 1528.
  MapNumber Long@0, RoomNumber Long@4, Name@261, RoomExit[10] Long@824,
  RoomType[10] Int@864, Para1[10] Long@884.
Key = (MapNumber, RoomNumber). Exit d: dest room = RoomExit[d] (>0); dest map =
Para1[d] when RoomType[d]==8 (map-change portal) else the room's own MapNumber.
"""
import struct
from collections import deque

PS, RL, HDR = 1536, 1528, 6
DIRS = ["N","S","E","W","NE","NW","SE","SW","U","D"]
OPP  = {0:1,1:0,2:3,3:2,4:7,7:4,5:6,6:5,8:9,9:8}

def L(b,o): return struct.unpack_from("<i",b,o)[0]
def I(b,o): return struct.unpack_from("<h",b,o)[0]
def cstr(b,o,n):
    s=b[o:o+n]; z=s.find(b"\x00"); return (s[:z] if z>=0 else s).decode("latin1").rstrip()

def load(path="/home/daniel/bbs/tmp/WCCMP001.VIR"):
    data=open(path,"rb").read()
    rooms={}
    for page in range(PS,len(data),PS):
        r=data[page+HDR:page+HDR+RL]
        if len(r)<RL: break
        mp,rm=L(r,0),L(r,4)
        if mp<1 or mp>999 or rm<1:          # filter deleted/placeholder records
            continue
        name=cstr(r,261,53)
        if not name.strip(): continue
        exits={}
        for d in range(10):
            dest=L(r,824+d*4)
            if dest<=0: continue
            etype=I(r,864+d*2)
            dmap = L(r,884+d*4) if etype==8 else mp   # type 8 = map change
            exits[d]={"dest":(dmap,dest),"type":etype}
        rooms[(mp,rm)]=dict(name=name,exits=exits)
    return rooms

def analyze(rooms):
    total=sum(len(r["exits"]) for r in rooms.values())
    resolved=sum(1 for r in rooms.values() for e in r["exits"].values() if e["dest"] in rooms)
    recip=0
    for k,r in rooms.items():
        for d,e in r["exits"].items():
            dst=rooms.get(e["dest"])
            if dst and OPP[d] in dst["exits"] and dst["exits"][OPP[d]]["dest"]==k:
                recip+=1
    # connectivity (undirected over resolved exits)
    adj={}
    for k,r in rooms.items():
        adj.setdefault(k,set())
        for e in r["exits"].values():
            if e["dest"] in rooms:
                adj[k].add(e["dest"]); adj.setdefault(e["dest"],set()).add(k)
    seen=set(); comps=[]
    for k in rooms:
        if k in seen: continue
        q=deque([k]); seen.add(k); n=0
        while q:
            x=q.popleft(); n+=1
            for y in adj.get(x,()):
                if y not in seen: seen.add(y); q.append(y)
        comps.append(n)
    comps.sort(reverse=True)
    return dict(rooms=len(rooms), maps=len(set(m for m,_ in rooms)),
                total_exits=total, resolved=resolved,
                resolved_pct=round(100*resolved/total,1),
                reciprocal=recip, reciprocal_pct=round(100*recip/total,1),
                components=len(comps), largest=comps[0], top=comps[:6])

if __name__=="__main__":
    rooms=load()
    for k,v in analyze(rooms).items(): print("%-16s %s"%(k,v))
    print("\nTown Gates (Map1 Room1) exits:")
    tg=rooms[(1,1)]
    for d,e in sorted(tg["exits"].items()):
        dst=rooms.get(e["dest"])
        print("  %-3s type=%-3d -> %s %s" % (DIRS[d],e["type"],e["dest"],dst["name"] if dst else "?"))
