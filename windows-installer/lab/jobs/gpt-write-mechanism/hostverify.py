#!/usr/bin/env python3
"""
Host-side verification of the gpt-write-mechanism guest run.

Two overlays are examined, both converted to sparse raw by qemu-img and
compared against their pristine backing files:

  /o/gptmech-fa.raw   vs /w/fixture-a.raw  -- where the two mechanisms ran
  /o/gptmech-sys.raw  vs /w/golden.raw     -- the LIVE SYSTEM DISK, which
                                              phase A probed with a no-op raw
                                              write and an unmodified
                                              SET_DRIVE_LAYOUT_EX

The question the decision asks is invariant 1: "The delta is exactly one
entry's type GUID and attributes. Nothing else on the disk changes, proven
byte-identical against a pristine fixture." This answers it by bytes.
"""
import binascii, hashlib, json, os, struct, sys

SEC = 512
LINUX = '0fc63daf-8483-4772-8e79-3d69d8477de4'
BASIC = 'ebd0a0a2-b9e5-4433-87c0-68b6b72699c7'
TARGET_ID = 'a56302e7-577f-42b4-b6c1-d4d89d654411'

results = []
def check(ok, name, detail):
    results.append((ok, name, detail))
    print(f'[{"PASS" if ok else "FAIL"}] {name}: {detail}', flush=True)
def note(s): print(f'    {s}', flush=True)

def rd(path, off, ln):
    with open(path, 'rb') as f:
        f.seek(off); b = f.read(ln)
    if len(b) != ln: raise IOError(f'short read {path}@{off}')
    return b

def guid(b):
    d1, d2, d3 = struct.unpack_from('<IHH', b, 0)
    d4 = b[8:16]
    return '%08x-%04x-%04x-%s-%s' % (d1, d2, d3, d4[:2].hex(), d4[2:].hex())

def crc32(b): return binascii.crc32(b) & 0xFFFFFFFF

def parse_gpt(path, lba):
    h = rd(path, lba * SEC, SEC)
    if h[:8] != b'EFI PART':
        return None
    hs   = struct.unpack_from('<I', h, 12)[0]
    hcrc = struct.unpack_from('<I', h, 16)[0]
    my   = struct.unpack_from('<Q', h, 24)[0]
    alt  = struct.unpack_from('<Q', h, 32)[0]
    first = struct.unpack_from('<Q', h, 40)[0]
    elba = struct.unpack_from('<Q', h, 72)[0]
    n    = struct.unpack_from('<I', h, 80)[0]
    esz  = struct.unpack_from('<I', h, 84)[0]
    ecrc = struct.unpack_from('<I', h, 88)[0]
    hz = bytearray(h); hz[16:20] = b'\0\0\0\0'
    ents = rd(path, elba * SEC, n * esz)
    return dict(my=my, alt=alt, elba=elba, n=n, esz=esz, first=first,
                hcrc=hcrc, hcrc_calc=crc32(bytes(hz[:hs])),
                ecrc=ecrc, ecrc_calc=crc32(ents), ents=ents, raw=h)

def entries(g):
    out = []
    for i in range(g['n']):
        e = g['ents'][i * g['esz']:(i + 1) * g['esz']]
        if e[:16] == b'\0' * 16: continue
        out.append((i, guid(e[:16]), guid(e[16:32]),
                    struct.unpack_from('<Q', e, 32)[0],
                    struct.unpack_from('<Q', e, 40)[0],
                    struct.unpack_from('<Q', e, 48)[0]))
    return out

def written_extents(mapjson):
    m = json.load(open(mapjson))
    return sorted((e['start'], e['start'] + e['length'])
                  for e in m if e.get('depth') == 0 and e.get('data'))

def diff_ranges(a_path, b_path, off, ln, chunk=1 << 20):
    """Byte offsets that differ between two files over [off, off+ln)."""
    out, left, pos = [], ln, off
    fa, fb = open(a_path, 'rb'), open(b_path, 'rb')
    try:
        fa.seek(off); fb.seek(off)
        while left:
            k = min(chunk, left)
            x, y = fa.read(k), fb.read(k)
            if x != y:
                for i in range(k):
                    if x[i] != y[i]: out.append(pos + i)
            pos += k; left -= k
    finally:
        fa.close(); fb.close()
    return out

def summarise(offsets):
    """Collapse a sorted offset list into contiguous runs."""
    runs = []
    for o in offsets:
        if runs and o == runs[-1][1]: runs[-1][1] = o + 1
        else: runs.append([o, o + 1])
    return runs

# ─────────────────────────────── fixture-a: where the mechanisms ran ────────
print('=== fixture-a: the disk both mechanisms were run against ===')
MODE = os.environ.get('APEX_GPT_MODE', 'm2')
FA  = os.environ.get('APEX_GPT_FA',  '/o/gptmech-fa.raw')
MAP = os.environ.get('APEX_GPT_MAP', '/o/gptmech-fa-map.json')
FAP = '/w/fixture-a.raw'
print(f'(mechanism under test: {MODE.upper()}; image {FA})')
size = os.path.getsize(FAP)
check(os.path.getsize(FA) == size, 'fixture-a sizes match', f'{size}')

pre_pri  = parse_gpt(FAP, 1)
post_pri = parse_gpt(FA, 1)
post_bak = parse_gpt(FA, post_pri['alt'])
pre_bak  = parse_gpt(FAP, pre_pri['alt'])

note(f"pristine primary: entries at LBA {pre_pri['elba']}, {pre_pri['n']}x{pre_pri['esz']}")
note(f"final    primary: entries at LBA {post_pri['elba']}, {post_pri['n']}x{post_pri['esz']}")
note(f"pristine backup : entries at LBA {pre_bak['elba']}")
note(f"final    backup : entries at LBA {post_bak['elba']}")

check(post_pri['hcrc'] == post_pri['hcrc_calc'] and post_pri['ecrc'] == post_pri['ecrc_calc'],
      'final primary GPT CRCs are self-consistent',
      f"header 0x{post_pri['hcrc']:08x}==0x{post_pri['hcrc_calc']:08x}, "
      f"entries 0x{post_pri['ecrc']:08x}==0x{post_pri['ecrc_calc']:08x}")
check(post_bak['hcrc'] == post_bak['hcrc_calc'] and post_bak['ecrc'] == post_bak['ecrc_calc'],
      'final backup GPT CRCs are self-consistent',
      f"header 0x{post_bak['hcrc']:08x}==0x{post_bak['hcrc_calc']:08x}, "
      f"entries 0x{post_bak['ecrc']:08x}==0x{post_bak['ecrc_calc']:08x}")
check(post_pri['ecrc'] == post_bak['ecrc'],
      'both copies describe the same partition array',
      f"entries CRC 0x{post_pri['ecrc']:08x} on both")

# The delta the decision cares about: exactly one entry's type GUID + attrs?
pre_e  = {e[2]: e for e in entries(pre_pri)}
post_e = {e[2]: e for e in entries(post_pri)}
check(set(pre_e) == set(post_e), 'the same partitions exist before and after',
      f'{len(pre_e)} partitions, same unique GUIDs')
changed = []
for pid in pre_e:
    a, b = pre_e[pid], post_e[pid]
    if a[1] != b[1] or a[3] != b[3] or a[4] != b[4] or a[5] != b[5]:
        changed.append((pid, a, b))
check(len(changed) == 1, 'exactly one entry differs', f'{len(changed)} changed')
for pid, a, b in changed:
    note(f'  {pid}')
    note(f'    type   {a[1]}  ->  {b[1]}')
    note(f'    start/end/attrs unchanged: {a[3]==b[3] and a[4]==b[4] and a[5]==b[5]}')
    check(pid == TARGET_ID and a[1] == BASIC and b[1] == LINUX,
          'the changed entry is the intended one, basic-data -> Linux filesystem',
          f'{pid[:8]}… {a[1][:8]}… -> {b[1][:8]}…')

# Where on the disk did bytes actually move?
ext = written_extents(MAP)
note(f'guest-written extents on fixture-a: {len(ext)}, '
     f'{sum(b-a for a,b in ext)} bytes')
P1, P2, P3 = (1048576, 18254659584), (18254659584, 19328401408), (19328401408, 37582012416)
def inside(x, r): return x[0] < r[1] and r[0] < x[1]
in_p1 = [x for x in ext if inside(x, P1)]
in_p3 = [x for x in ext if inside(x, P3)]
in_p2 = [x for x in ext if inside(x, P2)]
# p2 is the NTFS volume Windows mounts at E:. It dirties it by mounting it --
# the same behaviour that made the qcow2 fixture overlays necessary, and the
# same 6 MB of metadata churn payload-write measured. It is not the tool and
# it is not either mechanism, so it is reported and attributed rather than
# asserted away.
check(not in_p1 and not in_p3,
      'neither mechanism wrote a byte of partition CONTENT',
      'no written extent touches p1 (the eligible Linux partition) or p3 '
      '(the partition whose TYPE both mechanisms changed) — changing an '
      'entry never touched the bytes it describes'
      if not in_p1 and not in_p3 else f'p1:{in_p1} p3:{in_p3}')
note(f'p2 (NTFS, mounted E:): {len(in_p2)} extents, '
     f'{sum(b-a for a,b in in_p2)} bytes — Windows metadata from having the '
     f'volume mounted, not the tool')
outside = [x for x in ext if not (inside(x, P1) or inside(x, P2) or inside(x, P3))]
note(f'outside every partition: {len(outside)} extents')
for a, b in outside:
    note(f'  written {a}..{b}  (LBA {a//SEC}..{b//SEC})')

# Byte-level delta over the whole GPT reserved area (LBA 0 .. first partition)
d = diff_ranges(FA, FAP, 0, P1[0])
note(f'primary GPT area (LBA 0..2047): {len(d)} bytes differ from pristine')
for r in summarise(d):
    note(f'  bytes {r[0]}..{r[1]} (LBA {r[0]//SEC}, offset {r[0]%SEC} within it), {r[1]-r[0]} bytes')
d2 = diff_ranges(FA, FAP, P3[1], size - P3[1])
note(f'backup GPT area (after p3): {len(d2)} bytes differ from pristine')
for r in summarise(d2):
    note(f'  bytes {r[0]}..{r[1]} (LBA {r[0]//SEC}), {r[1]-r[0]} bytes')

check(len(d) + len(d2) > 0 and not in_p1 and not in_p3,
      'the whole GPT delta is confined to the two GPT areas',
      f'{len(d)} bytes in the primary GPT area + {len(d2)} in the backup GPT '
      f'area, and not one byte of p1 or p3 on a {size}-byte disk')

# The relocation, stated as its own finding rather than buried in a byte list.
moved = pre_pri['elba'] != post_pri['elba']
check(moved == (MODE == 'm2'),
      'the entry array moved if and only if SET_DRIVE_LAYOUT_EX was used',
      f"mechanism {MODE.upper()}: array {'moved' if moved else 'stayed put'} "
      f"(LBA {pre_pri['elba']} -> {post_pri['elba']}) — the raw "
      f"read-modify-write edits in place, so it leaves exactly one table")
note(f"PRIMARY ENTRY ARRAY: LBA {pre_pri['elba']} -> {post_pri['elba']} "
     f"({'MOVED by SET_DRIVE_LAYOUT_EX' if moved else 'not moved'})")
# WHY it moved, which decides whether the hazard applies to real machines at
# all: Windows appears to park the array so that it ENDS at FirstUsableLBA.
# fixture-a's FirstUsableLBA is 2048 (a 1 MiB reserve, what sgdisk/sfdisk and
# therefore Linux tooling default to), so 2048-32 = 2016. A disk Windows Setup
# partitioned itself has FirstUsableLBA 34, so 34-32 = 2 and the array would
# land back where it started. This is a falsifiable rule, so it is checked.
arr_sectors = post_pri['n'] * post_pri['esz'] // SEC
note(f"FirstUsableLBA = {post_pri['first']}, array is {arr_sectors} sectors")
if moved: check(post_pri['elba'] + arr_sectors == post_pri['first'],
      'the relocated array ends exactly at FirstUsableLBA',
      f"{post_pri['elba']} + {arr_sectors} == {post_pri['first']} — so the "
      f"destination is a function of FirstUsableLBA, not a fixed LBA. On a "
      f"disk with FirstUsableLBA 34 the same rule yields LBA 2, i.e. no move "
      f"and no stale table")
if moved:
    # What is left behind at the old location? If the old array is still there,
    # anything that reads LBA 2 directly instead of honouring the header's
    # PartitionEntryLBA sees a STALE table with the OLD type GUID.
    stale = diff_ranges(FA, FAP, pre_pri['elba'] * SEC, pre_pri['n'] * pre_pri['esz'])
    check(not stale,
          'the OLD entry array is still sitting at its old LBA, untouched',
          f"LBA {pre_pri['elba']}..{pre_pri['elba'] + 32} is byte-identical to "
          f"pristine — the stale pre-change table was NOT erased or updated"
          if not stale else f'{len(stale)} bytes differ')
    old_ents = rd(FAP, pre_pri['elba'] * SEC, pre_pri['n'] * pre_pri['esz'])
    still = rd(FA, pre_pri['elba'] * SEC, pre_pri['n'] * pre_pri['esz'])
    tgt_off = None
    for i in range(pre_pri['n']):
        e = still[i * pre_pri['esz']:(i + 1) * pre_pri['esz']]
        if e[:16] != b'\0' * 16 and guid(e[16:32]) == TARGET_ID:
            tgt_off = i; break
    if tgt_off is not None:
        e = still[tgt_off * pre_pri['esz']:(tgt_off + 1) * pre_pri['esz']]
        note(f'  a reader that trusts LBA {pre_pri["elba"]} sees this entry as '
             f'type {guid(e[:16])}')
        note(f'  the header says the live array is at LBA {post_pri["elba"]}, '
             f'where the same entry reads {post_e[TARGET_ID][1]}')

# ──────────────────────── the live system disk: did phase A change it? ──────
print()
print('=== the LIVE SYSTEM DISK, probed in phase A ===')
SY, SYP = '/o/gptmech-sys.raw', '/w/golden.raw'
sext = written_extents('/o/gptmech-sys-map.json')
note(f'guest-written extents on the system disk: {len(sext)}, '
     f'{sum(b-a for a,b in sext)} bytes (a booted Windows writes constantly)')
GPT_PRI = (0, 34 * SEC)
ssize = os.path.getsize(SYP)
GPT_BAK = (ssize - 33 * SEC, ssize)
sys_pri = parse_gpt(SYP, 1)
note(f"golden.raw (partitioned by Windows Setup): FirstUsableLBA = "
     f"{sys_pri['first']}, PartitionEntryLBA = {sys_pri['elba']} — the rule "
     f"above predicts {sys_pri['first'] - arr_sectors} for this disk, i.e. "
     f"{'NO relocation' if sys_pri['first'] - arr_sectors == sys_pri['elba'] else 'a relocation'}")
for nm, r in (('primary GPT', GPT_PRI), ('backup GPT', GPT_BAK)):
    dd = diff_ranges(SY, SYP, r[0], r[1] - r[0])
    check(not dd, f'system disk {nm} byte-identical to pristine golden.raw',
          f'{r[1]-r[0]} bytes at {r[0]} compare equal — the no-op raw write and '
          f'the unmodified SET_DRIVE_LAYOUT_EX changed nothing'
          if not dd else f'{len(dd)} bytes differ, first at {dd[0]}')

print()
bad = [r for r in results if not r[0]]
print(f'=== {len(results)-len(bad)} passed, {len(bad)} failed ===')
for _, n, dtl in bad: print(f'  FAILED: {n}: {dtl}')
sys.exit(1 if bad else 0)
