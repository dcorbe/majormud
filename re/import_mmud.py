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

def import_textblocks(db):
    """WCCTEXT2 — the quest/dialogue/name-generator text-block store.

    Unlike every other game file this is a Btrieve 6.x VARIABLE-length record
    file; vir_wg.records() cannot read it. Format (solved empirically 2026-07-19,
    engine cross-ref get_text_block@0x3379c which fetches into a 2024-byte buffer
    keyed on seq@0 + block id@16 and null-terminates at 0x7e7):

      Page header (all page kinds): byte1 = type ('V' 0x56 = variable/text page,
      'D' 0x44 = fixed data page holding record heads), LOGICAL page number at
      +2 (u16) — logical != physical position — and a generation counter at +4.
      Btrieve shadow-paging leaves stale copies of D pages: the LIVE copy of a
      logical D page is the physical one with the highest generation byte.
      (The two FCR/PP page pairs follow the same generation scheme.)

      D pages: 6-byte header, then 30-byte physical records:
        [usage u16 == 1 when live] [logical 24 bytes] [VRP 4 bytes]
      logical: seq s16@0 (fragment index within the block), junk@2..15,
      block id i32@16, next-block link i32@20 (word 10 of the engine record;
      only ever set on the seq-0 record — asserted below). VRP: the text page
      as a LOGICAL V-page number, u16 at physical bytes 27-28.

      V pages: [16-byte header][2016-byte fragment][16-byte tail]; exactly one
      fragment per page (word@10 == 1 file-wide), 1:1 with live head records.
      Text is stored shifted: plain = (stored - 0x20) & 0xff (newline 0x0a ->
      '*' 0x2a on disk, ESC 0x1b -> 0x3b, etc.); first 0x00 terminates.

    A block's body = fragments of seq 0..n concatenated. 3267 blocks, 79
    multi-fragment, ids 0..10003. Known shipped-dangling next-links: 4
    (133, 440, 2962, 9637) — allowlisted engine-side like the message refs.
    """
    src = DATA + "wcctext2.vir"
    data = open(src, "rb").read()
    ps, _, _ = V.read_fcr(data)
    npages = len(data) // ps

    vmap = {}                                   # logical V page -> physical page
    dpages = {}                                 # logical D page -> (gen, physical)
    for p in range(npages):
        off = p * ps
        h = data[off:off+8]
        if h[:2] in (b"FC", b"PP"):
            continue
        logical = struct.unpack_from("<H", h, 2)[0]
        if h[1] == 0x56:                        # 'V' text page
            assert logical not in vmap, "duplicate logical V page %d" % logical
            vmap[logical] = p
        elif h[1] == 0x44:                      # 'D' head page (maybe shadowed)
            if logical not in dpages or h[4] > dpages[logical][0]:
                dpages[logical] = (h[4], p)

    def fragment(logical_v):
        off = vmap[logical_v] * ps
        frag = data[off+16 : off+ps-16]
        z = frag.find(b"\x00")
        if z >= 0:
            frag = frag[:z]
        return frag

    recs = {}                                   # (block id, seq) -> (next, logical V page)
    for gen, p in dpages.values():
        off = p * ps
        for k in range((ps - 6) // 30):
            r = data[off+6+30*k : off+6+30*k+30]
            if len(r) < 30:
                break
            if struct.unpack_from("<H", r, 0)[0] != 1:
                continue                        # free/deleted slot
            seq = struct.unpack_from("<h", r, 2)[0]
            bid = struct.unpack_from("<i", r, 18)[0]
            nxt = struct.unpack_from("<i", r, 22)[0]
            vp  = struct.unpack_from("<H", r, 27)[0]
            assert (bid, seq) not in recs, "duplicate live record (%d,%d)" % (bid, seq)
            assert vp in vmap, "block %d seq %d -> unknown V page %d" % (bid, seq, vp)
            assert seq == 0 or nxt == 0, "block %d seq %d carries a next-link" % (bid, seq)
            recs[(bid, seq)] = (nxt, vp)

    byid = {}
    for (bid, seq), (nxt, vp) in sorted(recs.items()):
        byid.setdefault(bid, []).append((seq, nxt, vp))
    db.execute("DROP TABLE IF EXISTS textblock")
    db.execute('CREATE TABLE textblock ("number" INTEGER PRIMARY KEY,'
               ' "next" INTEGER, "seq_count" INTEGER, "body" TEXT, "_raw" BLOB)')
    multi = 0
    for bid, parts in byid.items():
        seqs = [s for s, _, _ in parts]
        assert seqs == list(range(len(seqs))), "block %d seq gap %r" % (bid, seqs)
        raw = b"".join(fragment(vp) for _, _, vp in parts)
        body = bytes((b - 0x20) & 0xff for b in raw).decode("latin1")
        if len(parts) > 1:
            multi += 1
        db.execute("INSERT INTO textblock VALUES (?,?,?,?,?)",
                   (bid, parts[0][1], len(parts), body, raw))
    db.commit()
    print("  %-8s %-13s %4d cols  %6d rows  [%d multi-fragment, %d V pages]"
          % ("textblock", "wcctext2.vir", 4, len(byid), multi, len(vmap)))


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
    import_textblocks(db)
    db.close()
    print("Wrote", OUT)

if __name__ == "__main__":
    main()
