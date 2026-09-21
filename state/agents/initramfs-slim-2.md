# initramfs-slim-2 — continuation of initramfs-slim

items: none directly (ESP budget; feeds P0-001 and the boot-v2 work)
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim-2
branch: task/initramfs-slim-2, cut from origin/roadmap/v2.2 @ f3b1b3d4
lab: /var/lab-scratch/initramfs-slim-2

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## LANDABLE — `ba9a417a` (more coming; see NEXT)

Three commits, all pushed to `origin/task/initramfs-slim-2`. Evidence file plus
the correction of two claims the first real build disproved. Nothing in the
branch changes a build step — it is documentation and measurement only, so the
landing risk is nil.

## NEXT

- Add section 7 to `ROADMAP/evidence/initramfs-slim2-20260922.md` recording the
  katana precheck below, update `docs/boot-v2.md`'s `esp-too-small` row and
  `ROADMAP/evidence/migrate-preconditions-20260921.md` §5 (both still say the
  512 MiB ESP is the blocker), commit, push, re-mark LANDABLE with the new sha.

## DONE

- **`97850c6d` pushed to `origin/task/initramfs-slim-2`** —
  `ROADMAP/evidence/initramfs-slim2-20260922.md`, 239 lines, the six
  measurements below. Branch cut from `origin/roadmap/v2.2` @ `f3b1b3d4`.

- **Item 5 (cross-BUILD reproducibility) is ANSWERED and it splits in two.**
  All measurements below are on this machine today, recorded in
  `/var/lab-scratch/initramfs-slim-2/`.

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

### **katana can migrate. It is the first APEX machine that can.**

`sudo apex-boot-migrate precheck --explain` on katana, booted on the slim
image — the verb documents itself as writing nothing and mounting the ESP `ro`,
and `status` was read first:

```
OK      esp-choice   bootc will write PARTUUID 99af3362-… of 1 ESP(s) on the root's disk
OK      esp-space    503 MiB free, 350 MiB needed
NOTE    esp-changes-disk  boots from PARTUUID 2ba9a2ea-…, will write 99af3362-…
This machine can migrate.                                               rc=0
```

Every check OK. Before this image the same machine refused **`esp-too-small`,
503 MiB free against 1173 MiB needed**. The initramfs work is the only change
between those two runs. The engine's own arithmetic says **350 MiB**, which
matches the build log's `3 x 100.9 + 48` to the rounding.

**Three other units' open items fall out of that one command:**

* `sdboot-xbootldr` NEXT #6 — *"the cross-disk note … has never fired in a
  guest"*. It has now fired on **real hardware**: `NOTE esp-changes-disk`,
  BootCurrent's PARTUUID `2ba9a2ea…` (the Windows disk) against the ESP it will
  write, `99af3362…` (APEX's own). `esp-choice` picks the root's disk exactly as
  that unit's source reading predicted.
* `sdboot-xbootldr` NEXT #3 — *"apex-os:daily ships no rsync, the FIRST refusal
  a real machine gets"*. `OK tools: mkfs.vfat, rsync and podman are all in this
  image`. Fixed by this build, as predicted.
* `migrate-preconditions` — katana's 154 MiB per-deployment ceiling is met with
  53 MiB to spare (100.9), and its `esp-too-small` line is now stale.

### ITEM 5 — the answer, measured

**Content: reproducible. Layer: not.** Two independent
`podman build --no-cache --layers=false` runs from the *same* parent
(`ghcr.io/andrenijman/apex-os:core-68ca7094…`), running the exact
`Containerfile.apex` dracut stanza (`/var/lab-scratch/initramfs-slim-2/repro/`):

| | build a | build b |
|---|---|---|
| `initramfs.img` sha256 | `4569c3b0…d881` | `4569c3b0…d881` |
| bytes | 88,934,458 | 88,934,458 |
| `cmp` | — | **byte-identical** |
| image last layer | `42760fdc…b285` | `a096af77…fb60` — **DIFFERENT** |
| `initramfs.img` mtime | 03:32:33 | 03:33:03 |

So **`find_vmlinuz_initrd_duplicate` CAN fire** (bootc digests content), and the
landed sentence in `Containerfile.apex:~240` — *"unchanged theme + unchanged
kernel yield a byte-identical initramfs and the layer digest stops moving"* — is
**half true**: the initramfs half is right, the layer half is wrong. The layer
moves because the tar records `initramfs.img`'s mtime. There is no
`--timestamp` and no `SOURCE_DATE_EPOCH` in `.github/workflows/build-image.yml`.

**One flag closes it, demonstrated:** two more `--no-cache` builds with
`podman build --timestamp 0` gave the **same last layer** `0c81bf74…53ec` *and*
the same image id `d920817c…57af` both times. Stated as a lever, not a
recommendation — `--timestamp` rewrites every mtime in the tier and its
interaction with ostree/bootc is untested.

### ITEM 5, the practical half — it has never fired, and the registry proves it

14 published `apex-<sha>` tags inspected with `skopeo inspect --raw` (no pulls;
manifests cached in `/var/lab-scratch/initramfs-slim-2/manifests/`):

* the tier is built `--layers=false` (`build-image.yml:1339`) so **the whole of
  `Containerfile.apex` is ONE layer**;
* **14 of 14 have a distinct final layer digest** — no APEX update has ever
  reused the previous one;
* every one of the 14 sits on **its own** `base-<same sha>`, so no two apex
  builds have ever shared a parent — dedup never had the chance;
* the fat-era layers are not even the same SIZE as each other (358.1, 358.2,
  359.0, 359.4, 359.5 MiB), so the content differed, not just the timestamps.

**Answer for `sdboot-xbootldr` NEXT #2, in one line:** an APEX update has never
left the initramfs byte-identical and never will while every build rebuilds
`base`, but the initramfs itself IS bit-reproducible from an unchanged parent,
so the lever is "stop rebuilding base when nothing changed", not "make dracut
deterministic" — that part already works.

### A 275 MiB-per-update download win nobody claimed

Same registry data: the apex-tier layer is **84.5 MiB compressed at
`apex-44c9a5cb`** against **358-360 MiB for all 13 earlier builds**. Because
the tier is one squashed layer and its digest moves every build, every APEX
machine re-downloads that layer on every update. The slimming therefore takes
**~275 MiB off every single `bootc upgrade`**, which is a `docs/update-cost.md`
number and is not in the evidence file.

### The ESP claim is no longer arithmetic — katana has three deployments on disk

The landed evidence says "No real 512 MiB ESP was filled … arithmetic over
measured file sizes, not an observed three-deployment ESP". It is now observed.
katana is booted on **exactly this image** (`bootc status`: image
`ghcr.io/andrenijman/apex-os:apex-44c9a5cb…`, digest `sha256:06ba23c3…`,
timestamp 2026-09-21T17:42:03Z) and carries three deployments:

```
101M  /boot/ostree/default-2e8ac7c8…   <- slim, this build
376M  /boot/ostree/default-ed5f1247…   <- fat
376M  /boot/ostree/default-93133f9a…   <- fat
```

Exact shipped bytes read off katana: `initramfs.img` **88,935,408 B**,
`vmlinuz` **16,906,312 B** → **105,841,720 B = 100.94 MiB per deployment**,
`need = 3 x 100.94 + 48 = 351 MiB`. katana's own ESP is `nvme1n1p2`,
**512 MiB** → **fits with 161 MiB spare**. Against `migrate-preconditions`'
ceilings (katana 154 MiB, L16 180 MiB) both clear.

### `/root` is a dangling symlink in the image, and dracut says FAILED then exits 0

Cause found, not guessed: in `core-68ca7094`, `/root -> var/roothome` and
`/var/roothome` **does not exist** (bootc puts it in `/var`, which is empty in
the image). So `dracut-install -f /root` fails, dracut prints
`dracut[E]: FAILED: …` — and returns **0**. Round 39's lab chroot bind-mounted
`/var`, which is exactly why it recorded `var/roothome` as an "extra entry" and
never saw this. Harmless (nothing needs `/root` in an initramfs) but it means
**dracut's exit status does not cover this class of failure**; the budget
predicate is what actually catches a bad initramfs.

## BLOCKED ON

- nothing
