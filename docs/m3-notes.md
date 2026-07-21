# APEX-OS — M3 notes (apexd v1: power engine)

M3 delivers the first-party power daemon `apexd`, its control CLI `apex`, and
the pure, unit-tested core `apexd-core`. It absorbs the hand-tuned L16
"powermode" design (see the decisions vault note) into a proper daemon with a
frozen D-Bus API, AC/battery auto-switching (which the Void box never had), a
gated RyzenAdj EC-defeat loop, and Prometheus metrics.

## Workspace

```
apexd/                 cargo workspace root
  apexd-core/          lib: fingerprint, profiles, layered selection, tier engine, SysWriter
  apexd/               bin: zbus D-Bus service + AC loop + gated ryzenadj + metrics
  apex/                bin: control CLI (D-Bus client + local read-only fallbacks)
config/sysprofiles/    the six shipped profiles (embedded via include_str! + on-disk override)
files/system/          D-Bus policy + activation, polkit policy
docs/apexd-dbus.md     the frozen D-Bus contract
```

All hardware effects go through the `SysWriter` trait. `RealWriter` writes
sysfs / execs `ryzenadj` (and honours dry-run); `MockWriter` records intended
actions and touches nothing. Every logic path is unit-testable without writing
real sysfs.

## Build / test / lint

- `cargo build --release` — clean.
- `cargo test` — **17 passed** (6 selection + 11 tier-engine, incl. dry-run
  write-gate proofs). No test writes real sysfs.
- `cargo clippy --all-targets -- -D warnings` — clean.
- Toolchain: rustc/cargo as installed on the L16 (edition 2021, `rust-version = 1.75`).

## Per-vendor tier table (governor / EPP / platform_profile)

The three-knob mapping mirrors the proven amd-pstate-epp "powermode" table.
Governor and EPP names are identical across amd-pstate-epp and intel_pstate
active mode, and `platform_profile` names are the ACPI standard, so the shape is
shared; vendor/device divergence lives in the **defaults**, **charge config**,
and **device extras** (ryzenadj).

| Tier | governor | EPP | platform_profile |
|---|---|---|---|
| ultra-max | performance | performance | performance |
| ultra | performance | performance | performance |
| performance | performance | balance_performance | performance |
| balanced | powersave | balance_power | balanced |
| power-saver | powersave | power | low-power |

Per-profile differences:

| Profile | AC default | battery default | charge | device extra |
|---|---|---|---|---|
| generic-desktop | balanced | balanced | — | — (no platform_profile) |
| generic-laptop | balanced | power-saver | — | — |
| amd-zen (class) | performance | balanced | — | — |
| intel-hybrid (class) | performance | balanced | — | — |
| thinkpad-l16-g2 (device) | performance | balanced | 75/80 (BAT0) | ryzenadj on **ultra-max** |
| msi-katana-gf76 (device) | **ultra** | performance | 60/80 (BAT1) | — (no platform_profile) |

**ultra-max vs ultra:** identical in the three-knob table; ultra-max differs
only by the device extra (the L16 RyzenAdj loop). On machines without a
ryzenadj block the two tiers are equivalent — intentional.

**thinkpad-l16-g2 ultra-max ryzenadj:** STAPM 62 W / fast 75 W / slow 62 W,
tctl 95 °C, reapplied every 1 s to defeat the EC's ~30 W clawback, with a hard
`ceiling_mw = 79000` clamp (< 80 W) enforced in code (`RyzenAdjConfig::clamped`
— unit-tested to clamp over-limit values). The loop is **gated**: it runs only
if `device == thinkpad-l16-g2` AND `ryzenadj` is on PATH AND not dry-run, and is
torn down automatically when switching away from ultra-max.

## Selection

`generic (chassis) → class (CPU) → device (DMI)`; most specific wins.

- chassis 3/4/6/7 → `generic-desktop`; 8/9/10/11/14/30-32 → `generic-laptop`.
- AMD → `amd-zen`; Intel **only if P/E hybrid** → `intel-hybrid` (uniform Intel
  stays generic).
- DMI product name/family/version contains `Katana` → `msi-katana-gf76`;
  contains `ThinkPad L16` → `thinkpad-l16-g2`.

Unit-tested with synthetic fingerprints: L16, Katana, generic AMD desktop,
unknown Intel hybrid, uniform Intel laptop.

## On-box validation (THIS machine — read-only, dry-run, daemon NOT started)

This physical box *is* the amd-zen / thinkpad-l16-g2 target. Validation used
only `apex` in `APEXD_DRY_RUN=1`; the writing daemon was never started and no
sysfs was written (verified: `charge_control_end_threshold` stayed 90,
`scaling_governor` stayed `performance`, `platform_profile` stayed `performance`
after all commands).

`APEXD_DRY_RUN=1 apex fingerprint`:

```
Machine
  vendor        : LENOVO
  product       : 21SCCTO1WW
  family        : ThinkPad L16 Gen 2
  version       : ThinkPad L16 Gen 2
  chassis       : 10 (laptop)
CPU
  vendor        : AMD
  model         : AMD Ryzen 7 PRO 250 w/ Radeon 780M Graphics
  topology      : 8 cores / 16 threads
  scaling driver: amd-pstate-epp
GPU
  AMD [1002:1900] @ 0000:c5:00.0
Power supply
  AC present    : true
  batteries     : BAT0
Profile (layered selection)
  generic       : generic-laptop
  class         : amd-zen
  device        : thinkpad-l16-g2
  active        : thinkpad-l16-g2
```

`APEXD_DRY_RUN=1 apex status` additionally printed the full dry-run tier plan
for `thinkpad-l16-g2` — ultra-max including
`ryzenadj stapm=62000mW fast=75000mW slow=62000mW tctl=95C`, down to power-saver
(`powersave` / `power` / `low-power`), plus charge defaults 75/80 and
`auto-switch defaults: AC -> performance, battery -> balanced` — with the banner
`apexd: not running — showing local dry-run view.`

`apex doctor` (this box): PASS on profile resolution, EPP-capable driver
(`amd-pstate-epp`), ACPI `platform_profile` present, charge-threshold control
present, **`ryzenadj` on PATH**, and `s2idle` active; WARN only on "apexd
running" and "metrics endpoint reachable" (both expected — the daemon was
deliberately not started).

**Fingerprint + selection confirmed: AMD Zen + Radeon 780M + laptop →
`active = thinkpad-l16-g2` (class `amd-zen`).**

## What can NOT be tested without the real target machines / a running daemon

Honest list — all of this is code-complete and compiles, but was not exercised
at runtime here (the writing daemon must never run on this box, and dry-run
blocks all writes):

1. **Live sysfs application** of governor/EPP/platform_profile across all
   cpufreq policies — only proven against a *fixture* sysfs tree (the dry-run
   write-gate test), never against real `/sys` on this box by policy.
2. **The RyzenAdj EC-defeat loop actually holding ~62 W STAPM against the EC
   clawback** on the L16 — needs the daemon running as root with real ryzenadj
   writes (explicitly forbidden here). Only the plan/gating/clamp logic is
   tested.
3. **The full D-Bus service at runtime**: name acquisition on the system bus,
   the six interfaces answering, `TierChanged`/property-changed signals firing,
   and polkit `CheckAuthorization` returning `allow_active = yes` for the
   logged-in user. The zbus code compiles and the client proxies match, but no
   live bus round-trip was performed (no daemon started).
4. **AC/battery auto-switch transitions** — the poll loop + tier reconciliation
   need a live daemon and a real plug/unplug event.
5. **Prometheus endpoint on :9723** and the `.Metrics.Snapshot` a{sv} — the
   render/gather code is unit-reviewable but the socket was never bound here.
6. **`apex pin|rollback|update|changelog`** — these shell out to
   `ostree`/`bootc`/`fwupdmgr`/`skopeo`, which only do anything on an actual
   bootc deployment (not on this Void host). The command wiring and
   missing-binary graceful-degrade paths are in place; the real effects need an
   installed APEX-OS image (M4).
7. **The MSI Katana (`msi-katana-gf76`) and Intel-hybrid paths** — selection is
   unit-tested with synthetic fingerprints, but real Intel P/E hybrid detection
   (`/sys/devices/cpu_core` + `cpu_atom`), intel_pstate EPP application, and the
   Katana's charge/`BAT1` paths can only be confirmed on that machine.
8. **NVIDIA/Optimus, fan control, game mode** — out of M3 scope; `.Fan` and
   `.GameMode` are declared no-op stubs, real impl at M6.

## Integration handoff

- apex-shell's `PowerProfileService` currently drives the legacy
  `/usr/local/bin/powermode` CLI and watches `/run/powermode/mode`. Migrating it
  to `apexd` is an M-shell task: swap `setProfile(id)` to call `.Power.SetTier`
  and bind `current`/`Tier` + the `TierChanged` signal. The tier IDs already
  match exactly, so it is a backend swap, not a redesign.
- The daemon reads profiles from `/usr/share/apexos/sysprofiles/` if present,
  else the embedded copies of `config/sysprofiles/*.toml`.
