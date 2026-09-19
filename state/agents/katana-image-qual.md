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

**Poll for the tag.** `skopeo inspect --no-tags
docker://ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`
— last checked 17:13 AWST, still `manifest unknown`. When it appears, run step R
below.

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
- **6.4** `/usr/libexec/apex-safe-graphics check`, then
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

## DONE

- 17:12 — worktree `/var/tmp/apex-work/wt-katana-qual2` on
  `task/katana-image-qual` off `roadmap/v2.2` @ 7f647470.
- 17:12 — katana confirmed free: uptime 6:37, greetd on seat0+tty1, nobody
  logged in, no steam/gamescope/proton/wine process, `rpm-ostree status` idle.
- 17:12 — pre-rebase baseline captured (see plan step P).
- 17:15 — read §6 of `docs/gaming-and-sessions.md` end to end and §§3, 4, 6 of
  the round-31 evidence; plan above written.

## IN PROGRESS

- polling GHCR for the per-SHA tag.

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

## BLOCKED ON

- CI 35433705393 producing
  `ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`.
  Last poll 17:13 AWST: `manifest unknown`.
