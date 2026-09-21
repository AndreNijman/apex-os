## LANDABLE — `ede9a6f7`

The gate that FATALed on this branch's own initramfs is gone, replaced by a
predicate a CI suite proves red on a fat image and green on a slim one (35
assertions), and the evidence file exists. Landing gives a 512 MiB ESP a
deployment of 114.6 MiB instead of 375.2 MiB.

**The one caveat, stated rather than buried:** no full image build has run the
new stanza end to end — the first build after landing is the first time
`COPY files/scripts/check-initramfs-budget.sh` + the call execute together.
Every static checker this repo has is green (containerfile-order,
containerfile-assertions, shellcheck-coverage, suites-run-in-ci, the new suite,
YAML parse). The risk that remains is a build-time one, and the new suite is
exactly what makes it two seconds to diagnose instead of forty minutes.

Confidence rests on a specific fact, checked not assumed: the **ethernet gate
was never on `roadmap/v2.2`** — round 1 introduced it on this branch — while
the eight unlock-chain gate references **are** already on `roadmap/v2.2` and
have passed real builds. So this branch no longer adds a build-breaking gate,
and it changes none of the gates that already work.

---

# initramfs-slim — make the initramfs fit a 512 MiB ESP

items: none (no task id; dispatched as "it has to work in 512")
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim
branch: task/initramfs-slim, cut from roadmap/v2.2 @ 4031d43f
lab: /var/lab-scratch/initramfs-slim
evidence: ROADMAP/evidence/initramfs-slim-20260921.md (being written)

## ROUND 39 — LIVE STATE (newest; wins over every section below)

agent: round-39, started 17:18 AWST 2026-09-21. Orchestrator dies ~21:08.
branch: task/initramfs-slim @ 530a9dc6 (merged origin/roadmap/v2.2 b137f03f,
        clean — no overlap on Containerfile.{apex,core,kernel})

### LANDABLE as of 20:35 — see the block at the top of this file.

(The round opened NOT landable: the Containerfile gate FATALed on the very
initramfs the branch produces. That is fixed in `7d9b959a`.)

### INVENTORY CORRECTIONS (checked on arrival, contradicts the notes below)

* The round-38 note "tree clean at 847b5bfb, nothing new on disk" is WRONG.
  Round 2 left a great deal in /var/lab-scratch/initramfs-slim/vm:
  - `apex.img` — a REAL 48 GiB bootc-installed APEX disk.
  - `mk-initramfs.sh` — builds one variant inside the deployment chroot with
    the exact flags Containerfile.apex uses (bind-mounts conf.d, /opt, /var).
  - `run-qemu.sh` — headless OVMF boot, serial to a file, `-vga std`.
  - `hook/99-apexprobe.sh` — pre-pivot hook writing /run/apexprobe-initrd.txt
    (DRM driver bound, module count, netifs, NM, cryptsetup) — survives
    switch-root because /run is the same tmpfs.
  - `confs/v0..v4.conf` — the ATTRIBUTION matrix, already defined.
  - `out/v0-nohook.img` (377M) and `out/v1-nohook.img` (291M) already BUILT;
    v2 was killed mid-run (log exists, no image).
  These are NOT the same variants as round 1's varA/varB/varC — varA is 139 MB
  where v1 is 291 MB. Do not map one onto the other.
* Round 38 (13:55-14:59) produced exactly one artifact: `varB.modules.dep`.
* The loop device is GONE (reboot). `vm/loop.env` says /dev/loop2; nothing is
  attached now and nothing under /var/lab-scratch is mounted. Re-attach with
  `losetup -fP --show` before using `dep.env`'s deployment path.

### FOUND — round 39

* **The ethernet gate is broken worse than the card said.** varB also contains
  `drivers/net/wireless/` — libertas, libertas_sdio, mt76, mt76-sdio,
  mt76-connac-lib, mt792x-lib, mt7921-common, mt7921s. Eleven files, 1.5 MiB.
* **They arrive by THREE unrelated paths, not "SCSI offload deps":**
  - `cnic` <- `scsi/bnx2fc` and `qed` <- `scsi/qedf`  (FCoE offload) — 2 files
  - `cxgb4` <- `crypto/chelsio/chcr`  (a CRYPTO accelerator, not SCSI) — 1 file
  - libertas* and mt76* <- SDIO wireless (`*_sdio`), pulled along the MMC/SDIO
    block path — 8 files
  So no `omit_dracutmodules` change can remove them; they are dependencies of
  modules that are in the initramfs for storage and crypto reasons.
* **The real, quantified predicate**: `kernel/drivers/net/` goes from
  **322 files / 21.9 MiB** (baseline) to **11 files / 1.5 MiB** (varB). The
  regression this gate exists to catch is the 20.4 MiB `kernel-network-modules`
  tree coming back, and a bounded size/count assertion catches it while the
  dependency residue passes.
* **The green fixture MUST be deployment-built, not host-built.** varB.list was
  built on the L16 host and is MISSING `apex-unlock-hint`,
  `apex-unlock-hint.service`, `apex-vconsole-credential` and its
  `sysinit.target.wants` symlink (0 matches each) — the host has no APEX dracut
  modules. A host-built listing therefore fails four unlock-chain gates for a
  reason that has nothing to do with this branch.

### MEASURED — round 39, the attribution matrix (NEW, supersedes round 1's numbers)

All five built in the SAME deployment chroot (`vm/mk-initramfs.sh`, the real
bootc-installed APEX deployment, kernel **7.2.5**-cachyos1 — NOT the L16 host's
7.2.3), with the exact flags Containerfile.apex:244 uses. Bytes, then what the
step bought:

| variant | what it adds | bytes | MiB | delta |
|---|---|---|---|---|
| v0 | as shipped (`add_drivers+=nvidia…`) | 376,595,424 | 359.1 | — |
| v1 | minus `add_drivers` (RPMFusion default) | 291,208,822 | 277.7 | **-81.4** |
| v2 | + omit nouveau/amdgpu/i915/xe/radeon/amdkfd/nvidia* | 138,978,697 | 132.5 | **-145.2** |
| v3 | + omit network/nfs/nvmf modules (**what the branch ships**) | 103,256,987 | 98.5 | **-34.0** |
| v4 | v3 with `compress="zstd -19"` in dracut.conf.d | 103,256,987 | 98.5 | **0.0** |

* **The per-deployment number, quotable:** vmlinuz 16,910,408 + initramfs
  103,256,987 = **120,167,395 B = 114.6 MiB per deployment**, so
  `need = 3 x 114.6 + 48 = 391 MiB`. **Fits a 512 MiB ESP with ~120 MiB spare.**
  Today's shipped image is 16,910,408 + 376,456,492 = 375.2 MiB per deployment,
  `need = 1173 MiB` — which is why every 512 MiB ESP is refused today.
* **v2 alone does NOT fit the 130 MiB ceiling (132.5 MiB).** Both omissions are
  load-bearing; the KMS one alone is not enough.

### FOUND — `compress=` in dracut.conf.d is dead code in this build

v3 and v4 are the SAME sha256 (`b6ca6196…3946`), byte for byte, and both record
`--zstd` in their dracut Arguments. `Containerfile.apex:244` passes `--zstd` on
the COMMAND LINE, which overrides any `compress=` in dracut.conf.d. So tuning
compression through the conf file cannot work and any future attempt to do it
there is a no-op that looks like a setting. (Round 1's host-measured "varB19
saves 3.2 MB" was produced without that command-line flag and does not
transfer.) Adopting -19 would mean changing the Containerfile's dracut flags.

### MEASURED — the reproducibility answer

* **Run-to-run: byte-identical.** v3 built twice in the same chroot ->
  `b6ca6196…3946` both times, `cmp` clean. `--reproducible` holds.
* **Lab chroot vs the real image build: faithful, not bit-identical.** Lab v0
  vs the initramfs the image build actually shipped on this disk:
  - the dracut MODULE LIST is **identical**
  - **zero** paths present in the shipped image are missing from the lab one
  - the lab one has **14 extra entries** and is 138,932 B larger (0.037%):
    `dev/{console,kmsg,null,random,urandom}`, `var/roothome`,
    `var/lib/nfs/statd*`, `usr/etc/*`, one authselect symlink — every one of
    them an artefact of the bind mounts that make the chroot runnable.
  So the lab predicts image content exactly and image BYTES to 0.04%. It does
  NOT prove cross-host bit-reproducibility, and `--reproducible` fixes
  timestamps, not the package set.

### FOUND — a stale comment the branch contradicts

`Containerfile.apex:226-229` still says core's 99-nvidia-dracut.conf "is what
puts nvidia/nvidia_modeset/nvidia_drm/nvidia_uvm INTO this initramfs". The
branch inverts exactly that. Must be rewritten with the gates.

### BOOT PROOF — done, and it took TWO boots

Same disk (reflink copy of the real bootc-installed lab disk), slim 98.5 MiB
initramfs installed over the shipped 359.0 MiB one, `-vga std`, dracut
pre-pivot hook reporting from inside the initramfs to /dev/kmsg, headless
qemu inside `localhost/apex-bootlab` (the host L16 has NO qemu and NO OVMF —
that is why the container exists).

| | boot 1, guest defaults | boot 2, KMS blacklisted |
|---|---|---|
| card0 driver | `bochs-drm` | **`simple-framebuffer`** |
| fb console | `fb0 fbcon` | `fb0 fbcon` |
| switch-root | reached | reached |
| initrd netifs | `lo` only | `lo` only |
| NetworkManager | absent | absent |
| systemd-cryptsetup | present | present |
| full boot | graphical.target 37.0 s | graphical.target **7.714 s** |

**Boot 1 is the trap.** bochs is still in the initramfs, so on `-vga std` it
binds card0 and evicts simpledrm — a guest run without the blacklist measures
the OLD design and "passes" while proving nothing. Boot 2 is the claim.
Serial logs: `/var/lab-scratch/initramfs-slim/serial-{1,2}.txt`.

### FOUND — the branch's own policy sentence was false

Stage 1f said "NO KMS DRIVER IS IN THE INITRAMFS". bochs, cirrus-qemu,
virtio-gpu, vmwgfx and ast are all still there. They carry no firmware — which
is the actual hazard — and are how a VM and a BMC server get a display, so
they stay; the sentence was corrected to "no FIRMWARE-BEARING KMS driver"
(`ede9a6f7`). This is also exactly why the boot proof needed two boots.

### FOUND — pre-existing, NOT mine, worth someone's time

`android/tools/release-version.sh` fails `check-shellcheck-coverage.sh`
(SC2034, `head_sha` unused at line 92) and is NOT on
`tests/shellcheck-known-failing.txt`. It is on `origin/roadmap/v2.2` already,
so that gate is red on the roadmap tip independently of this branch.

### NEXT

Nothing blocking. If the orchestrator wants more before landing: run one real
image build of Containerfile.apex and confirm the new
`/tmp/check-initramfs-budget.sh` call prints its two `initramfs-budget:` lines
and exits 0. Everything else on this unit is done and pushed.

### DONE — round 39

* merged origin/roadmap/v2.2 (b137f03f) into the branch at 530a9dc6, clean
* ran all 12 gates against varB.list — found the two defects above
* **7d9b959a pushed**: files/scripts/check-initramfs-budget.sh, the gates as a
  predicate. Verified BOTH ways on real artifacts (green on slim, red on fat
  naming gsp_ga10x.bin at 75 MB). shellcheck clean.
  - Its own first draft had the repo's pipefail defect INSIDE it:
    `printf | grep -qE` returned 141 on an early match (listing line 280) and 0
    on a late one, so 7 present files read as absent. Predicates now grep a
    real file. Recorded in the commit message.
* re-attached the lab disk (now **/dev/loop1**, was loop2), mounted the
  deployment; built v2/v3/v4/v3b; measured the full matrix + reproducibility.
* **a4c9ab97 pushed**: `tests/test-apex-initramfs-budget.sh` (35 assertions,
  both ways) + `tests/fixtures/initramfs/{fat,slim}.list.gz` + Containerfile.apex
  now COPYs and calls the predicate instead of ~140 inline greps + wired into
  `pr-validation.yml`. check-suites-run-in-ci green.
* **0ce5e9e4 pushed**: `ROADMAP/evidence/initramfs-slim-20260921.md` — the
  evidence file round 1 never wrote. Matrix, reproducibility, boot proof, and
  an explicit "what this does NOT prove" section.
* **ede9a6f7 pushed**: corrected the false policy sentence in stage 1f.
* Lab disk copy for the boot proof: `/var/lab-scratch/initramfs-slim/vm/
  apexboot.img` (reflink, disposable). `apex.img` is untouched.

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
### NEXT — rewritten for round 39

**Andre settled the ESP-ownership question at 17:09 today and it does NOT
retire this unit.** The decision (recorded in `ROADMAP/state/dispatch.json`
under `_decisions.esp_ownership_2026_09_21`, and landed as `bc5c3822` —
`docs/apex-owns-its-esp.md`) is that APEX builds its own ESP on Windows
machines rather than borrowing Windows'. Its own text says, verbatim:
*"does_not_retire: initramfs-slim. The 512 MiB ceiling still binds wherever the
ESP is ALREADY APEX's own — the L16 and every existing install. Andre's 'it has
to work in 512' was about the L16, which has no Windows on it."* Keep going.

Continue at step 3 of the NEXT list above, which is where the round-38 agent
was killed. In order:

1. **Fix the ethernet gate first — it is the whole "assertions that cannot
   pass" family and it is already proven broken on your own artifact.**
   `Containerfile.apex` asserts `! grep -qE 'kernel/drivers/net/ethernet/'`,
   and the slim initramfs round 1 produced CONTAINS `cnic.ko.zst`,
   `cxgb4.ko.zst` and `qed.ko.zst` — pulled in as dependencies of SCSI offload
   modules, not by the `network` dracut module. Replace it with a true
   predicate (the thing you actually mean is "the `network`/`nfs` dracut
   modules are absent", not "no file path contains the word ethernet").
2. Move the gates into a script both the Containerfile and a `tests/` suite
   call, so both-ways can be demonstrated **without** a 40-minute image build.
   `files/scripts/` + `tests/` is the shape the repo already uses. Remember the
   CI suite gate: a new `tests/*.sh` has to be reachable from CI or listed in
   `tests/suites-not-in-ci.txt` with a reason.
3. Size matrix with attribution — 4 dracut runs, apex-tier flags, which flag
   bought which megabytes. You have the numbers (baseline 375,738,711 ·
   varA 138,952,921 · varB 103,248,585 · varB19 99,981,645 · varC 102,200,674);
   what is missing is *why*.
4. Cross-build reproducibility (varB already reproduced byte-identically:
   sha256 `704337bc…e578a` across `repro/a.img` and `repro/b.img` — say what
   that does and does not prove).
5. A VM boot proof. The claim is "no KMS driver in the initramfs and the
   machine still gets a console" — that is measurable in a guest and is the
   only thing that turns this from a size win into a safe one. `simpledrm`
   works on the L16's real hardware (its boot log says so); the guest tells you
   whether the *slim* initramfs still reaches switch-root.
6. `ROADMAP/evidence/initramfs-slim-20260921.md` — round 1 explicitly did not
   write it. No evidence file, no landing.

**Your branch was pushed by the orchestrator at `86867c61`** (your merge of
`origin/roadmap/v2.2`; the round-38 agent had it locally and unpushed).
`847b5bfb` is NOT landed and must not land while the ethernet gate is in it —
it would FATAL every image build, which is precisely the defect class this
repo has already paid five days for.

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

---

## ROUTED FINDING — from `migrate-preconditions`, 2026-09-21 ~18:05 AWST

Routed by the orchestrator, not written by you. `migrate-preconditions` landed
as merge `69253336`; its evidence file is
`ROADMAP/evidence/migrate-preconditions-20260921.md`.

**Your target is not 512 MiB of ESP. It is a per-deployment ceiling, and the
two real machines give 180 MiB and 154 MiB.** Computed from the live
partitions rather than hardcoded, against `need = per-deployment * 3 + 48 MiB`:

| machine | ESP | per-deployment ceiling |
| --- | --- | --- |
| L16 | 600 MiB | **180 MiB** |
| katana | 512 MiB | **154 MiB** |

Today's cost is **374 MiB** per deployment (vmlinuz 16,898,120 B + initramfs
375,558,646 B on 7.2.3-cachyos2). So the binding number is 154, not 512, and it
is the *sum of kernel and initramfs*, not the initramfs alone — your 16.9 MiB
vmlinuz is inside the budget, leaving roughly **137 MiB** for the initramfs on
katana.

Your measured variants against that: varB 103,248,585 B and varB19 99,981,645 B
both clear it with room; the baseline 375,738,711 B does not, and varA
138,952,921 B clears katana only just once vmlinuz is added — 148 MiB of 154.
State which variant you are proposing against **154**, not against 512.

The same finding corrects `docs/apex-owns-its-esp.md`, which calls katana "the
cheapest first proof" of the ESP-ownership design. Needing no new partition is
true; it is not sufficient. The partition katana already has is 512 MiB, so it
fails the same fit check the L16 does. That is now recorded on the landed doc.
