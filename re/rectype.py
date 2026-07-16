#!/usr/bin/env python3
"""Parse Nightmare-Redux `Public Type XxxRecType` VB struct definitions into
(offset, size, sqltype, colname) field lists. These structs are the authoritative,
complete record layouts (the Add*FieldMap builders only expose a prefix).

VB sizes: Integer=2, Long=4, Byte=1, Single=4, Double=8, String*N=N.
Arrays: Field(N) => N+1 elements (VB Dim is 0..N inclusive); Field(A To B) => B-A+1.
"""
import re

SRC = "/home/daniel/bbs/re/Nightmare-Redux/modFieldmaps.bas"
_lines = open(SRC, encoding="latin1").read().splitlines()

VBSIZE = {"integer":2, "long":4, "byte":1, "single":4, "double":8, "currency":8}

def parse_rectype(typename):
    """typename e.g. 'ItemRecType' -> [(offset,size,sqltype,colname), ...]."""
    out, off, inside, seen = [], 0, False, {}
    for ln in _lines:
        s = ln.strip()
        if s.startswith("Public Type " + typename):
            inside = True; continue
        if inside and s.startswith("End Type"):
            break
        if not inside or s.startswith("'") or not s:
            continue
        s = s.split("'", 1)[0].strip()          # drop trailing comment
        # NAME[(dims)]  As  TYPE[* N]
        m = re.match(r"(\w+)\s*(?:\(([^)]*)\))?\s+As\s+(\w+)(?:\s*\*\s*(\d+))?", s, re.I)
        if not m:
            continue
        name, dims, vbtype, strlen = m.group(1), m.group(2), m.group(3).lower(), m.group(4)
        # element size
        if vbtype == "string":
            elem = int(strlen); sqlt = "TEXT"
        elif vbtype in VBSIZE:
            elem = VBSIZE[vbtype]; sqlt = "REAL" if vbtype in ("single","double") else "INTEGER"
        else:
            continue                             # unknown type -> stop cleanly
        # array count
        n = 1
        if dims is not None:
            dm = re.match(r"\s*(\d+)\s+To\s+(\d+)\s*", dims, re.I)
            if dm:
                n = int(dm.group(2)) - int(dm.group(1)) + 1
            else:
                n = int(dims.strip()) + 1        # Dim x(N) => 0..N
        for i in range(n):
            base = re.sub(r"\W+", "_", name.lower()).strip("_")
            col = base if n == 1 else "%s_%d" % (base, i+1)
            if col in seen:
                seen[col] += 1; col = "%s__%d" % (col, seen[col])
            else:
                seen[col] = 0
            # strings are single fields (not arrays of chars); size = elem
            out.append((off, elem, sqlt, col))
            off += elem
    return out

TYPEMAP = {  # table -> RecType name
    "race":"RaceRecType", "class":"ClassRecType", "spell":"SpellRecType",
    "monster":"MonsterRecType", "item":"ItemRecType", "shop":"ShopRecType",
    "room":"RoomRecType", "message":"MessageRecType", "action":"ActionRecType",
}

if __name__ == "__main__":
    import struct
    for tbl, tn in TYPEMAP.items():
        f = parse_rectype(tn)
        total = f[-1][0] + f[-1][1] if f else 0
        print("%-8s %-16s fields=%3d  total=%d"%(tbl,tn,len(f),total))
