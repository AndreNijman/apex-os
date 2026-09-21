# katana-final-qual — qualify the FINAL image on the machine it is already booted on

items: P1-043, P2-003, P2-005, P2-006, P2-007, P1-038 (rows that need a session)
repo: apex-os
worktree: /var/tmp/apex-work/wt-katana-final-qual (create it)
branch: task/katana-final-qual, cut from origin/roadmap/v2.2
lab on katana: /var/lab/scratch/katana-final-qual   (katana uses /var/lab/scratch,
  NOT /var/lab-scratch — that path is the L16's and does not exist there)

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## READ THIS FIRST — the machine is already in the state you need

**katana is booted on the final image right now.** You do not install anything,
you do not reboot it, and you do not `bootc switch`. Verified at 03:13 AWST:

- booted `ghcr.io/andrenijman/apex-os:apex-44c9a5cb6ba07b5892ee3b1e34ec77403e7b446a`
  at digest `sha256:06ba23c3a9d666780b01bb8774a56693cf66e68e49d35232a52223e756596112`
  (build run 35624291221 — the first time `base` has ever passed in this program)
- kernel `7.2.6-cachyos1.apex1.fc43.x86_64` — APEX's OWN kernel tier
- uptime ~1h, `apex game status` → `active: false`, 0 failed system units
- only `greetd` on seat0/tty1. **Nobody is sitting at the machine and no game is
  running.** Re-check both before every heavy step anyway (see CONSTRAINTS).

## WHAT IS ALREADY EVIDENCED — do NOT redo any of it

Read these on the tip before you plan anything:

- `ROADMAP/evidence/katana-schedext-fixed-20260922.md` — the controlled
  COPR-vs-apex1 comparison. `scx_rustland` attaches on apex1 (`state=enabled`,
  `enable_seq=1`, `switch_all=1`) and refuses on COPR with the pahole<1.26 BTF
  diagnosis. **The kernel half of P1-043 is DONE.** Do not re-run that comparison.
- `ROADMAP/evidence/katana-nvram-baseline-20260921.md` — NVRAM is byte-identical
  across the switch and the reboot. katana's `Boot0000 APEX-OS Primary` shares a
  PARTUUID with `Boot0002 Windows Boot Manager`. **Do not touch NVRAM.** If you
  ever see those entries differ from the baseline, STOP and record it — that is a
  tell-Andre finding, not a row to grade.
- `ROADMAP/evidence/katana-displays-20260922.md`, `katana-p1038-apps-20260922.md`
  — two connected displays at genuinely different DPI, and P1-038's applications
  ARE installed on katana. That is what makes the session rows reachable at all.
- `/var/lab-scratch/katana-measure/after.txt` on the L16 — a thin sweep taken
  02:12 today. It is a starting point, not the qualification. Several of its
  CASEs say COULD-NOT-RUN for want of a graphical session; those are your job.

## WHAT IS ACTUALLY LEFT — in this order

### 1. P1-043's last assertion: a scheduler loaded THROUGH apexd, not by hand
The evidence above loaded `scx_rustland` **by hand**. The roadmap's open
assertion is about Gaming Mode: `apexd` calls `scxctl`, and the `gaming-scx`
landing made it READ the verb off `/sys/kernel/sched_ext/state` (`start` when
none is attached, `switch` when one is) instead of hardcoding `switch`.

Until today **no machine here could load a scheduler at all**, so that code has
never once reached its success path. katana now can. Enter Gaming Mode through
the real path and assert, from the KERNEL rather than from `scxctl`'s exit code:
`state=enabled`, `enable_seq` incremented, `apex game status` reporting
`scx_state: loaded` with a `scx_detail` that matches what `/sys` says. Then exit
and assert it detached (`state=disabled`, `nr_rejected=0`).

Run-book rows are `docs/gaming-and-sessions.md` 6.8. Read `gaming-scx.md` and
`gaming-release.md` cards first — they name the non-discriminators so you do not
waste a row: katana's default AC tier is ALREADY `performance`, so the governor
reads the same before/during/after. Use `active:`, the `apex-game` cgroup, and
`/sys/kernel/sched_ext/state` instead.

The `switch` branch (a scheduler already attached, Gaming Mode replacing it) is
now reachable for the first time too — attach one by hand first, then enter
Gaming Mode. That branch has never executed anywhere.

### 2. P2-005 / P2-006 / P2-007 — the six lines only a booted machine answers
A container has no D-Bus, systemd, NetworkManager, udev or seat, which is why
these have read "installed, and whether it runs was not answered" for weeks. The
step-by-step list is at the end of
`ROADMAP/evidence/P2-005-007-device-maturity.md` — follow that, do not invent
your own. `after.txt` already shows firewall active, avahi active, NM
connectivity full, bluetooth active with 2 paired devices, and a real FUJIFILM
ApeosPrint discoverable over `_ipp`/`_ipps`/`_pdl-datastream`. Turn those into
graded assertions against the actual criteria.

### 3. P2-003 — does quickshell publish an accessibility tree on this image?
This is the one that needs a graphical session. The new image installs
`quickshell-git-0.3.1^860.gitc6a5160` which CARRIES upstream `916a0dd`
("launch: avoid creating multiple QApplications"); katana's previous image had
released `0.3.1`, which does not. So the prediction is: **more than one node**.
One node = still broken.

`after.txt` graded this COULD-NOT-RUN because there is no quickshell process —
greetd is sitting at the greeter. Getting a session is the established dance from
`katana-image-qual.md`: stop greetd, run your session, **restore greetd
byte-identical and assert it** (`cmp`, 0 timers armed, `apex game status
active=false`). Put the restore in a `trap` — and note this repo's own lesson
that `trap … EXIT INT TERM` does NOT end a script on a signal, so a
restore-on-kill written that way works by accident. Verify the restore by
reading it back, not by having run the command.

### 4. P1-038 — the session rows whose applications now exist
Only the rows the installed applications actually cover. `katana-p1038-apps-*`
says which. VRR stays permanently could-not-run on katana — **no connector there
exposes `vrr_capable`** — and physical output hotplug needs somebody at the
machine. Say so; do not proxy them and grade them green.

## WHAT DONE LOOKS LIKE

- `ROADMAP/evidence/katana-final-qual-20260922.md`, one section per item above,
  every row a verdict of pass / fail / could-not-run **with the reason**.
- `could-not-run` is a first-class result here and is worth more than a proxy.
  This program's whole evidence style is that a named gap beats a green row that
  measured something else.
- Status changes recorded with `ROADMAP/set-status.py`. **WARNING: set-status.py
  REPLACES the evidence field and has no append flag** — recording a landing has
  wiped a prior round's counts before. For any item whose existing evidence you
  would destroy, prepend instead (see `round24-prepend.py`, used for L-003).
- Branch pushed. Do NOT land it yourself; mark `## LANDABLE <sha>` on this card
  and the orchestrator lands it.

## CONSTRAINTS I AM UNDER

- **Do not interrupt gaming on katana.** Re-read `apex game status` and
  `loginctl list-sessions` before EVERY heavy step, not once at dispatch. If a
  real user session or a game appears, stop and write the card.
- Never touch katana's boot path, NVRAM, or `bootc` state. Windows must keep booting.
- Headless only — never open a window on Andre's desktop (the L16). katana's
  screen is fine; it has no one at it.
- Never run `qs -p`. No polkit or keyring prompts. Never `pkill apex-agentd`.
- Never push `main`, never open a PR, never land on `roadmap/v2.2`.
- Long jobs under `systemd-run --user`, never `nohup &` — a backgrounded podman
  gets SIGTERMed, truncating the run while still exiting 0.
- On katana use `/var/lab/scratch`. On the L16 use `/var/lab-scratch`, never
  `/tmp` (it is a 15 GB tmpfs on 29 GB of RAM and filling it kills the machine).

## NEXT

- Item 2 (P2-005/006/007): run the 6 booted-machine lines off `apex devices all`
  on katana (the criterion is what the TOOL prints, not raw nmcli), plus
  `apex devices network/print/scan/share`. Do NOT run checklist step 5
  (hotspot) — katana's only link is `wlo1` and a hotspot severs ssh; grade it
  could-not-run. Do not modify any 802.1X profile. Then item 3 (session).

## DONE

- Orientation. Worktree `/var/tmp/apex-work/wt-katana-final-qual`, branch
  `task/katana-final-qual` cut from `origin/roadmap/v2.2` @ `f3b1b3d4`.
- Read all four evidence files + `docs/gaming-and-sessions.md` 6.6/6.8 +
  `gaming-scx-20260920.md`. Machine re-checked 03:23 AWST: `apex game status
  active=false`, only greetd on seat0/tty1, kernel 7.2.6-...apex1, uptime 1h11.
  **`scx_btf : ok` on the booted image**, so run-book Row 0 = ok and Rows A and
  C are runnable for the first time in this program.

## IN PROGRESS

## FOUND

- **`root/ops` is NOT `lavd`. It reads `lavd_1.1.3_x86_64_unknown_linux_gnu`.**
  First hardware reading of this string anywhere in the program.
  `gaming-scx-20260920.md` §5 predicted the kernel publishes the bare
  struct_ops name, so `scx_ops_matches()` strips only the `scx_` prefix and
  compares verbatim — it therefore reports a perfectly good `scx_lavd` as
  "attached, but not the scheduler that was asked for", in `scx_detail`, in
  `notes`, and as an apexd journal warning. The run-book §6.8 Row A asked for
  exactly this string verbatim and said `scx_ops_matches()` wants to know.
  Fix: also accept `<name>_<buildid>`. No scx scheduler name is another's
  prefix-plus-underscore, so this cannot collide.
- **NEW DEFECT, two parts, in the `switch` branch — measured at 50 ms.**
  A `switch` on katana is ~1.42 s of teardown-and-reattach and `scxctl` returns
  162 ms in: `enabled(rusty)` -> `disabling` @+0.129s -> `disabled` @+0.183s ->
  `enabled(lavd)` @+1.547s.
  (a) `scx_settle_until(|s| Enabled{..})` in `apexd-core/src/syswriter.rs:752`
  is satisfied by the scheduler being REPLACED — it confirms a state that was
  already true before the command. The 2 s budget is never spent. On a switch
  the predicate must be enabled AND `root/ops` == the requested scheduler.
  (b) `scx.observed` (`apexd/src/game.rs:479`) is a second, LATER read and
  lands in the disabled trough, so `apex game status` reported
  `scx_state : not loaded` / "nothing is attached" about a live `scx_lavd`
  session. A false negative — the mirror of the false positive §5c removed.
  (a) cannot be fixed without the `scx_ops_matches` fix above, or the new
  predicate can never become true.
- 26 of 73 IRQ affinity writes are refused EPERM on katana even as root
  (managed IRQs). Reported honestly by apexd (`irqs_refused: 26`), not a
  defect — recorded so the next reader does not chase it.
- `gpus_locked: 0` / `gpus_lock_attempted: 0` in `apex game status` are the
  GPU **index** list `[0]`, not a count — the journal says `1/1 GPU(s) locked`
  for the same session. Reads like a contradiction; it is not.

## BLOCKED ON

- nothing
