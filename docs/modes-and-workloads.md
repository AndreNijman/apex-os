# Modes, the workload manager and the Performance Lab

Roadmap §11 (modes), §12 (gaming mode and Performance Lab) and §13 (the
workload-aware performance manager), as shipped.

## `apex mode` — named modes, composed from what already exists

§11 asks for coherent operating modes and says how: *"avoid duplicating the
whole OS for every mode"*, *"use `apexd` as a narrow policy/control plane"*. So a
mode adds no hardware lever. It is a named combination of three things the CLI
already did:

| Lever | D-Bus call |
|---|---|
| power tier | `org.apexos.Apexd1.Power.SetTier` |
| AC/battery auto-switch | `org.apexos.Apexd1.Power.SetAutoSwitch` |
| game mode (cpuset, IRQ steering, GPU clock locks, sched-ext) | `org.apexos.Apexd1.GameMode.SetActive` |

No D-Bus member was added. The frozen `org.apexos.Apexd1` surface is unchanged.

```
apex mode list             # the eight modes and the policy each applies
apex mode show gaming      # what it changes, what it only reports, and why
apex mode status           # which mode the machine is in
apex mode set gaming       # apply
apex mode set gaming --dry-run
apex mode set --auto       # apply what `apex workload` measured
```

### The catalogue

| Mode | Tier | Game mode | Intent (§13) |
|---|---|---|---|
| `daily` | auto (profile defaults) | off | — |
| `gaming` | performance (pinned) | **on** | latency |
| `development` | performance (pinned) | off | throughput |
| `creator` | performance (pinned) | off | sustained |
| `ai` | **balanced** (pinned) | off | preserve VRAM |
| `battery` | power-saver (pinned) | off | efficiency |
| `couch` | balanced (pinned) | off | low-power |
| `server` | performance (pinned) | off | throughput |

`ai` pins *balanced* rather than *performance* deliberately: local inference is
GPU- and memory-bandwidth-bound, so pinning every core to the performance
governor spends package power the GPU wants without moving tokens/second. APEX
cannot reserve VRAM — no kernel interface does — so the mode reports VRAM
headroom rather than claiming to manage it.

`gaming` is the only mode that turns game mode on. Game mode confines work to
the P-cores, which is right for a game and actively wrong for a parallel build.

### There is no mode state file

The active mode is **derived** from what apexd reports — tier, auto-switch, game
mode — not stored. Two consequences, both wanted:

* `apex mode set` needs no root. Persisting the mode would have meant a
  root-owned file under `/var/lib`, and `apex`'s root gating already documents
  why a blanket root requirement is wrong for the verbs the desktop's power
  controls drive as the session user.
* The answer cannot go stale. Change the tier by hand and `apex mode status`
  says so immediately, naming the closest mode and the exact difference.

`apex mode status` can report **several** modes at once:

```
mode          : development, creator, server
```

That is the honest answer, not a bug. Those three pin the same tier with the
same game-mode setting; they differ only in declared intent and in the service
sets they report, neither of which is readable off a running machine. Collapsing
them to one would be inventing certainty.

### Ordering is load-bearing

`apex mode set` applies its steps in a fixed order, and two of the rules exist
because getting them wrong makes a mode silently not stick:

1. **Leave game mode first.** `apex game stop` restores the tier that was active
   before the session. Setting the new mode's tier first lets that restore
   overwrite it — the user asks for Battery Saver and lands wherever they were
   an hour ago.
2. **Turn auto-switch off before pinning a tier.** With it on, apexd re-derives
   the tier from the profile's AC/battery defaults, and enabling it reconciles
   immediately.

Both are pinned by mutation-verified tests.

### Service sets and system extensions are reported, not applied

§11 lists them among the things a mode "may change". They are modelled so
`apex mode show` can state the full intent, and `apex mode set` does **not** move
them. Merging a system extension on a mode switch is a heavyweight lever with
its own rebuild service, and `Containerfile.gaming` already masks `irqbalance`
permanently, so a mode toggling it would fight the image. A declared gap beats
execution that silently does not happen — and the shell suite fails if `apex
mode` ever spawns `systemctl`.

## `apex workload` — measured signals, and an explained verdict

§13 is prescriptive about the manner of this feature, not just its shape:

> Make automatic choices visible and overrideable.
> Do not market random tuning as AI optimization.
> Use measured workload signals and hardware capabilities.

Three rules follow from that.

**Every signal carries its provenance.** A reading is either measured — with the
path it came from — or unavailable, with the reason. There is no third state
where a missing reading becomes a default. A kernel without PSI says so and
names `CONFIG_PSI`; a machine whose `power_supply` class has no `Mains` object is
a *gap*, never "on battery", because defaulting that either way is wrong on half
the fleet.

**A process name never decides anything on its own.** An editor holding a stale
`rustc` is not a build. The render and compile rules require corroboration from
an independently measured busy signal (PSI `some avg10`, or load per CPU); when
neither can be read, the verdict is `unknown` and the gap is named.

**The classification is a documented ladder**, most authoritative first:

1. apexd's own game cgroup being populated — first-party fact, it put those PIDs
   there. `steam` is deliberately *not* in the process table: it runs whenever
   the client is open, so it would report a permanent game session.
2. a game runtime process (gamescope, wine), reported as the weaker signal it is
3. a local inference server, corroborated by VRAM where the driver reports it
4. rendering, 5. compiling — both requiring a busy signal
6. browsing — browsers present and measurably *not* busy
7. idle

The battery row of §13 is a **constraint** layered on top rather than a workload:
on battery, efficiency takes precedence — except over a live game session, which
is left alone, because silently unwinding something the user started explicitly
is exactly the invisible automatic choice §13 prohibits.

### Nothing is applied automatically

APEX ships no timer, no daemon loop and no background auto-apply. `apex workload`
reports; `apex mode set --auto` applies once, when you run it. A
shipped-but-disabled systemd unit was considered and rejected — the root
`AGENTS.md` treats aspirational language presented as implemented as a defect.

## `apex perf` — the Performance Lab

§12 asks for "frame time, CPU/GPU clocks, power, temperatures, VRAM and
scheduler state". `apex perf` reports all of it, `--json` included, read-only and
without root.

### Frame time is unavailable, and nothing stands in for it

There is no generic source. Frame pacing is a property of a client's swapchain:
visible to the application, to an interposed layer such as MangoHud, or to a
compositor that exports it — not to a bystander reading sysfs. Wayland's
presentation feedback goes to the *client*.

So the row says that, names MangoHud as the way to get a real measurement, and
refuses to substitute GPU busy percentage, a clock, or a frame rate derived from
anything else. A GPU can sit at 99% while a game stutters and at 40% while it
runs perfectly. A Performance Lab showing a confident number it did not measure
is worse than one with an honest gap.

### There is no single "package power" figure

There used to be, and it was wrong. The reader took the first hwmon publishing
`power1_*`; on the development ThinkPad that is `hwmon4`, owned by `BAT0`, so
`apex perf` printed **"package: 20.47 W" above a battery row showing the
identical figure from the identical sensor**. Both numbers were real and the
label was a fabrication.

Every hwmon power sensor is now reported with its chip and label
(`amdgpu/PPT: 10.00 W`), and hwmon devices hanging off a `power_supply` are
skipped because the battery row covers them. Whether a given chip's figure is
"the package" is a question the reader can answer from the label — which is more
than the code could honestly decide on their behalf.

### Per-machine gaps are named, not hidden

VRAM comes from amdgpu's `mem_info_vram_*` in sysfs; i915/xe publish no total,
and the NVIDIA driver publishes none at all, so that leg goes through an injected
`nvidia-smi` querier. GPU clocks come from `pp_dpm_sclk` (the `*`-marked active
level, not the ceiling), `gt_cur_freq_mhz`, or the querier. Where none applies,
the row says which three interfaces were tried.

## Testing, and why this area is handled carefully

This is the area that has already caused harm. An earlier game-mode suite applied
its plans through a live writer, which shelled out to `scxctl` — a D-Bus client
for `scx_loader` whose polkit action is **not** passwordless. Running the tests
raised a burst of authentication prompts on the developer's own desktop and then
blocked for 177 seconds waiting on a password, which read as a slow suite rather
than as a test reaching the host. Once authenticated it would have switched the
scheduler of the machine running the tests.

Everything here is built so that cannot recur:

* `apexd-core::mode`, `::workload` and `::perf` construct **no `SysWriter` of any
  kind**. `mode` performs no I/O at all; the other two are read-only and take
  explicit `/sys` and `/proc` roots so every case runs against a fixture.
* The NVIDIA legs go through the `NvidiaSmi` trait, so tests inject a mock and
  no process is spawned.
* `APEX_MODE_NO_APPLY=1` makes `apex mode set` refuse *before* it connects to
  anything. The suite proves that ordering by redirecting
  `DBUS_SYSTEM_BUS_ADDRESS` at nothing and checking which message comes out.
* `tests/test-apex-modes.sh` puts fake `scxctl`, `nvidia-smi`, `systemctl`,
  `busctl` and `pkexec` first on PATH, fails if any is called, and then **calls
  `scxctl` on purpose** to prove the tripwire was armed — because "no spawn
  detected" is worth nothing if the fakes were never really there.

`RealWriter::new()` still does not run host commands and only the daemon's own
constructor does; `pr-validation.yml` enforces that statically, and nothing in
this phase weakens it.
