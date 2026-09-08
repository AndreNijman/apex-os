# base-partials
items: BASE-002, BASE-005, BASE-009, BASE-010, BASE-013, BASE-014, BASE-016, BASE-018
repo: apex-os + apex-shell
worktree: /var/tmp/apex-work/wt-base-os (apex-os), /var/tmp/apex-work/wt-base-shell (apex-shell)
branch: task/base-partials (apex-os), task/base-partials-shell (apex-shell)
base: apex-os e67fab9, apex-shell a90cef6 (both origin/roadmap/v2.2)

## NEXT
Await the two recon sweeps, then write the per-criterion suite map (for each
acceptance criterion: which NAMED suite asserts it today, or `no suite`) into
this card and push. Then work in this order: BASE-018 (Surface.bootloader
caveat, fixture only), BASE-016 (new tests/test-apex-disposable-live.sh wired
into pr-validation.yml), BASE-002, BASE-005, BASE-013, BASE-014, BASE-010,
BASE-009.

## DONE
- Worktrees created and both branches pushed with -u before any work.
- Read existing roadmap evidence for all eight items (see WHAT EVIDENCE SETTLES).

## WHAT EXISTING EVIDENCE ALREADY SETTLES (per item)
- BASE-002: attach/detach/persistence PROVEN live (isolated daemon, marker
  replayed after daemon kill). Direct binaries proven upstream, not shims.
  Settled. Open: two named code defects (adapter.rs:50-56 TOOLCHAIN_RO lacks
  .local/lib; registry.rs:173-178 next_id resets to 1 on restart).
- BASE-005: capsule isolation MEASURED — read-only OS layer only, shares real
  HOME, /run/host read-write as the user, apex-env refuses root. Not a security
  sandbox; that is recorded fact, not a gap. 253/253 + 44/44 green. Open: AMD
  and hw device profiles argv-pinned, never hardware-verified.
- BASE-009: perf metrics accuracy PROVEN live (vram matched nvidia-smi; frame
  time honestly unmeasurable). Open: controller-first path unproven (gamescope
  and steam were absent at measurement time); Desktop->Gaming live switch not
  performed.
- BASE-010: RTX 3070 detection, accel list, VRAM budget, per-user socket, TCP
  refusal all PROVEN live. 44/0. Open: no runtime or model installed anywhere,
  so placement and idle-unload have never run against a live model.
- BASE-013: shared model PROVEN (48/0 facade under real labwc, 36/0 adapter
  confinement with negative control); no config drift PROVEN. Criterion 3
  FAILS as a real defect, not a missing test: greeter prints raw compositor
  names (GreetContext.qml:332-335). Hyprland/niri backends load+schema checked
  only.
- BASE-014: theme wiring, cornerRadius/shadow config, matugen pipeline, real
  screenshot with accent border, emergency fallback all PROVEN (18/0). Open:
  rounded corners not visible at headless size, shadow indistinguishable on
  black, menu/switcher need a click, rebound key after --reconfigure untested.
- BASE-016: live run with --copy-in/--copy-out, complete teardown, explicit
  plan boundary all PROVEN. Not-a-security-boundary and the /run/host write
  reaching the host are recorded facts. Open: no shell-level suite for the
  shipped script's teardown fencing and image resolution.
- BASE-018: 60/0 structural, 85/85 --with-binary, green CI booting signed /
  unsigned / foreign / tampered UKIs under SB-enforcing OVMF + swtpm. Route
  fix landed. Open: Surface.bootloader still prints grub with no caveat when
  efivarfs is refused wholesale.

## IN PROGRESS
- nothing yet

## FOUND
- CLAUDE.md records that katana now HAS steam + gamescope + mangohud installed
  (apex-user.raw, 219 pkgs) as of 2026-09-06. BASE-009's "gamescope and steam
  are not installed" is stale. Does not by itself close the item.

## BLOCKED ON
- nothing
