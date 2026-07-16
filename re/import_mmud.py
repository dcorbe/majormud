#!/usr/bin/env python3
"""Import MajorMUD WG3-NT Btrieve data into a modern SQLite database.

Combines:
  - vir_wg.py         : correct Btrieve 6.x record reader (validated).
  - Nightmare-Redux modFieldmaps.bas : authoritative field layout per record type
    (its Add*FieldMap builders sum to the WG3-NT logical record length, so they map
    directly onto the reader's logical records).

Output: one SQLite table per record type, fully named + typed. Self-contained; the
reimplementation reads from this DB, with zero dependency on any legacy tool.
"""
import os
import re
import sqlite3
import struct
import sys

sys.path.insert(0, "/home/daniel/bbs/re")
import vir_wg as V
import rectype                       # authoritative RecType struct definitions

DATA = "/home/daniel/bbs/re/wg_nt_ref/WCCNT8PJ/out/"
OUT  = "/home/daniel/bbs/re/mmud_wgnt.sqlite"

# table name -> (RecType name, .vir filename)
TABLES = {
    "race":    ("RaceRecType",    "wccrace2.vir"),
    "class":   ("ClassRecType",   "wccclas2.vir"),
    "spell":   ("SpellRecType",   "wccspel2.vir"),
    "monster": ("MonsterRecType", "wccknms2.vir"),
    "item":    ("ItemRecType",    "wccitem2.vir"),
    "shop":    ("ShopRecType",    "wccshop2.vir"),
    "room":    ("RoomRecType",    "wccmp002.vir"),
    "message": ("MessageRecType", "wccmsg2.vir"),
    "action":  ("ActionRecType",  "wccacts2.vir"),
}

def decode_field(rec, off, size, typ):
    if typ == "FLD_STRING":
        b = rec[off:off+size]; z = b.find(b"\x00")
        return (b[:z] if z >= 0 else b).decode("latin1").rstrip()
    if typ == "FLD_IEEE":
        if size == 4: return struct.unpack_from("<f", rec, off)[0]
        if size == 8: return struct.unpack_from("<d", rec, off)[0]
    # FLD_INTEGER / FLD_BYTE / FLD_DATE -> signed little-endian int
    return int.from_bytes(rec[off:off+size], "little", signed=True)

def sqltype(typ):
    return "TEXT" if typ == "FLD_STRING" else ("REAL" if typ == "FLD_IEEE" else "INTEGER")

def decode_rec(rec, off, size, st):
    """Decode a field given RecType's sqltype ('TEXT'/'INTEGER'/'REAL') and byte size."""
    if st == "TEXT":
        b = rec[off:off+size]; z = b.find(b"\x00")
        return (b[:z] if z >= 0 else b).decode("latin1").rstrip()
    if st == "REAL":
        return struct.unpack_from("<f" if size == 4 else "<d", rec, off)[0]
    return int.from_bytes(rec[off:off+size], "little", signed=True)

def main():
    if os.path.exists(OUT):
        os.remove(OUT)
    db = sqlite3.connect(OUT)
    for table, (recname, fn) in TABLES.items():
        path = DATA + fn
        if not os.path.exists(path):
            print("  SKIP %-8s (missing %s)" % (table, fn)); continue
        # fields = [(offset, size, sqltype, colname)] from the authoritative RecType struct
        fields = rectype.parse_rectype(recname)
        total = fields[-1][0] + fields[-1][1] if fields else 0
        _, logical, _, recs = V.records(path)
        # Any bytes past `total` are Btrieve record padding (verified all-zero); _raw keeps them.
        tail = logical - total
        cov = "OK" if tail == 0 else "+%d pad" % tail
        coldefs = ", ".join('"%s" %s' % (col, st) for (_, _, st, col) in fields)
        db.execute('DROP TABLE IF EXISTS "%s"' % table)
        db.execute('CREATE TABLE "%s" (%s, "_raw" BLOB)' % (table, coldefs))
        ph = ",".join("?" * (len(fields) + 1))
        rows = 0
        for num, rec in recs:
            if len(rec) < logical:
                continue
            vals = [decode_rec(rec, off, size, st) for (off, size, st, _) in fields]
            vals.append(rec)
            db.execute('INSERT INTO "%s" VALUES (%s)' % (table, ph), vals)
            rows += 1
        db.commit()
        print("  %-8s %-13s %4d cols  %6d rows  [%s]" % (table, fn, len(fields), rows, cov))
    db.close()
    print("Wrote", OUT)

if __name__ == "__main__":
    main()
