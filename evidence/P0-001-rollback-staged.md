# P0-001 — "Rollback succeeds": staged 2026-09-12, waiting on one reboot

Andre asked for this to be staged for him. It is. **Nothing has rebooted, and
nothing will until he runs `systemctl reboot`.**

## Why this criterion, and why now

P0-001's acceptance list is six criteria wide and most of it needs a fresh
install, a second machine, or hardware that is off-limits. "Rollback succeeds"
is the exception: it needs one reboot on this laptop.

It is worth doing *here* specifically. katana's second deployment slot holds the
same digest as its first, so a rollback there demonstrates nothing — the machine
would boot the image it was already on and report success. The L16 has two
deployments with genuinely different digests, so a rollback here is a real test
of the mechanism.

## What was staged

```
sudo rpm-ostree rollback          # ran 2026-09-12, swap only, no reboot
```

| | digest | built |
|---|---|---|
| was booted, still running | `sha256:308127d9…` | 2026-09-05T13:36:24Z |
| **now default for next boot** | `sha256:5e206de5…` | 2026-09-05T03:29:10Z |

Both deployments run kernel **7.2.3-cachyos2.fc43.x86_64**, verified from
`/boot/loader/entries/`. This is **not** the image that hard-crashed on
2026-09-05 with watchdog and clocksource timeouts — that was `7.1.5-cachyos1`,
the August daily, and it is no longer in the boot chain. The rollback target is
four hours older than what is running, not four weeks.

## What Andre does

1. Reboot when convenient: `systemctl reboot`
2. First thing after logging back in:
   `ROADMAP/evidence/P0-001-rollback-verify.sh`
3. Roll forward whenever you like — the script prints the two commands, and
   staying on the older image for a while costs nothing.

## What the reboot costs

Any agent running at that moment dies with the session. That is survivable by
design and has happened seven times in this program without losing work: every
agent pushes as it commits, `apex-wip-snapshot.timer` force-pushes every dirty
worktree to `refs/wip/` every three minutes, and this whole `ROADMAP/` directory
goes to `refs/wip/roadmap-state`. The in-flight CI image build is unaffected —
it runs on GitHub, not here.

There is one honest caveat about the verification itself: a crash before a clean
shutdown discards a staged ostree change, so if the machine goes down badly
instead of rebooting cleanly, it will come back on the original image and the
script will say so rather than pretending.

## What this does and does not close

It closes **one** of P0-001's six criteria. The rest — fresh install, the
Daily/Gaming build matrix on real hardware, the three GPU vendors, suspend and
resume, Wi-Fi, Bluetooth, audio, multi-monitor, portals, Steam and recovery —
need an install and a machine that is currently off-limits. The build criterion
is separately blocked in CI and is being fixed there; see the merge commit for
`task/p0-finish`.
