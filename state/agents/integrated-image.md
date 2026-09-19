# integrated-image — build the integrated image and qualify it on katana

Repo: apex-os. Branch `task/integrated-image`, cut 2026-09-19T18:32Z off
`roadmap/v2.2` @ `97c9e8f2`. Worktree `/var/tmp/apex-work/wt-integrated-image`.
Tip `d6bea820` = 97c9e8f2 + ONE evidence-only commit (no image content).

## THE IMAGE EXISTS AND IS GREEN END TO END
**Run 35461554871 completed `success` at 2026-09-19T19:45Z, 1 h 12 m.**
`changes` success, `rust` success, `core` success (13 steps, none skipped),
`base` success, `image` success (17 steps, none skipped), `installer-iso` and
`qcow2` skipped because their dispatch inputs default false.

```
ghcr.io/andrenijman/apex-os:apex-97c9e8f25ee55593a97502505f51c6115ebbee7c
  digest sha256:55fc9e4ee9b2c1b27045cba5a594a40e97fb0d73c70170cfbe0003f5cd4d73b0
```
and the log says `not publishing from refs/heads/task/integrated-image:
per-SHA tags written` — no floating tag moved, the fleet is untouched.

**Two of the four rows are already answered BY THE BUILD** (hardware still
owes the other half of each):
- p2-b: `catalogue: compiled 1 catalogue(s) from apex-shell 03d77f96…`,
  `Apex.I18n built and proved to load: 29776 bytes`, `Apex.I18n stage
  complete`, `catalogue: apex-shell_de.qm loads under LANG=de`. The in-build
  offscreen probe's `APEXI18N: registerTypes uri=Apex.I18n` grep passed. This
  is the FIRST full base build ever to carry the catalogue stage.
- coredump: STEP 137 copies the drop-in, STEP 138 asserts the merged config
  and prints `coredump storage bounded: MaxUse=256M KeepFree=2G (default was
  a 4 GiB cap)`.

## IMAGE BUILD — THE RUN ID, RECORDED FIRST
- **Run 35461554871**, workflow_dispatch on `task/integrated-image` @ 97c9e8f2,
  queued 2026-09-19T18:33:05Z.
  https://github.com/AndreNijman/apex-os/actions/runs/35461554871
- `changes` job READ, not assumed:
  `Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/integrated-image): 03d77f96f521df7501d710d5c15ae8d0074a39ce`
  — the current apex-shell tip, which carries the Apex.I18n work. The
  12e9d454 ordered fallback did its job; this is NOT a main-shell build.
- `core rebuild: true (core sources changed)` → full ~1 h 20 m build, and
  katana's pull will be the whole image rather than tens of MB.
- `PUBLISH: false` → per-SHA tags only, no floating name moves, fleet untouched.
  Expected artifact:
  `ghcr.io/andrenijman/apex-os:apex-97c9e8f25ee55593a97502505f51c6115ebbee7c`
- FALLBACK IF IT FAILS: run **35457162588** (success, 1 h 17 m, task/sbom-attest
  @ `fd456c5a`). `git diff fd456c5a 97c9e8f2` touches ONLY
  `.github/workflows/build-image.yml` and deletes `sbom-probe.yml`, so its
  image content is this tree's. Confirm the shell sha it resolved before
  treating it as equivalent. Tag
  `apex-fd456c5acf1531beeb1e90cf050a1d9c3a47ef93`.

## Boundaries (from the brief, not negotiable)
- **Do not touch the L16.** Booting it is Andre's half of unit `final`.
- **Do not merge to main, do not open a PR.** When Andre does it:
  apex-shell `roadmap/v2.2` -> `main` FIRST (Containerfile.base carries
  `ARG APEX_SHELL_REF=main`), THEN apex-os `roadmap/v2.2` -> `main`.
- katana: APEX disk = Micron serial `220534D1CB81` (nvme0n1 today, 953.9 G,
  carries `/var` on p3). Windows = SPCC serial `240023925111005` (nvme1n1
  today, 1.8 T) and it holds APEX's own `Boot0000` ESP plus Andre's games.
  **Never write to it.** `/dev/nvmeXnY` is not stable; address by serial.
- greetd is boot-critical: arm `qual-greetd-restore` FIRST, use
  `greetd-set.sh` / `greetd-restore.sh` (never a bare restart — the
  `/run/greetd.run` runfile trap), and finish with `cmp` against
  `/etc/greetd/config.toml.orig-qual2`.

## Katana starting state (measured 2026-09-20 ~02:3x AWST, before any change)
- booted `apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`,
  digest `sha256:be3bdd0c…3aafb`, deployed 2026-09-19T10:09Z
- rollback `apex-266dcc572c51bdf9ec421d79eaa8784583184cd2`,
  digest `sha256:ba263890…503d` — **now PINNED** (`ostree admin pin 1`) so the
  switch + reboot cannot prune it. Unpin with `ostree admin pin -u 1`.
- `/var` 954 G, 53 G free (95 % used), on the APEX Micron. `/` composefs.
- extension `/var/lib/extensions/apex-user.raw` 535 928 832 bytes,
  sha256 **`eb3b8ba056d8f6f3caca2c3ed3204bda077f5f5f2fa426ccf264e385622c28a6`**.
  **The brief's `99749240…` is STALE** — the katana-image-qual run rebuilt it
  at 18:31 AWST. `eb3b8ba0…` is the number that has to move.
- `sudo jq -r .pkg_compat_level /var/lib/apex/pkg/state.json` → **2**
  (booted engine `/usr/libexec/apex-pkg` line 99: `PKG_COMPAT_LEVEL=2`;
  the tip's line 119 reads 3 — this is the pair that makes the row testable)
- `apex-sysext-rebuild.service`: active (exited), **started and finished in the
  same second** (18:29:40 → 18:29:40) — the defect's exact signature.
- `ls /usr/share/vulkan/icd.d/ | grep -c i686` → 13 already (pkg-share landed
  on the booted image; it must still read 13 AFTER the rebuild).
- `systemd-analyze cat-config systemd/coredump.conf` → no `MaxUse`/`KeepFree`;
  no drop-in dir at all. `/var/lib/systemd/coredump` 26 M.

## A HOST-BREAKING DEFECT FOUND AND FIXED ON THE WAY IN
`rpm-ostreed.service` would not start: 217/USER, "Failed to update dynamic user
credentials: Permission denied", persistent. It was **not rpm-ostreed** — a
bare `DynamicUser=yes` probe failed the same way, so the whole facility was
down. SELinux Enforcing, **no AVC logged**. PID 1 debug logging named it:
`Cannot open /etc/.pwd.lock: Permission denied`; the file carried
`rpm_var_lib_t` where policy wants `passwd_file_t`.

`apex install` put it there. `relabel_tree()` runs `setfiles` on the extraction
tree's `/usr` and `/opt` only; `install_etc` copies into the live `/etc` with
`cp -a`, which preserves the source label. **7 of the 11 paths in
`/var/lib/apex/pkg/etc.list` were mislabelled.** `restorecon` on the seven
fixed it and it is verified (DynamicUser probe succeeds, `rpm-ostree status`
returns, `rpm-ostreed` active). Full account, including the engine fix that was
deliberately NOT made here and the one read-only command to check the L16:
`ROADMAP/evidence/katana-pwd-lock-selinux-20260920.md` (commit `d6bea820`).

## The four things to verify, and the discriminators (from the `closed` notes)
1. **pkg-update** — journal line `level 2 -> … level 3 — rebuilding`; duration
   in MINUTES; `state.json` → 3; extension sha off `eb3b8ba0…`; i686 ICDs 13;
   `comm -12` of the extension file list against `rpm -qal` EMPTY. The journal
   line and the duration are load-bearing; the sha alone is not.
   Katana rebuilds **once redundantly** — that is the mechanism, not a defect.
2. **gaming-release** — `docs/gaming-and-sessions.md` §6.6 and §6.7.
   `active : false`, `apex-game` cgroup gone, `sched_ext/state` → `disabled`,
   apexd's `the session owner is gone`. `apex game status` must show
   `owner_pid` ≠ 0 or the row cannot pass. **The CPU governor is NOT a
   discriminator here** (katana's AC default tier is already `performance`).
   Plus: mangoapp no longer crash-loops, and whether the MangoHud overlay
   actually RENDERS with `APEX_GAMING_EXPOSE_WAYLAND=0` (it never has).
3. **p2-b** — `/usr/lib64/apex-shell/qml/Apex/I18n/libapexi18n.so` present in
   the booted `/usr`, and the shell loads it
   (`APEXI18N: registerTypes uri=Apex.I18n` in the greeter's journal).
4. **coredump drop-in** — `systemd-analyze cat-config systemd/coredump.conf`
   → `MaxUse=256M` and `KeepFree=2G`.

## PREP ALREADY DONE ON KATANA (so a stranger does not redo it)
- `/var/tmp/apex-work/scratch-integrated-image/` holds the PRE snapshot
  (`ext-pre.txt` 3 564 paths, `image-pre.txt` 266 093, `shadow-pre.txt` 0,
  `state-pre.json`, `ext-sha-pre.txt`) and **`post-boot.sh`**, which runs all
  four rows plus the `.pwd.lock` recurrence audit. Run it AFTER the sysext
  rebuild reaches a terminal state, not before — `state.json` reads 2 and the
  ICD count can read 0 while it is still `activating`.
- The greetd helpers from the last run are intact and were re-read:
  `/var/tmp/apex-work/scratch-katana-image-qual/greetd-set.sh`,
  `greetd-restore.sh`, `measure-session.sh`, `measure-pkgshare.sh`.
  Session id for Gaming Mode is **`apex-gaming`**.
  Do NOT copy the old card's `--on-calendar='2026-09-19 20:45:00'`; it is in
  the past and fires immediately. Use `--on-active=30min`, unit name
  `qual-greetd-restore` (that is the name `greetd-restore.sh` stops).
- Two readings that will mislead a stranger, both checked in the source:
  * `owner_pid` is inserted into `apex game status` ONLY while a session is
    active (`apexd/apexd/src/game.rs` ~457). Its absence on an idle machine is
    by design. The idle discriminator is
    `strings -a /usr/bin/apexd | grep -c 'the session owner is gone'`.
  * The greeter's stderr never reaches the journal (greetd dup2s the VT onto
    the session stdio), so grepping the journal for `APEXI18N` proves nothing.
    Use the Containerfile's own offscreen probe —
    `/usr/lib64/qt6/bin/qml -platform offscreen` with
    `QML_IMPORT_PATH=/usr/lib64/apex-shell/qml` — which `post-boot.sh` does.

## SWITCH DONE, REBOOT PENDING
`bootc switch` is COMPLETE and the new deployment is **staged**; `ostree admin
status` shows three deployments and the rollback still `Pinned: yes`.

**A trap that cost 15 minutes here, write it down:** `bootc switch` run
straight over ssh is SIGHUPed when the ssh channel closes, and the Bash tool's
600 s cap closes it. The pull (6.3 GB, 67 of 113 layers) survived in the image
store but nothing was staged, and `bootc status` said `staged: none` — which
reads exactly like a failed pull. Re-run it as a detached transient unit:
`sudo systemd-run --unit=intimg-switch --collect
--property=TimeoutStartSec=infinity --service-type=oneshot /usr/bin/bootc
switch --transport registry <IMG>`, then poll `journalctl -u intimg-switch`.
The second run found `No changes` and staged in 10 s.

Next step is a CLEAN `systemctl reboot` — a crash before a clean shutdown
discards a staged update and it reads as "updated but nothing changed".

## NEXT
After the reboot: wait with a BOUNDED ssh retry loop (the in-boot rebuild
makes first boot slow), wait for `apex-sysext-rebuild.service` to reach a
TERMINAL state (`active` or `failed`) before running `post-boot.sh` — while it
is `activating`, `state.json` still reads 2 and the ICD count can read 0 — and
work the four rows above in that order. `apex update` is not the
route — katana tracks a per-SHA tag that never moves and a non-main dispatch
moves no floating tag; the unit's re-open condition is "an image built from a
tip carrying b512cf12, taken by a machine that already has an extension", and
`bootc switch` satisfies it.
