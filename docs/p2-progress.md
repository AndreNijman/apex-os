# P2 — progress and resume point

P2 is rows 9 and 10 of the roadmap's §23 implementation order:

| row | build | roadmap section |
| --- | --- | --- |
| 9 | Local AI service + remote compute | §14, §20 |
| 10 | Boot v2: composefs + systemd-boot + UKIs + measured boot | §22 |

Branched from `p1/integration-2` at `c4d026d`, **not** from `main`. `main` is at
`4e3c490` and carries no P1 content at all; §14's model storage and VRAM
handling need the capsules, resolver and modes that P1 built, so a branch cut
from `main` would develop against a tree missing its own foundations.

Same rule as P1: nothing merged to `apex-os/main` until Andre asks for the
image build. P1's build is still unspent, so the first one will carry both.

## The constraint most likely to get silently inverted

**GRUB stays the default for every published image in this generation.**

§23's one-line table says "Boot v2: composefs + systemd-boot + UKIs + measured
boot", and read alone that sounds like a bootloader swap. §22 itself says the
opposite, starting with its own title — *"do not switch to Limine as the main
path"* — and its recommendation is to **keep GRUB for the current APEX
generation**, targeting systemd-boot + signed UKIs "once APEX is ready to move
to the bootc/composefs path". Its anti-goal is explicit: *do not switch
bootloaders for aesthetics; change the boot architecture only when it improves
reliability, verification and rollback.*

Its migration sequence is seven numbered steps, and the numbers matter:

| step | what | P2 deliverable |
| --- | --- | --- |
| 1 | Stabilize existing GRUB + bootc/OSTree install and recovery | already shipped; preserved |
| 2 | **Prototype composefs/systemd-boot in CI and VMs** | built and VM-proven |
| 3 | Signed UKIs and APEX-owned EFI paths | built |
| 4 | Boot counting and health-based automatic rollback | built |
| 5 | Measured boot + TPM-bound unlock **as an opt-in developer feature** | built, opt-in, not defaulted |
| 6 | Encryption default *once recovery and hardware edge cases are proven* | **not** done — the proof does not exist yet |
| 7 | Legacy BIOS stays on GRUB | preserved |

So a branch that makes systemd-boot the default for `daily`, `gaming-mesa` or
`gaming-nvidia` has violated the section it claims to implement, and the Secure
Boot product invariant in `AGENTS.md` at the same time. The boot-path rules
added to `AGENTS.md` in `baf25f4` carry this as rule 5.

## Sequencing, and why it is not 9a → 9b → 10

§20's remote compute is three verbs — `apex build --on desktop`,
`apex ai run --on desktop`, `apex agent run --host desktop claude` — and two of
them are dispatch wrappers over things §14 and P0's agent runtime already own.
Building `apex ai run` locally first and then adding `--on` means rebuilding its
argument handling once the transport exists.

1. **Transport and trust first** (`apex host`): trusted-device registry, SSH
   primitives, capability probe. Nothing above it can be built honestly without
   knowing what a remote host is.
2. **§14 local AI** on top of that, so `--on` is a target from the start.
3. **Dispatch** — the three `--on`/`--host` verbs as thin wrappers, plus
   clipboard/file handoff and remote agent status in the shell.
4. **Boot v2**, independent of all three. Its VM tooling is a prerequisite that
   does not exist on the katana yet, so that install starts early and in the
   background rather than being discovered at hour six.

## The katana

20 cores, 62 GB RAM, 108 GB free on `/var`, podman 5.8.4, `/dev/kvm` present,
RTX 3070 Mobile (`nvidia-smi` available) plus an Alder Lake iGPU. It is the
build box for images and the host for boot VMs — faster than waiting on GitHub,
and the one image build reserved for the end is not spent on iteration.

It is also a real APEX machine, which is why `AGENTS.md` now has a boot-path
section. Guest ESPs only.

## Status

| phase | state |
| --- | --- |
| Boot-path rules in `AGENTS.md` | `baf25f4` |
| This tracker | in progress |
| 9.1 `apex host` — trust and transport | not started |
| 9.2 §14 local AI service | not started |
| 9.3 §20 dispatch, handoff, remote status | not started |
| 10 Boot v2 | not started |
