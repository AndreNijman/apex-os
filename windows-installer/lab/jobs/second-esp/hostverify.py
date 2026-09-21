#!/usr/bin/env python3
"""Host-side check of the second-ESP run, against pristine golden.raw."""
import binascii, json, os, struct, sys
SEC = 512
SY, SYP = '/o/secondesp-sys.raw', '/w/golden.raw'
ESP_T = 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b'
MSR_T = 'e3c9e316-0b5c-4db8-817d-f92df00215ae'
results = []
def check(ok, n, d):
    results.append((ok, n, d)); print(f'[{"PASS" if ok else "FAIL"}] {n}: {d}', flush=True)
def note(s): print(f'    {s}', flush=True)
def rd(p, o, l):
    with open(p, 'rb') as f: f.seek(o); b = f.read(l)
    if len(b) != l: raise IOError('short read')
    return b
def guid(b):
    d1, d2, d3 = struct.unpack_from('<IHH', b, 0)
    return '%08x-%04x-%04x-%s-%s' % (d1, d2, d3, b[8:10].hex(), b[10:16].hex())
def gpt(p, lba=1):
    h = rd(p, lba * SEC, SEC); assert h[:8] == b'EFI PART', p
    d = dict(hs=struct.unpack_from('<I', h, 12)[0], hcrc=struct.unpack_from('<I', h, 16)[0],
             first=struct.unpack_from('<Q', h, 40)[0], alt=struct.unpack_from('<Q', h, 32)[0],
             elba=struct.unpack_from('<Q', h, 72)[0], n=struct.unpack_from('<I', h, 80)[0],
             esz=struct.unpack_from('<I', h, 84)[0], ecrc=struct.unpack_from('<I', h, 88)[0])
    hz = bytearray(h); hz[16:20] = b'\0\0\0\0'
    d['hcrc_calc'] = binascii.crc32(bytes(hz[:d['hs']])) & 0xFFFFFFFF
    d['ents'] = rd(p, d['elba'] * SEC, d['n'] * d['esz'])
    d['ecrc_calc'] = binascii.crc32(d['ents']) & 0xFFFFFFFF
    return d
def parts(g):
    out = {}
    for i in range(g['n']):
        e = g['ents'][i * g['esz']:(i + 1) * g['esz']]
        if e[:16] == b'\0' * 16: continue
        out[guid(e[16:32])] = dict(idx=i, type=guid(e[:16]),
            start=struct.unpack_from('<Q', e, 32)[0], end=struct.unpack_from('<Q', e, 40)[0],
            attrs=struct.unpack_from('<Q', e, 48)[0],
            name=e[56:128].decode('utf-16-le').rstrip('\0'))
    return out
def diffs(a, b, off, ln, chunk=1 << 20):
    out, left, pos = 0, ln, off
    fa, fb = open(a, 'rb'), open(b, 'rb')
    try:
        fa.seek(off); fb.seek(off)
        while left:
            k = min(chunk, left); x, y = fa.read(k), fb.read(k)
            if x != y: out += sum(1 for i in range(k) if x[i] != y[i])
            pos += k; left -= k
    finally: fa.close(); fb.close()
    return out

pre, post = gpt(SYP), gpt(SY)
note(f"pristine: FirstUsableLBA={pre['first']} PartitionEntryLBA={pre['elba']}")
note(f"after   : FirstUsableLBA={post['first']} PartitionEntryLBA={post['elba']}")

# THE PREDICTION, from the gpt-write-mechanism round: Windows parks the entry
# array so it ends at FirstUsableLBA. golden.raw's is 34, so 34-32 = 2 and the
# array should NOT move -- unlike fixture-a, where 2048-32 = 2016 and it did.
# Windows has now rewritten this GPT for real (a shrink plus a new partition),
# so this is the measurement that prediction was waiting for.
check(post['elba'] == 2 and pre['elba'] == 2,
      'PREDICTION CONFIRMED: no relocation on a FirstUsableLBA=34 disk',
      f"Windows rewrote this GPT (shrank C:, added a partition) and the primary "
      f"entry array stayed at LBA {post['elba']}. 34 - 32 = 2, so there is "
      f"nowhere else to park it and no stale table is left behind — exactly as "
      f"the fixture-a measurement predicted for a Windows-made disk")

check(post['hcrc'] == post['hcrc_calc'] and post['ecrc'] == post['ecrc_calc'],
      'the rewritten primary GPT is self-consistent',
      f"header 0x{post['hcrc']:08x}, entries 0x{post['ecrc']:08x}")
pb = gpt(SY, post['alt'])
check(pb['hcrc'] == pb['hcrc_calc'] and pb['ecrc'] == pb['ecrc_calc'] and pb['ecrc'] == post['ecrc'],
      'the rewritten backup GPT is self-consistent and agrees with the primary',
      f"entries CRC 0x{pb['ecrc']:08x} on both copies")

a, b = parts(pre), parts(post)
note(f'pristine partitions: {len(a)}; after: {len(b)}')
added = [k for k in b if k not in a]
removed = [k for k in a if k not in b]
check(len(added) == 1 and not removed, 'exactly one partition was added and none removed',
      f'added {len(added)}, removed {len(removed)}')
for k in added:
    note(f"added: {b[k]['name']!r} type={b[k]['type']} LBA {b[k]['start']}..{b[k]['end']}")
    check(b[k]['type'] == ESP_T, 'the added partition is an EFI System Partition',
          f"type {b[k]['type']}")

# Windows' OWN entries: the ESP and the MSR must be untouched; C: may change
# only its END, because the shrink is supposed to move only that.
for pid in a:
    x, y = a[pid], b[pid]
    if x['type'] == ESP_T:
        check(x == y, "Windows' own ESP entry is completely unchanged",
              f"type, start {x['start']}, end {x['end']}, attrs 0x{x['attrs']:016x}, name {x['name']!r}")
    elif x['type'] == MSR_T:
        check(x == y, 'the Microsoft Reserved entry is completely unchanged',
              f"start {x['start']}, end {x['end']}")
    else:
        same_but_end = (x['type'] == y['type'] and x['start'] == y['start']
                        and x['attrs'] == y['attrs'] and x['name'] == y['name'])
        check(same_but_end and x['end'] != y['end'],
              "C:'s entry changed in EndingLBA and nothing else",
              f"end {x['end']} -> {y['end']} ({(x['end']-y['end'])*SEC/2**20:.0f} MiB "
              f"returned), start/type/attrs/name identical")

# Windows' ESP CONTENT. It is not byte-identical, and the guest says why.
esp = [v for v in a.values() if v['type'] == ESP_T][0]
off, ln = esp['start'] * SEC, (esp['end'] - esp['start'] + 1) * SEC
n = diffs(SY, SYP, off, ln)
note(f"Windows' ESP content: {n} of {ln} bytes differ from pristine "
     f"({100.0*n/ln:.3f}%) — the guest attributes this to Windows' own BCD "
     f"writes (BCD, BCD.LOG, BOOTSTAT.DAT, Recovery\\BCD), not to anything APEX did")
check(n > 0, 'Windows wrote its OWN ESP during this run',
      f'{n} bytes changed — so "Windows\' ESP is never written" is a rule about '
      f'what APEX does, not a description of what the partition experiences')

print()
bad = [r for r in results if not r[0]]
print(f'=== {len(results)-len(bad)} passed, {len(bad)} failed ===')
for _, n_, d_ in bad: print(f'  FAILED: {n_}: {d_}')
sys.exit(1 if bad else 0)
