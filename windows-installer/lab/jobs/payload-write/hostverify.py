#!/usr/bin/env python3
"""
Host-side byte verification of round 38's payload-write guest run.

Reads the qcow2 overlay the guest actually wrote through
(/w/payload-write-fixture-a.qcow2), converted to raw by qemu-img, and compares
it against the pristine backing file /w/fixture-a.raw. Nothing here trusts
anything Windows printed: every number is re-derived on the host.
"""
import hashlib, json, os, sys

W, O = '/w', '/o'
PRISTINE  = f'{W}/fixture-a.raw'
CONVERTED = f'{O}/pw-fixture-a.raw'
MAPJSON   = f'{O}/overlay-map-verify.json'

DISK_BYTES = 38654705664
SECTOR     = 512

# Partition extents, from the guest's own survey in payload-write.log.
P1 = (1048576,     18254659584)   # APEX-TARGET-A, Linux filesystem, eligible
P2 = (18254659584, 19328401408)   # "Windows data", NTFS, mounted at E:
P3 = (19328401408, 37582012416)   # "Blank basic", RAW, lettered F:
GPT_PRI = (0, 34 * SECTOR)                       # protective MBR + hdr + entries
GPT_BAK = (DISK_BYTES - 33 * SECTOR, DISK_BYTES) # backup entries + header
FREE_TAIL = (P3[1], GPT_BAK[0])                  # unpartitioned gap before backup GPT

PAYLOAD_OFF, PAYLOAD_LEN = 1048576, 4194304
PAYLOAD_SHA   = '551611eab74b0fd88e2c00685778fb6aad233dc0916e3c464ddbc4ac2d21b683'
P2_SAMPLE_SHA = 'd97f92039d5ed46a57ad51a364c491ef7356238b4dc380f4f03f09d36993768e'
P3_BEFORE_SHA = '076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560'
P3_AFTER_SHA  = 'f4d5587ca8006d82d8b4ae388e30713a8293bb1e03590dbae0fbdd6fc321fdf9'
SAMPLE_LEN    = 512

results = []
def check(ok, name, detail):
    results.append((ok, name, detail))
    print(f'[{"PASS" if ok else "FAIL"}] {name}: {detail}', flush=True)

def rd(path, off, ln):
    with open(path, 'rb') as f:
        f.seek(off)
        b = f.read(ln)
    if len(b) != ln:
        raise IOError(f'short read {path} @{off}: {len(b)} of {ln}')
    return b

def sha(b):
    return hashlib.sha256(b).hexdigest()

def sha_range(path, off, ln, chunk=1 << 20):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        f.seek(off)
        left = ln
        while left:
            b = f.read(min(chunk, left))
            if not b:
                raise IOError(f'short read {path} @{off}')
            h.update(b); left -= len(b)
    return h.hexdigest()

def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]

def diffbytes(off, ln, chunk=1 << 20):
    """Byte positions that differ between converted and pristine over [off,off+ln)."""
    out, left, pos = [], ln, off
    fa, fb = open(CONVERTED, 'rb'), open(PRISTINE, 'rb')
    try:
        fa.seek(off); fb.seek(off)
        while left:
            n = min(chunk, left)
            a, b = fa.read(n), fb.read(n)
            if len(a) != n or len(b) != n:
                raise IOError('short read during compare')
            if a != b:
                for i in range(n):
                    if a[i] != b[i]:
                        out.append(pos + i)
            pos += n; left -= n
    finally:
        fa.close(); fb.close()
    return out

print('=== host-side verification of payload-write (round 38 guest run) ===')
print(f'pristine  : {PRISTINE}  mtime {os.path.getmtime(PRISTINE)}')
print(f'converted : {CONVERTED}')
print()

# ---------------------------------------------------------------- sizes ----
check(os.path.getsize(PRISTINE) == DISK_BYTES, 'pristine size',
      f'{os.path.getsize(PRISTINE)} == {DISK_BYTES}')
check(os.path.getsize(CONVERTED) == DISK_BYTES, 'converted size',
      f'{os.path.getsize(CONVERTED)} == {DISK_BYTES}')

# ------------------------------------------- the allocation map (depth 0) ---
# depth==0 means the cluster lives in the OVERLAY: the guest wrote it.
# depth==1 means it reads through to the pristine backing file untouched.
m = json.load(open(MAPJSON))
written = [(e['start'], e['start'] + e['length'])
           for e in m if e.get('depth') == 0 and e.get('data')]
written.sort()
total_written = sum(b - a for a, b in written)
print(f'--- {len(written)} extents written by the guest, {total_written} bytes total ---')

def classify(x):
    for nm, r in (('p1', P1), ('p2', P2), ('p3', P3),
                  ('GPT-primary', GPT_PRI), ('GPT-backup', GPT_BAK),
                  ('free-tail', FREE_TAIL)):
        if overlaps(x, r):
            return nm
    return 'OUTSIDE-EVERYTHING'

buckets = {}
for x in written:
    buckets.setdefault(classify(x), []).append(x)
for k in sorted(buckets):
    n = sum(b - a for a, b in buckets[k])
    print(f'    {k:20s} {len(buckets[k]):3d} extents, {n} bytes')
print()

stray = buckets.get('GPT-primary', []) + buckets.get('GPT-backup', []) \
      + buckets.get('free-tail', []) + buckets.get('OUTSIDE-EVERYTHING', [])
check(not stray, 'nothing written outside p1/p2/p3',
      'no depth-0 extent touches the GPT (primary or backup), the free tail, '
      'or any region outside the three partitions'
      if not stray else f'STRAY EXTENTS: {stray}')

# ------------------------------------------------ 1. the payload landed -----
got = sha_range(CONVERTED, PAYLOAD_OFF, PAYLOAD_LEN)
check(got == PAYLOAD_SHA, 'p1 payload sha256 (4 MiB @ 1048576)',
      f'{got} == guest PAYLOAD-SHA256' if got == PAYLOAD_SHA
      else f'{got} != {PAYLOAD_SHA}')

p1w = buckets.get('p1', [])
check(p1w == [(PAYLOAD_OFF, PAYLOAD_OFF + PAYLOAD_LEN)],
      'p1 was written at the verified offset and nowhere else',
      f'exactly one extent {p1w} = [{PAYLOAD_OFF}, {PAYLOAD_OFF + PAYLOAD_LEN})'
      if p1w == [(PAYLOAD_OFF, PAYLOAD_OFF + PAYLOAD_LEN)] else f'extents: {p1w}')

# p1's remainder was proven all-zero in-guest before the write; it is
# unallocated in the overlay, so it still reads the pristine bytes. Spot-check.
zsamples = [PAYLOAD_OFF + PAYLOAD_LEN, P1[1] - SECTOR, (P1[0] + P1[1]) // 2]
zbad = [o for o in zsamples if rd(CONVERTED, o, SECTOR) != b'\0' * SECTOR]
check(not zbad, 'p1 remainder still zero',
      f'{len(zsamples)} sampled sectors after the payload are all zero'
      if not zbad else f'non-zero at {zbad}')

# ------------------------------------------- 2. the GPT was never touched ---
for nm, r in (('GPT primary', GPT_PRI), ('GPT backup', GPT_BAK)):
    d = diffbytes(r[0], r[1] - r[0])
    check(not d, f'{nm} byte-identical to pristine',
          f'{r[1]-r[0]} bytes at {r[0]} compare equal'
          if not d else f'{len(d)} differing bytes, first at {d[0]}')

# --------------------------- 3. write 2 (NTFS): refusal left bytes alone ----
c2 = rd(CONVERTED, P2[0], SAMPLE_LEN)
p2p = rd(PRISTINE, P2[0], SAMPLE_LEN)
check(sha(c2) == sha(p2p) == P2_SAMPLE_SHA,
      "write 2's exact target (512 B @ p2 offset) unchanged",
      f'converted {sha(c2)[:16]}… == pristine {sha(p2p)[:16]}… == guest '
      f'p2-before/after {P2_SAMPLE_SHA[:16]}…'
      if sha(c2) == sha(p2p) == P2_SAMPLE_SHA
      else f'converted {sha(c2)} pristine {sha(p2p)} guest {P2_SAMPLE_SHA}')

# Everything else that changed inside p2 is Windows' own NTFS activity from
# having E: mounted — not the tool, and not the refused write. Quantify it.
p2diff = 0
for a, b in buckets.get('p2', []):
    p2diff += len(diffbytes(a, b - a))
print(f'    p2: {p2diff} bytes differ from pristine across '
      f'{sum(b-a for a,b in buckets.get("p2", []))} bytes of guest-written extents '
      f'(Windows NTFS metadata, volume mounted at E:)')

# ---------------------- 4. write 3 (RAW volume): succeeded; bound its reach --
p3w = buckets.get('p3', [])
check(len(p3w) == 1 and p3w[0][0] == P3[0],
      'p3 has exactly one written cluster, at the partition offset',
      f'{p3w}')
if len(p3w) == 1:
    a, b = p3w[0]
    d = diffbytes(a, b - a)
    inside = all(P3[0] <= x < P3[0] + SAMPLE_LEN for x in d)
    check(inside and d,
          "write 3's damage is bounded to the 512 bytes it wrote",
          f'{len(d)} differing bytes in the {b-a}-byte cluster, all inside '
          f'[{P3[0]}, {P3[0]+SAMPLE_LEN})'
          if inside and d else f'{len(d)} differing bytes, range '
          f'{(min(d), max(d)) if d else "none"}')
    c3 = rd(CONVERTED, P3[0], SAMPLE_LEN)
    p3p = rd(PRISTINE, P3[0], SAMPLE_LEN)
    check(sha(c3) == P3_AFTER_SHA, 'p3 now holds what the guest wrote',
          f'{sha(c3)} == guest p3-after' if sha(c3) == P3_AFTER_SHA
          else f'{sha(c3)} != {P3_AFTER_SHA}')
    check(sha(p3p) == P3_BEFORE_SHA, 'pristine p3 still holds the pre-write bytes',
          f'{sha(p3p)} == guest p3-before' if sha(p3p) == P3_BEFORE_SHA
          else f'{sha(p3p)} != {P3_BEFORE_SHA}')

print()
bad = [r for r in results if not r[0]]
print(f'=== {len(results)-len(bad)} passed, {len(bad)} failed ===')
for _, n, d in bad:
    print(f'  FAILED: {n}: {d}')
sys.exit(1 if bad else 0)
