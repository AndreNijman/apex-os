# katana-image-qual — the two run-books that were waiting on an image

items: units `gaming-gpu` and `pkg-share`, both RE-OPENED 2026-09-19 (round 33)
repo: apex-os
worktree: `/var/tmp/apex-work/wt-katana-qual2`, branch `task/katana-image-qual`
  (created 2026-09-19 17:12 AWST off `roadmap/v2.2` @ 7f647470)
evidence file: `ROADMAP/evidence/katana-image-qual-20260919.md` (new; the round-31
run is `ROADMAP/evidence/katana-qualification-20260919.md` and is already landed —
read it, do not overwrite it)
scratch: `/var/tmp/apex-work/scratch-katana-image-qual/`

## NEXT

**Waiting on the GHCR tag; everything that does not need it is already done.**
Poll: `skopeo inspect --no-tags
docker://ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`
— last checked 17:22 AWST, still `manifest unknown` (build dispatched 17:04
AWST, ~1 h). A background watcher is armed in this session; if you are a fresh
agent, just poll.

The moment the tag exists, in this order:

1. `ssh katana 'sudo bootc switch --transport registry ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e'`
   then `sudo systemctl reboot` (clean reboot, never a hard reset — a crash
   discards a staged update). Re-read `bootc status --json`; record the digest.
2. On the new image, prove the no-op first (evidence §0.2): `sudo
   /usr/libexec/apex-pkg rebuild --if-needed` and `sudo apex install steam`
   should both decline to rebuild. Then force it:
   `sudo systemd-sysext unmerge` followed by `sudo apex install steam 2>&1 |
   tee /var/tmp/apex-work/scratch-katana-image-qual/apex-install-steam.log`
   (foreground; `systemd-run --wait` if it needs to outlive the ssh).
   **Check `grep 'multilib: carrying'` in that log BEFORE believing any count**
   — its absence means the old extension was measured.
3. Re-run the §0.3 baseline block verbatim against the new `.raw` for the
   shadow count, and `ls /usr/share/vulkan/icd.d/ | grep -c i686`.
4. Then §6, using `/var/tmp/apex-work/scratch-katana-image-qual/greetd-set.sh
   <session-id>` + `sudo systemctl restart greetd`.
   **If you are a fresh agent and `/etc/greetd/config.toml` has an
   `[initial_session]` block, katana is auto-logging in as andre — undo it with
   `sudo /var/tmp/apex-work/scratch-katana-image-qual/greetd-restore.sh`,
   which restores the backup, `cmp`s it and restarts greetd.**

## The plan, written 2026-09-19 17:15 AWST while waiting for the tag

### R — rebase katana (step 1, not optional)

Katana boots `ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc57…`,
digest `sha256:ba263890…`, and `bootc status --json` confirms the transport is
`registry` with `store: ostreeContainer`. Rollback is the old `gaming-nvidia`
tag, `sha256:308127d9…`, still `Unlocked: hotfix`.

    ssh katana 'sudo bootc switch --transport registry \
      ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e'

then reboot and re-read `bootc status --json`; record the digest landed on.
Keep the previous deployment as the rollback (bootc does by default).

### P — pkg-share confirmation (gates one gaming-gpu row)

State on the OLD image, measured 17:12 AWST, so the deltas are attributable:
- `ls /usr/share/vulkan/icd.d/ | grep -c i686` → **0** (13 files, all x86_64)
- `/var/lib/extensions/apex-user.raw` → 540 012 544 B, built 2026-09-19 09:43
  by the round-31 run with the OLD engine. **It must be rebuilt by the new
  engine or the measurement is of the old one.**

1. Record the pre-state: extension size, `rpm -qal` intersect count (the §3.1
   shadow measurement, `LC_ALL=C` on every `sort`/`comm`), i686 ICD count.
2. `sudo apex install steam` on the NEW image, capturing the full log to
   `/var/tmp/apex-work/scratch-katana-image-qual/apex-install-steam.log`
   (foreground, or `systemd-run --wait`; a backgrounded long command is
   SIGTERMed and still exits 0).
3. Re-measure: i686 ICD count (must be non-zero — gate on P1-038 row 6), the
   §3.1 shadow count (was 177), and `grep 'multilib: carrying'` in the log with
   the image-owned vs native-pass split.
4. Positive control for §3.3: `gst-inspect-1.0 | tail -1` — was "2 features"
   with the shadowed i686 scanner, must be 1344.

### G — the gaming run-book, docs/gaming-and-sessions.md §6

**The greetd problem, and how this run answers it.** Round 31 could not log in
through greetd: `apex-session-select` deliberately does not arm autologin and
that unit did not have Andre's password. Neither do I, and I must not ask.
`sudo -n` on katana IS passwordless, so the route is:

- temporarily add an `initial_session` block to `/etc/greetd/config.toml`
  (backed up first, byte-compared and restored afterwards), naming the Exec
  line read out of `/usr/share/wayland-sessions/apex-gaming.desktop` and user
  `andre`; reboot; greetd itself launches the session on VT 1 through its own
  worker — same PAM service (`greetd`), same `pam_open_session`, same
  `setresuid`, same exec. Only the auth conversation is skipped, and the
  capability question §6.3 asks is about session *setup*, not about auth.
- Divergence to state honestly in the evidence: no PAM `auth` stack ran.
  Everything downstream of `pam_open_session` — which is where a capability
  would be granted, e.g. by `pam_cap` — is identical.
- Restore `/etc/greetd/config.toml` from the backup and `cmp` it before
  finishing, and confirm the greeter comes back on tty1.

Blocks, in order, all from §6:
- **6.1** `apex gaming` + `apex gaming --gamescope-device-args` BEFORE any
  reboot into anything. Expect `--prefer-vk-device 10de:249d --prefer-output
  HDMI-A-1`, rc=0, and the report's screen block naming card2. Then the
  greetd-launched Gaming Mode session, and the five log lines. Then the render
  proof: the monitor is the screen that lit, not the panel (`grim` per output,
  distinct-colour count, like round 31 §5).
  Machine fact confirmed 17:12: only two connectors are `connected` —
  `card1-eDP-1` (Intel) and `card2-HDMI-A-1` (NVIDIA).
- **6.2** `grep -E '^Cap(Eff|Prm|Amb):' /proc/self/status` inside the session,
  `getcap $(command -v gamescope)`, `apex gaming | grep -E 'realtime (limit|capability)'`.
  Expect limit yes / capability no, and the session log saying
  "CAP_SYS_NICE: absent" and NOT passing `--rt`.
- **6.3** the row this whole unit exists for. Only retest after P shows a
  non-zero i686 ICD count. Then read `CapEff/CapPrm/CapAmb` out of the session
  log and the bwrap line out of `~/.local/share/Steam/logs/console-linux.txt`.
  gaming-gpu's own closure says bwrap "refuses to start with any permitted
  capability", so CapPrm=0 from greetd + bwrap still failing moves the hunt to
  Steam's runtime; CapPrm non-zero attributes it to the image.
- **6.4** `/usr/libexec/apex-safe-graphics check` runs fine over ssh; the
  forced-device run needs a seat, so it goes through the same
  `initial_session` mechanism with the env var set in the command.
  `APEX_SAFE_GRAPHICS_DRM_DEVICE=/dev/dri/card2 /usr/libexec/apex-safe-graphics`.
  The automatic branch needs the panel genuinely dark — record whether it could
  be reached rather than claiming it.
- **6.5** the niri bar: needs one login to the niri session, then the five
  greps. Same `initial_session` mechanism, second reboot.
- **6.6** — §6 has 6.1–6.5 in the doc plus §6.6 "Gaming Mode cleanup" in the
  round-31 evidence numbering. Cover cleanup (`apex game status`, `pgrep -c -x
  gamescope`) after each Gaming Mode attempt.

### Rules, and the three that are about this machine

- **Katana's NVMe device names are not stable** — identify disks by serial,
  PCI function, PARTUUID or label. Never by `/dev/nvme*n1`.
- **APEX's Boot0000 lives on the WINDOWS ESP.** Do not tidy EFI entries.
- Reboots authorised; katana's own screen is the test. **Never open a window on
  Andre's L16 desktop.** Never run `qs -p`. Never `pkill apex-agentd`.
- `sudo` or `--user` only — no polkit or keyring prompts.
- A backgrounded long command is SIGTERMed and still exits 0.
- `ROADMAP/set-status.py` REPLACES evidence; read the existing text and prepend.
- Never push `main`; never open a PR; push only `task/katana-image-qual`.

## Orchestrator additions, round 33, folded 17:19 AWST

1. **The old extension is the false-negative trap, and one of the
   orchestrator's facts about it is stale.** The message said the `.raw` is the
   2026-09-06 one, 219 packages / ~337 MB. It is not: measured 17:16 AWST it is
   **540 012 544 B, built `2026-09-19T01:43:06Z`, 223 resolved packages**, i.e.
   the round-31 run rebuilt it TODAY — but with the image's engine, which
   predates pkg-share. The substance is unchanged and the warning stands: it
   survives a rebase and re-merges at boot, so a premature
   `ls /usr/share/vulkan/icd.d/ | grep -c i686` reads **0** and looks like
   pkg-share failed.
   **THE DISCRIMINATOR IS `multilib: carrying` IN THE INSTALL LOG** — only the
   new engine emits it (`merge_multilib`, `apex-pkg` ~line 698). Its ABSENCE
   means the September/round-31 extension was measured, not that the fix
   failed. Establish which engine ran before believing any count.
   *What I will do with the old `.raw`*: not delete it. `sudo systemd-sysext
   unmerge` makes `merged` false so `apex install steam` falls through to a
   real rebuild, and `activate_payload` replaces the file itself; the old
   size/sha (`99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed`,
   540 012 544 B) are recorded above and in the evidence before anything moves.
   The hand-copied-file manifest is `/var/lib/apex/steam-recovery-20260917/installed-files.jsonl`
   (1 687 289 B, 2026-09-17) and is left untouched.
2. **Disk checked rather than assumed** — `df -h /var` at 17:18:
   `954G size, 890G used, 60G available, 94%`. `/var`, `/sysroot` and `/boot`
   are the same filesystem. 60 G is ample for a multi-GB pull, so **no
   `rpm-ostree cleanup -r`** — that would drop the `gaming-nvidia` rollback
   deployment for no benefit, and `bootc switch` retires the oldest deployment
   on its own.
3. **HARD DEADLINE ~20:58 AWST** (orchestrator runs under `timeout 4h` from
   16:58). Priority if short: (a) rebase + digest, (b) pkg-share's three
   numbers, (c) the greetd login that attributes §6.3's bwrap cause, (d) 6.4-6.6.
   Evidence is written **per block**, committed and pushed as measured.

## DONE

- 17:12 — worktree `/var/tmp/apex-work/wt-katana-qual2` on
  `task/katana-image-qual` off `roadmap/v2.2` @ 7f647470.
- 17:12 — katana confirmed free: uptime 6:37, greetd on seat0+tty1, nobody
  logged in, no steam/gamescope/proton/wine process, `rpm-ostree status` idle.
- 17:12 — pre-rebase baseline captured (see plan step P).
- 17:15 — read §6 of `docs/gaming-and-sessions.md` end to end and §§3, 4, 6 of
  the round-31 evidence; plan above written.
- 17:22 — greetd arm/restore helpers written to
  `/var/tmp/apex-work/scratch-katana-image-qual/greetd-{set,restore}.sh` on
  katana (the restore one refuses without a backup and `cmp`s the result).
- 17:21 — **evidence §0 written, committed `b00e2ae6`, branch pushed.**
  §0.3 reproduces round 31's shadow count exactly on this deployment:
  3 716 extension files, 266 030 image paths, **177 shadows, 14 of them 32-bit
  ELF over a 64-bit image binary**, **0 i686 ICDs of 13**, extension has no
  `/usr/share/vulkan/icd.d/` at all, `gst-inspect-1.0` → **2 features**.
  That is the same-agent control the post-rebase numbers are a delta against.
- 17:20 — negative control on the old image (see FOUND).
- 17:18 — `df -h /var` on katana: 60 G available, no cleanup needed.
- 17:17 — PAM capability audit on katana (no `pam_cap`), `apex-pkg` rebuild
  short-circuit read out of the engine, extension/state baseline captured. See
  FOUND.

## IN PROGRESS

- polling GHCR for the per-SHA tag. Everything below the tag is prepared.

## FOUND

- (orchestrator, round 33) katana's booted image is 131 commits behind the
  integration tip and predates both fixes this unit is meant to qualify. Any
  run-book executed before the rebase measures the wrong build.
- katana has **passwordless `sudo`** for `andre` over ssh (`sudo -n true` → 0).
  That is what makes the greetd route reachable without Andre's password.
- `/etc/greetd/config.toml` has **no `initial_session`** today, so the greeter
  is the only way in — hence the temporary, restored autologin above.
- The extension on katana right now was built at 09:43 today by the **old**
  engine. `apex install steam` must rebuild it after the rebase or the
  pkg-share numbers measure the old rule.
- **No `pam_cap` anywhere, and no `/etc/security/capability.conf`** — measured
  on katana 17:17 AWST: `grep -i pam_cap /etc/pam.d/{greetd,system-auth,postlogin,login}`
  is empty and the file does not exist. **This is the fidelity argument for the
  `initial_session` route**: greetd's initial session skips `pam_authenticate`
  but still runs `acct_mgmt` -> `setcred` -> `open_session`, and nothing in the
  `auth` stack on this machine can grant a capability, so skipping it cannot
  change `CapPrm`. Re-run the same grep on the NEW image before believing it.
- The greeter's own process (`sway` as user `greetd`) already reads
  `CapPrm: 0000000000000000` / `CapAmb: 0` with `CapBnd: 000001ffffffffff`. So
  greetd hands down an empty permitted set before any of this.
- **`apex install steam` will NOT rebuild the extension after the rebase, and
  neither will the boot service.** `rebuild_extension` short-circuits with
  "already up to date" when the resolved rpm set, `os_version_id` and
  `pkg_compat_level` all match and the sysext is merged
  (`files/system/libexec/apex-pkg` ~line 1300), and
  `apex-sysext-rebuild.service` runs `rebuild --if-needed`, which returns 0 on
  exactly the same three-way match (~line 1613). Measured on katana:
  `VERSION_ID=43`, `state.json` `os_version_id: "43"`, `pkg_compat_level: 2`,
  built `2026-09-19T01:43:06Z`, 8 requested / 223 resolved, extension
  540 012 544 B, sha `99749240…`. **VERSION_ID does not move across an APEX
  image build**, so none of the three changes.
  - *Consequence to test and record*: pkg-share's fix does not reach an
    existing machine through `sudo apex update` at all. The knob that exists
    for this is `PKG_COMPAT_LEVEL` (2 today); an engine change that alters
    WHICH FILES the extension carries is exactly the case it is documented for.
    Likely a follow-up defect for this unit to file.
  - *Method*: prove the no-op first (`sudo /usr/libexec/apex-pkg rebuild
    --if-needed` and `sudo apex install steam` on the new image, both expected
    to do nothing), then force it with `sudo systemd-sysext unmerge` followed
    by `sudo apex install steam`, which makes `merged` false and falls through
    to `extract_rpms`. Keep the requested set as it is (chromium, gamemode,
    gamescope, libgcc.i686, libSM.i686, mangohud, steam, steam-devices) so the
    177 shadow count is like-for-like with round 31.
- **Negative control on the OLD image, so "the image changed" is a measurement
  and not a hope** (17:20 AWST): `apex gaming --gamescope-device-args` → rc=2,
  `error: unexpected argument`; `grep -c 'prefer-vk-device\|prefer-output'
  /usr/libexec/apex-gaming-session` → 0; `grep -c 'CapEff\|CapPrm\|CapAmb'` →
  0; `getcap $(command -v gamescope)` → empty, rc 0; no `vrr_capable` anywhere.
  All five must flip (except getcap and vrr, which should not) on the new one.
- The new session script's exact strings, read out of the branch:
  `[apex-gaming-session] capabilities: CapEff=… CapPrm=… CapAmb=…`,
  `CAP_SYS_NICE: absent; soft RLIMIT_RTPRIO is …`, `GPU/output: …`,
  `starting: gamescope … -- steam …`. `log()` writes to **stderr**, so under
  greetd the whole session log lands in `journalctl -u greetd -b`.
- Session `Exec` lines are single commands, so greetd's `initial_session
  command =` can take them verbatim: `apex-gaming` →
  `/usr/libexec/apex-gaming-session`, `niri` → `niri --session`,
  `apex-safe-graphics` → `/usr/libexec/apex-safe-graphics` (prefix with `env
  VAR=…` for 6.4's forced device).
- **Block 6.5's precondition is intact**: `~/.config/niri/config.kdl` still has
  the untouched upstream `spawn-at-startup "waybar"` at line 271, there is no
  `config.kdl.pre-apex-bar.bak`, and `/usr/bin/waybar` exists.
  `apex-shell-firstrun` writes **no marker any more** and runs at every login,
  so one niri login on the new image applies the transform.
- Round 31's `/var/home/andre/qual/shot.py` (grim + distinct-colour/mean-RGB
  per output) survives and is reused for render proof in 6.4/6.5.
- `grim` and `wlr-randr` do not work under gamescope (its DRM/Wayland backend
  is its own, not wlroots). Render proof for 6.1 is therefore the five
  gamescope log lines + `nvidia-smi` listing gamescope as `G` + DRM sysfs
  (`/sys/class/drm/card2-HDMI-A-1/{enabled,dpms}` vs `card1-eDP-1/dpms`).
  `grim` is still the right tool for 6.4 (labwc) and 6.5 (niri).
- Only two connectors are `connected`: `card1-eDP-1` (Intel) and
  `card2-HDMI-A-1` (NVIDIA).


## BLOCKED ON

- CI 35433705393 producing
  `ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`.
  Last poll 17:13 AWST: `manifest unknown`.
