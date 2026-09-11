# P0-001 — hardware qualification evidence

Probed 2026-09-06 on both machines with `scratchpad/qualify.sh` (read-only).

## Both machines run the same image

| | L16 | katana |
|---|---|---|
| tag | `apex-os:daily` | `apex-os:gaming-nvidia` |
| booted digest | `sha256:308127d9` | `sha256:308127d9` |
| ostree checksum | `f3f505fc39fb268c` | `f3f505fc39fb268c` |
| kernel | 7.2.3-cachyos2.fc43 | 7.2.3-cachyos2.fc43 |
| rollback available | `sha256:5e206de5` | `sha256:8d89b52c` |
| failed units (system + user) | none | none |

Two different tags resolving to one digest and one ostree checksum is direct
evidence that the one-image collapse and the tag promotion work on real
hardware.

## GPU baseline

- **AMD** (L16): HawkPoint1 Radeon 780M, `amdgpu` loaded, `radeon_icd` present.
- **Intel** (katana): Alder Lake-P Iris Xe, `i915`/`xe` loaded.
- **NVIDIA** (katana): RTX 3070 Laptop, driver **580.178.04**, `nvidia-smi`
  answers. The Optimus pair is live: Intel and NVIDIA both bound on one box.
- The nvidia kernel module carries a signature on both machines.

## Criterion by criterion

| Criterion | Verdict | Evidence |
|---|---|---|
| Daily and Gaming images build cleanly | PASS | One image built and published as `308127d9`; both tags resolve to it and both machines boot it. |
| Fresh install succeeds | UNVERIFIED | Needs a wipe on real hardware. Not attemptable without Andre. |
| Upgrade succeeds | PASS | Both machines reached `308127d9` through `bootc upgrade` and a clean reboot. |
| Rollback succeeds | UNVERIFIED | A rollback deployment exists on both, and `apex doctor` is present, but the reboot has not been exercised. Testable on katana; deferred while agents are using it as a build box. |
| Hyprland/niri/Floating boot and basic workflows | PARTIAL | All four compositors installed on both machines, four wayland sessions registered (`apex-gaming`, `apex-labwc`, `hyprland`, `niri`). L16 is running a live session. Per-compositor boot and workflow passes are not yet recorded. |
| NVIDIA/AMD/Intel baseline tested | PASS at driver level | See GPU baseline. Per-GPU workload testing is separate. |
| Suspend/resume | PARTIAL | L16: 40 resumes in 30 days, `s2idle` supported. **katana: 0 resumes ever recorded**, so untested there despite supporting `s2idle` and `deep`. |
| Wi-Fi | PASS | Both connected (`wlp3s0`, `wlo1`), NetworkManager active. |
| Bluetooth | PASS | `bluetooth.service` active and adapter powered on both. |
| Audio | PASS | pipewire + wireplumber active on both; katana has a USB DAC bound. |
| Multi-monitor | UNVERIFIED | Needs physical displays. Ties to P0-018. |
| Portals | PASS | `gnome`, `gtk`, `hyprland`, `wlr`, `gnome-keyring` portals installed; 3 portal services running on both. |
| Steam | N/A by design | Steam, gamescope, MangoHud and Sunshine are absent on both, which is correct: the one-image design installs them on demand through `apex install`. |
| Recovery | PARTIAL | `apex doctor` present, rollback deployment present. Recovery flow not exercised. |

## Findings worth Andre's attention

**Secure Boot is disabled on katana.** The L16 reports `SecureBoot enabled`;
katana reports `SecureBoot disabled`. The project's stated end-state is Secure
Boot on for both, with katana enforcing from install day because its Windows
anti-cheat fallback requires it. Katana has drifted from that, or was never
enrolled. The signed nvidia module is present either way, so enabling it is a
firmware and MOK question rather than an image question.

**Suspend has never run on katana.** Zero suspend entries in 30 days against 40
on the L16. Any s2idle defect on the Intel/NVIDIA box would still be
undiscovered.

## Status

`partial`. The image is qualified on two real machines across all three GPU
vendors, with clean unit state and working network, audio, Bluetooth and
portals. Fresh install, rollback, multi-monitor and per-compositor workflow
passes remain.

---

# Round 13 — 2026-09-12: the build criterion, and what a reboot would cost

Re-probed on the L16 only (read-only `bootc`/`ostree`; katana off-limits by
instruction). Everything above still holds. Two rows change, and the first
one changes materially.

## "Daily and Gaming images build cleanly" — the PASS above is about a
## different tree, and must be split

The row above says PASS, and for digest `308127d9` — built from `main`, which
both machines boot — it is true. It is **not** true of the branch the roadmap
is actually being assembled on.

`build-image.yml` had only ever run on `main`. Run **34656544347** is the
first image build ever attempted from `roadmap/v2.2`, and it **failed**. Not
in any image stage: in the **`rust`** job, at `cargo test`, exit 101 — six of
`apex-agentd/tests/budget.rs`'s eight assertions:

    /proc/13146/cgroup places this connection in
    "/system.slice/hosted-compute-agent.service", which is neither a login
    session nor a user service, so there is no way to tell whether a human
    is at this machine

That is `origin::classify` behaving correctly. `apex-agentd` decides whether a
human is present from the connecting peer's cgroup, and a GitHub-hosted
runner sits in a system-slice service. `pr-validation.yml` learned this on
2026-09-07 and runs its tests through `tests/in-login-session.sh`;
`build-image.yml`'s rust job never got the same treatment.

**This blocks the image, not just six tests.** `base` declares
`needs: [changes, rust, core]`, and `image` needs `base`. A failing `rust`
job SKIPS them both. One missing wrapper is therefore the whole reason no
image can be built from the branch.

Fixed on `task/p0-finish` as `392108e5`, proved in both directions:

| Placement | Result |
|---|---|
| `session-4.scope` (this laptop, a real login) | 8 passed, 0 failed |
| `/system.slice/…service`, unwrapped — the runner's shape, reproduced locally with a transient system-slice service running as the invoking uid | **2 passed, 6 failed**, with CI's refusal text word for word |
| `/system.slice/…service`, wrapped in `in-login-session.sh` | **8 passed, 0 failed** — the helper minted `session-7.scope` |

And in the target environment rather than by local proxy: pr-validation run
**34640554622**, hosted `ubuntu-24.04`, shows its ✓ **Tests** step — the one
wrapped in `in-login-session.sh` — passing. logind `CreateSession` does work
on a hosted runner.

**A second, independent blocker sits behind it.** `Containerfile.base:338` on
`roadmap/v2.2` @ `61504ca2` asserts
`test -L /usr/lib/systemd/system/multi-user.target.wants/apex-secretd.service`,
which the predecessor measured inside `ghcr.io/andrenijman/apex-os:core` as
never true — `systemctl enable` writes the symlink under `/etc`, and the four
sibling checks in the same file all name `/etc`. That fix is `f372c089`, and
`git merge-base --is-ancestor f372c089 origin/roadmap/v2.2` answers **NO**.
Run 34656544347 did not reach it, because `base` was skipped, so the
prediction stands untested and applies to the first run that gets there.

**Honest statement of the criterion:** `roadmap/v2.2` needs **both**
`392108e5` and `f372c089`; neither suffices alone. Whether further never-true
assertions sit past line 338 is **unknown** — the local `core -> base -> apex`
build that would have found them died with the previous session and was not
restarted, because a CI image build is stronger evidence than a laptop one.
**Closing this criterion costs one CI run and no hardware.**

## "Rollback succeeds" — still unverified, but the L16 is the machine to do it on

The narrative above defers rollback to katana and then notes that katana's
rollback slot holds the *same* digest as its booted one (a second deployment
of one commit, created by the hotfix unlock), so a reboot there would prove
very little.

**The L16's slot is not degenerate.** Read today:

| | booted | rollback |
|---|---|---|
| digest | `sha256:308127d9…` | `sha256:5e206de5…` |
| ostree checksum | `f3f505fc39fb268c…` | `c9230df867ad8b57…` |
| timestamp | 2026-09-05T13:36:24Z | 2026-09-05T03:29:10Z |

Two different images, two different commits. A rollback reboot on this
machine would therefore be a real test of the rollback path.

**What it would take: one consented reboot on the L16, and nothing else.** No
build, no staging, no hardware anyone lacks. This unit must not perform it —
rebooting Andre's daily machine is his call — but it is the cheapest
outstanding criterion in the whole item, and it is worth asking for.

The same two rows also re-confirm **"Upgrade succeeds"**: two different
digests of the same `:daily` tag, with the machine running the newer, is an
upgrade that was performed and booted.

## Everything else, re-measured today

| Fact | Value |
|---|---|
| Secure Boot (L16) | **enabled (deployed)** |
| Failed units, system bus | **zero** |
| Failed units, user bus | 10, and every one a `libpod-*` / `libpod-conmon-*` / `podman-*` transient scope left by this session's own container runs, `tests/run-clippy.sh` among them. **No image unit is failing.** Deliberately not `reset-failed`'d: this is a report of state, not a tidy-up. |
| Suspend/resume (L16) | **39** resumes in 30 days, all `systemd-suspend.service` finishing cleanly |
| Wayland sessions registered | `apex-gaming`, `apex-labwc`, `hyprland`, `niri` |
| Compositors on PATH | `Hyprland`, `niri`, `labwc` present; `gamescope` absent, which is correct — it is installed on demand |

## What remains, and who can do it

| Criterion | Needs |
|---|---|
| Images build cleanly | One CI run from a branch carrying `392108e5` + `f372c089`. **No hardware.** |
| Rollback succeeds | **One consented reboot on the L16.** Nothing else. |
| Fresh install succeeds | A wipe and a real install. Only Andre can authorise; this laptop is his daily machine and katana is off-limits. |
| Per-compositor boot and workflows | A human logging into each of the four sessions. |
| Multi-monitor | Physical displays. Ties to P0-018. |
| Recovery flow | Exercising it, which implies a reboot. |
| katana's Secure Boot and suspend | katana, which is off-limits. |

Nothing above was simulated. No install was attempted, no tag pushed, nothing
promoted, and every `bootc`/`ostree` call was read-only.
