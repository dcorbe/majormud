#!/usr/bin/env python3
"""Correct Btrieve 6.x fixed-length record reader for the MajorMUD WG3-NT data files.

Format (verified empirically against wccrace2/wccitem2/etc.):
  FCR (page 0/1, magic 'FC'): page_size @+0x08, logical rec len @+0x16,
      physical rec len @+0x18 (= stride; physical-logical = per-record prefix).
  PAT pages: magic 'PP'. Index/other pages: various.
  DATA pages: record count at page+4 (byte), 0x80 flag at page+5; 6-byte page header,
      then `count` physical records; each record's logical data starts `prefix` bytes in
      (prefix = physical - logical).
  Liveness: the per-record prefix is a little-endian usage word — 1 for live records,
      anything else (0, free-list pointers, junk) for deleted/free slots. Verified across
      all nine game files: live counts match the engine-visible record counts exactly
      (15 classes, 13 races, 67 actions, 1102 monsters, 26720 rooms, ...). Files with
      prefix == 0 bytes (wccitem2) carry no marker; all slots are kept there.
  Shadow pages: Btrieve 6.x transactions leave stale duplicate page images in the
      file; a sequential page walk therefore sees some records twice. All observed
      duplicates are byte-identical, so exact-duplicate records are dropped (identical
      bytes imply identical key, and a live key exists once in the index).
Enumerates live data records; no key/index traversal needed for a full export.
"""
import struct

def read_fcr(data):
    # FCR magic ('FC') is present in some files, absent (zeros) in others; the
    # geometry fields are at fixed offsets either way.
    page_size = struct.unpack_from("<H", data, 0x08)[0]
    logical   = struct.unpack_from("<H", data, 0x16)[0]
    physical  = struct.unpack_from("<H", data, 0x18)[0]
    return page_size, logical, physical

def records(path):
    """Yield (record_number, logical_bytes) for every data record."""
    data = open(path, "rb").read()
    page_size, logical, physical = read_fcr(data)
    prefix = physical - logical
    out = []
    seen = set()
    capacity = (page_size - 6) // physical        # physical records that fit per page
    for page_off in range(page_size, len(data), page_size):
        magic = data[page_off:page_off+2]
        if magic in (b"FC", b"PP"):
            continue                              # FCR / allocation page
        if not (data[page_off+5] & 0x80):
            continue                              # not a data page
        # Read the full page capacity and validate each slot; the page's count byte
        # is NOT a reliable record count (map pages hold 2 rooms but report 1).
        for k in range(capacity):
            phys_off = page_off + 6 + k*physical
            d0 = phys_off + prefix                # logical data start
            if d0 + logical > len(data):
                break
            if prefix and int.from_bytes(data[phys_off:d0], "little") != 1:
                continue                          # deleted / free-list slot
            rec = data[d0:d0+logical]
            if not any(rec):
                continue                          # empty slot
            num = struct.unpack_from("<H", rec, 0)[0]
            if num < 1 or num > 59999:
                continue                          # not a live record (key word 0 / 0xffff)
            if rec in seen:
                continue                          # stale shadow-page duplicate
            seen.add(rec)
            out.append((num, rec))
    return page_size, logical, physical, out

# --- field helpers on logical record bytes ---
def s16(r,o): return struct.unpack_from("<h",r,o)[0]
def s32(r,o): return struct.unpack_from("<i",r,o)[0]
def cstr(r,o,n):
    b=r[o:o+n]; z=b.find(b"\x00"); return (b[:z] if z>=0 else b).decode("latin1").rstrip()

if __name__ == "__main__":
    import sys
    base = "/home/daniel/bbs/re/wg_nt_ref/WCCNT8PJ/out/"
    # Validate against known values using Nightmare field offsets.
    print("=== RACE (Nightmare offsets: Number@0, Name@2, minstats@0x20 Int/Wil/Str/Hea/Agl/Chm) ===")
    ps,lg,ph,recs = records(base+"wccrace2.vir")
    print("page=%d logical=%d physical=%d  records=%d"%(ps,lg,ph,len(recs)))
    for num,r in sorted(recs)[:14]:
        if num<1 or num>50: continue
        stats=[s16(r,0x20+2*i) for i in range(6)]
        print("  #%-2d %-12s stats=%s"%(num,cstr(r,2,29),stats))

    print("\n=== ITEMS (Name@0xad, Type@0x2f4, minDmg@0x33e, maxDmg@0x340) ===")
    ps,lg,ph,recs = records(base+"wccitem2.vir")
    print("records=%d"%len(recs))
    for num,r in sorted(recs):
        nm=cstr(r,0xad,29).lower()
        if nm in ("dagger","mace","quarterstaff","grey robes"):
            print("  #%-4d %-14s type=%d min=%d max=%d"%(num,nm,s16(r,0x2f4),s16(r,0x33e),s16(r,0x340)))
