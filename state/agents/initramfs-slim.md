# initramfs-slim — make the initramfs fit a 512 MiB ESP

items: none (no task id; dispatched as "it has to work in 512")
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim
branch: task/initramfs-slim, cut from roadmap/v2.2 @ 4031d43f
lab: /var/lab-scratch/initramfs-slim
evidence: ROADMAP/evidence/initramfs-slim-20260921.md (being written)

## STATE — round 2 (fresh agent, 2026-09-21 ~11:45 AWST)

Round 1 left `847b5bfb` (Containerfile.apex + .core + .kernel) and measured
artifacts in `/var/lab-scratch/initramfs-slim/out`. It explicitly did NOT do:
the evidence file, a VM boot proof, the reproducibility answer.

### Confirmed on arrival (checked, not assumed)

* Host kernel config `/usr/lib/modules/7.2.3-cachyos2.fc43.x86_64/config`:
  `CONFIG_DRM=y CONFIG_DRM_SIMPLEDRM=y CONFIG_SYSFB_SIMPLEFB=y
  CONFIG_DRM_FBDEV_EMULATION=y CONFIG_FRAMEBUFFER_CONSOLE=y` — all five `=y`.
  The Containerfile.kernel assertion CAN pass.
* This L16's own boot log: `[drm] Initialized simpledrm 1.0.0 for
  simple-framebuffer.0 on minor 0`. The GOP path works on the target hardware.
* Sizes (bytes): baseline 375,738,711 · varA 138,952,921 · varB 103,248,585 ·
  varB19 99,981,645 · varC 102,200,674.
  varB == repro/a.img == repro/b.img, sha256
  `704337bc653941d29f10a8baec5716be1177c7bba581310bceac7109ce8e578a`.

### DEFECT FOUND IN ROUND 1'S OWN GATE — fix before anything else

`Containerfile.apex` asserts `! grep -qE 'kernel/drivers/net/ethernet/'`.
The slim initramfs round 1 produced **contains**
`kernel/drivers/net/ethernet/broadcom/cnic.ko.zst`, `.../chelsio/cxgb4/cxgb4.ko.zst`
and `.../qlogic/qed/qed.ko.zst` (pulled in as deps of SCSI offload modules, not
by the `network` dracut module). So that gate FATALs on the image it exists to
pass — the "Containerfile assertions that cannot pass" family. Must be replaced
with a true predicate.

## NEXT

1. [done] read 847b5bfb, verify kernel config, verify repro hashes
2. [done] card
3. fix the ethernet gate; move the gates into a script both the Containerfile
   and a tests/ suite call, so both-ways can be demonstrated without a build
4. regenerate the size matrix with attribution (4 dracut runs, apex-tier flags)
5. cross-BUILD reproducibility (answers sdboot-xbootldr NEXT #2)
6. VM boot proof on a reflink copy of /var/lab-scratch/sdboot-xbootldr/apexgrub.img
7. evidence file + commit + push --force-with-lease

## CONSTRAINTS I AM UNDER

Never land on roadmap/v2.2. Never push main. Headless only. /var/lab-scratch
not /tmp. podman/qemu in the FOREGROUND.

---

## ROUND 38 CONTINUATION — written by the orchestrator, 2026-09-21 13:55 AWST

The round-2 agent died at the 12:06 shutdown. Its NEXT items 1-2 were done;
3-7 were not started (tree clean at `847b5bfb`, nothing new on disk).

### What the orchestrator changed

- Pushed `847b5bfb` to `origin/task/initramfs-slim` with `--force-with-lease`
  (origin was the stale `26d92144`). The branch is safe on origin now.
- `origin/roadmap/v2.2` is `71bc2177` (luks-boot + windows-installer-3 merges:
  `installer/`, `windows-installer/`, `tests/suites-not-in-ci.txt`). Merge it
  first; no overlap with your three Containerfiles.

### Two other agents touch Containerfile.core this round

- `kernel-publish` pins `ARG APEX_KERNEL_IMAGE=…@sha256:…` at line 93.
- `kernel-akmods` rewrites the akmods RUN (~lines 596-616) so its failure
  path is reachable.

Run `git show --stat 847b5bfb` and note which hunks of Containerfile.core
you touch. If it is the dracut section only, merges stay clean; if you go
near either of those, merge their branch first rather than resolving later.

### Who consumes your number

`migrate-preconditions` decides "does this ESP fit" with
`need = per_deployment*3 + 48 MiB`, today against 374.3 MiB per deployment
(vmlinuz 16.9 MB + initramfs 375.6 MB), which fails a 600 MiB ESP by ~2x.
Your varB is 99.98 MB / 103.2 MB — roughly 16.1 + 98.5 ≈ 115 MiB per
deployment, need ≈ 392 MiB, which FITS 512. Put the final per-deployment
figure and the `need` it implies in your evidence file in one quotable
line, so they cite a measurement rather than this arithmetic.

### NEXT (items 3-7 stand; sharpened)

3. Fix the ethernet gate (`! grep -qE 'kernel/drivers/net/ethernet/'`
   FATALs on the image it exists to pass: cnic/cxgb4/qed arrive as SCSI
   offload deps). Replace with a true predicate; move all the gates into a
   script both the Containerfile and a `tests/` suite call, and show it
   red on a fat initramfs and green on varB without a build.
4. Size matrix with attribution (4 dracut runs, apex-tier flags).
5. Cross-BUILD reproducibility (answers sdboot-xbootldr NEXT #2).
6. VM boot proof on a reflink copy of
   `/var/lab-scratch/sdboot-xbootldr/apexgrub.img` — check it still exists
   first; headless qemu; `systemd-run --user`, never `nohup &`.
7. Evidence file (`ROADMAP/evidence/initramfs-slim-20260921.md`, apex-os
   repo) + commit + push.

Battery was 58% and DISCHARGING at 13:42. Read
`/sys/class/power_supply/BAT*/status` before the boot proof or any build
over ten minutes; one heavy podman/qemu job at a time on this laptop.

---

## ROUND 39 CONTINUATION — written by the orchestrator, 2026-09-21 17:20 AWST

The round-38 agent was killed by a **session usage limit at 14:59 AWST**
(`ROADMAP/state/autoresume.log`: `resume session ended (exit 1)`). It was never
messaged. You are a FRESH agent and this card is your whole inheritance —
everything above stands unless this section contradicts it, and where it
contradicts it, this section wins.

**Hard deadline: this orchestrator runs under `timeout 4h` and dies at about
21:08 AWST.** Commit and push small and often. Update this card after every
commit and whenever NEXT changes — a card that is only correct at the end is
worth nothing, which is the entire reason this directory exists.

**Write `## LANDABLE` at the top of this card, with the sha, the moment your
branch is ready to merge onto `roadmap/v2.2`.** The orchestrator lands on that
signal and will not guess. If it is NOT landable, say why in one line —
"landing this would break X" is a finding, not a failure.
__BODY_

### The contract (ROADMAP/state/README.md, short form)

Keep this card's `NEXT` / `DONE` / `IN PROGRESS` / `FOUND` / `BLOCKED ON`
sections current **as you go, never at the end**. `NEXT` is load-bearing: one
line, the exact next action, specific enough that a stranger could do it.
Everything else can be re-derived from git; the next action cannot.

### Constraints (non-negotiable)

- Never push `main`, never open a PR — final integration only.
- **Headless only.** Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit and no keyring prompts (`sudo` / `--user`, never an agent helper).
- Never `pkill apex-agentd`.
- Do not interrupt gaming on katana.
- Scratch goes in `/var/lab-scratch/<your-slug>/`, NOT `/tmp` (tmpfs, 15 GB on
  29 GB RAM — a stdout-only Bash failure there is memory, not disk). The
  scratchpad is shared between agents: use your own subdirectory.
- Long builds run in the FOREGROUND or under `systemd-run --user`; a
  backgrounded `podman` gets SIGTERMed and still exits 0.
