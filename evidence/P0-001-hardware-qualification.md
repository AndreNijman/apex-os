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
