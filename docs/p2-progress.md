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
| 9.1 `apex host` — trust and transport | done — `0650db6`, `e3e742e`, `355c946` |
| 9.2 §14 local AI service | in progress |
| 9.3 §20 dispatch, handoff, remote status | OS side done — `7636431`, `781378a`; shell side in progress |
| 10 Boot v2 | in progress |

## 9.1, and what it decided for everything above it

`apexd-core/src/host.rs` (37 tests) owns the registry, the validation and the
argv construction; `apex/src/host.rs` (31 tests) does the I/O;
`tests/test-apex-host.sh` (53 assertions) drives the shipped binary with a fake
`ssh` that records every argv.

**The transport is the user's own ssh configuration.** A host entry names an ssh
destination, normally an alias already in `~/.ssh/config`. Not for brevity: a
real entry is often not "a hostname" — the `katana` alias here resolves over the
LAN when the LAN is up, otherwise a VPS port, otherwise a jump host into a
reverse tunnel. An `address` field would work at home and fail everywhere else,
which is exactly when remote compute is worth having. It also means APEX
generates no key and holds no passphrase, so it cannot produce a credential
prompt.

Three keys exist only to be refused, so the refusal can say where the setting
really lives: `identity_file`, `strict_host_key_checking`, `ssh_options`.

Verified against the katana over real ssh, not a mock. Its installed apex 0.1.0
does not know `host describe`, so the live run took the fallback path and read
20 cpu / 62 GiB / cuda+vulkan / podman off it. The self-describe path was then
confirmed by running the new binary there, and **its actual output is the
fixture the parser test uses** — a hand-written fixture would only prove the
parser accepts what I imagine the other end sends.

## Where a project is on the far side — the 9.3 decision

§20's `apex build --on desktop` and `apex agent run --host desktop` need the
project, and the files are on the laptop while the compute is on the desktop.
Three options were on the table:

| option | why not |
| --- | --- |
| Same absolute path, assumed | Silently building the wrong tree is far worse than refusing. |
| Locate by git identity, clone on demand | Turns a dispatch into a repository write on the remote; uncommitted work still unhandled. |
| A configured path map per host | Real config complexity for a case that is usually trivial. |

**Chosen: same absolute path, *verified*, never assumed.** The remote path is
checked to exist and to be the same repository — `git remote get-url origin`
compared on both ends — and a mismatch is a refusal naming both values, with
`--remote-path` as the explicit override. For one developer with the same
username on two APEX boxes this needs no configuration, and when the assumption
is wrong it fails loudly instead of quietly.

**Uncommitted changes are not transferred, and the command says so.** A build on
the remote runs the remote's committed state; a dirty local worktree is reported
and needs `--allow-dirty` to proceed. Syncing a working tree would mean this
tool writing over files on another machine, which is not something a dispatch
verb should do by default.

## 9.3 — what §20 asked for, and where each piece landed

| §20 asks for | verb | state |
| --- | --- | --- |
| Run builds on a more powerful desktop | `apex build --on <host>` | done |
| Run agents there | `apex agent run --host <host>` | done |
| Continue a terminal or agent session elsewhere | `apex agent attach --host`, `apex agent list --host` | done |
| Run local-model inference there | `apex ai run --on <host>` | forwarder built; wired when §14 lands |
| Send clipboard and files between devices | `apex send <host> [paths…] \| --clipboard` | done |
| Open a browser tab or project on another device | `apex open <host> <target>` | done |
| Show remote agent status in APEX Shell | apex-shell branch | in progress |

`apex host run <host> -- <argv>` remains the general escape hatch §24 asks APEX
to keep, and the specific verbs are thin over it.

### apex open took three attempts, and each fault was invisible in a green test

The first version printed `opened on katana` while **nothing opened**.

1. `setsid --fork` returns 0 the instant it forks, so the exit status proved
   only that a fork happened. `WAYLAND_DISPLAY` was never set — only
   `DBUS_SESSION_BUS_ADDRESS` — so the browser had no display to reach, and the
   output went to `/dev/null` where the evidence died.
2. `systemd-run --user --wait` does propagate the real status, but a browser
   becomes the unit's main process, so it blocked until the browser exited —
   measured at two minutes and still going.
3. The backgrounded child's **stdout** has to be redirected, not just its
   stderr. ssh holds the session open while any descendant holds the channel,
   so a successfully launched browser kept the command hanging for a full
   minute *after* it had already worked.

What it does now: launch in the background, then observe. `RUNNING` after 1.5s
is success for a GUI, `EXIT 0` is success for a hand-off, and anything else —
including an answer it does not recognise — is a failure. The session probe
requires a **compositor socket** and not merely the per-user bus, because the
bus exists for any login including the ssh connection asking the question; a
machine at its greeter would otherwise report success.

The socket is matched as `wayland-[0-9]`, not by a `wayland-*` glob with
`head -1`: the katana's runtime directory holds `wayland-1`,
`wayland-1-awww-daemon.sock` and `wayland-1.lock`, and picking the right one by
sort order is not a reason.

**How it was finally verified:** by killing the firefox that attempt 2 had
started, so an already-running browser could not let `xdg-open` hand off and
exit 0. That confound is what made attempt 2 look like it worked.

### Tests

| suite | count |
| --- | --- |
| `apexd-core::host` | 37 |
| `apexd-core::dispatch` | 34 |
| `apex::host` | 31 |
| `apex::dispatch` | 18 |
| `tests/test-apex-host.sh` | 53 |
| `tests/test-apex-dispatch.sh` | wired; run pending the §14 branch compiling |

Both shell suites are shellcheck-clean at `-S warning` and wired into the
`rust` job of `pr-validation.yml` — that job rather than `static`, because they
need a toolchain to build the binary they drive. `static` is where a cross-file
parity check belongs, per the note P1 left after a check in a specialised job
was skipped by a PR touching only the other side.
