# initramfs-slim-2 — continuation of initramfs-slim

items: none directly (ESP budget; feeds P0-001 and the boot-v2 work)
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim-2
branch: task/initramfs-slim-2, cut from origin/roadmap/v2.2 @ f3b1b3d4
lab: /var/lab-scratch/initramfs-slim-2

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## NEXT

- Find two `build-image` runs whose apex-tier parent (base) digest is the same,
  pull ONLY the dracut layer blob of each from ghcr (not the 15 GB image), and
  compare the two `initramfs.img` byte for byte. That is item 5 and it is also
  the verification of a claim ALREADY LANDED in `Containerfile.apex:~240`:
  "unchanged theme + unchanged kernel yield a byte-identical initramfs and the
  layer digest stops moving."

## DONE

## IN PROGRESS

## FOUND — round 40, measured today

### The predecessor's open risk is CLOSED, and the numbers it left are stale

Build run **35624291221** (`roadmap/v2.2` @ `44c9a5cb`, 2026-09-21 16:14Z,
1h43m) — job `image` (id 106444907037) **succeeded**, and its log
(`/var/lab-scratch/initramfs-slim-2/imagejob-35624291221.log`, lines 800-840)
shows the new stanza running end to end:

* `STEP 11/16 COPY … check-initramfs-budget.sh` then `STEP 12/16 RUN … dracut
  … /tmp/check-initramfs-budget.sh …` — the COPY and the call DO execute
  together in a real image build.
* **17 gates, 17 passed, 0 failed** — including all eight `unlock-*` gates the
  predecessor said it could not claim had ever passed a real build.

**The shipped numbers are BETTER than the evidence file claims, because the
kernel changed.** The build used `7.2.6-cachyos1.apex1.fc43.x86_64` (the APEX
kernel tier), not the lab's `7.2.5-cachyos1`:

| | evidence file (lab, 7.2.5) | real image build (7.2.6-apex1) |
|---|---|---|
| vmlinuz | 16,910,408 B (16.1 MiB) | 16.1 MiB |
| initramfs | 103,256,987 B (98.5 MiB) | **84.8 MiB** |
| per deployment | 114.6 MiB | **100.9 MiB** |
| `need = 3x + 48` | 391 MiB | **350 MiB** |
| `=drivers/net` residue | 11 files / 1.5 MiB | 11 files / **1.2 MiB** |
| dracut modules / entries | — | 54 modules, 3684 entries |

So `ROADMAP/evidence/initramfs-slim-20260921.md` (landed) and
`Containerfile.apex`'s comment ("375.2 MiB to 114.6 MiB") both understate the
win and cite a lab kernel. Both need the real build-log figure.

### dracut printed a FAILED line and exited 0

In that same real build:
`dracut-install: ERROR: installing '/root'` /
`dracut[E]: FAILED: /usr/lib/dracut/dracut-install -D /var/tmp/dracut.dp78rrG/initramfs -f /root`
— and dracut still returned 0, so the build carried on. Also
`dracut[E]: No '/dev/log' or 'logger' included for syslog logging`. The
`clevis-pin-tang`/`cifs` "depends on module 'network'" messages are the
expected, documented ones.

### Items 4 and 6 were already done by round 39 and are ON THE TIP

The dispatch card listed items 4, 5, 6 as remaining. Checked against
`origin/roadmap/v2.2:ROADMAP/evidence/initramfs-slim-20260921.md`: the
attributed size matrix (item 4, v0-v4) and the two-boot VM proof (item 6,
`simple-framebuffer` + `graphical.target` in 7.714 s) are both there. Item 5 is
the only one genuinely open, and only its cross-BUILD half — round 39 measured
run-to-run in one chroot and lab-vs-image faithfulness, and said in the
evidence file, correctly, "what this does not prove: cross-host
bit-reproducibility".

## BLOCKED ON

- nothing
