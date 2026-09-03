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
| 9.3 §20 dispatch, handoff, remote status | OS side done — `7636431`, `781378a`, `5e42a49`, `5d72b5e`, `fdeef1e`; shell side in progress |
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
| Show remote agent status in APEX Shell | apex-shell `p2/remote-agent-status` | done |

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
| `tests/test-apex-dispatch.sh` | 55 |

Both shell suites are shellcheck-clean at `-S warning` and wired into the
`rust` job of `pr-validation.yml` — that job rather than `static`, because they
need a toolchain to build the binary they drive. `static` is where a cross-file
parity check belongs, per the note P1 left after a check in a specialised job
was skipped by a PR touching only the other side.

## Independent check of the boot units' inertness, and why the variable matters

`AGENTS.md` boot-path rule 5 says GRUB stays the default, and the OS-side boot
work enforces that with `ConditionPathExists=` on systemd-boot's
`LoaderBootCountPath-` EFI variable: on a GRUB machine the variable is absent,
the unit does not start, and a failed condition is a skip rather than a
failure.

Verified directly on both real machines rather than from the code:

| machine | bootloader | `LoaderBootCountPath-` | other `Loader*` vars |
| --- | --- | --- | --- |
| the laptop | GRUB 2.12 | absent | none |
| the katana | GRUB 2.12 | absent | `LoaderInfo`, `LoaderDevicePartUUID`, `LoaderSystemToken` |

**The katana result is the interesting one.** It carries three of systemd-boot's
variables while still booting GRUB, so a condition written against
`LoaderInfo` — the obvious "is systemd-boot involved" test — would have fired
the units on a machine that never boots through systemd-boot.
`LoaderBootCountPath` is set only when the booted entry actually carries a boot
counter, which is exactly the state the health gate is about. The choice of
variable is load-bearing rather than incidental, and this is the evidence.

## Whole-tree baseline — taken after 9.1 and 9.3 landed

| | count | failures |
| --- | --- | --- |
| Rust tests (`cargo test --locked`) | **944** | 0 |
| Shell assertions, 18 suites | **1269** | 0 |

At P1's close the same measurement was 659 Rust and 859 shell, so P2 has added
roughly 285 Rust tests and 410 shell assertions so far.

Per-suite, all green: blueprint 139, env 253, plugin 117, gaming 113, resolve
88, modes 67, **boot-v2 60**, **dispatch 55**, pkg 54, **host 53**, firstrun 51,
secret-broker 47, privilege-requests 38, labwc-keybinds 37, input 31, display
26, project-layout 22, labwc-session 18.

**`tests/test-apex-ai.sh` does not exist yet.** §14's shell suite is still owed
by the branch building it; the 944 Rust tests include its unit tests, but there
is no artifact-level suite driving `apex ai` as a process the way `host` and
`dispatch` have. Recorded here rather than left to be noticed.

## Known rough edge in the dispatch verbs

`apex ai run --on katana` currently ends in the katana's own
`error: unrecognized subcommand 'ai'`, because that machine runs apex 0.1.0
from the last published image and the verb is new. The message is truthful and
comes from the remote, and it resolves as soon as both ends carry this build.

It is not caught locally because the capability check only refuses when the
cached probe came from an **APEX peer that described itself**. The katana's
cached probe came from the portable shell fallback, which cannot report an
`apex_version`, so `is_apex()` is false and the check is skipped — "unknown is
not absent", by design. The alternative would be refusing dispatch to any host
whose `apex` is too old to self-describe, which is worse: it would refuse the
very machines that most need probing again.

## Open items found by review, not by a test

Both of these are recorded here so they cannot be quietly forgotten.

1. **`docs/boot-v2.md` is a 14-byte placeholder** (`# placeholder`) and
   `Containerfile.base` ships it to `/usr/share/doc/apex/boot-v2.md` while
   claiming, in the comment directly above the `COPY`, that enrollment is "a
   documented, user-initiated procedure in docs/boot-v2.md, not a script".
   The document is the whole safety argument for the boot work — enrollment
   being a human procedure rather than a script is *why* it is safe — so a
   placeholder there is a hole in exactly the place where a mistake is
   unrecoverable. `AGENTS.md` also forbids it directly: documentation must
   state current behaviour, not aspiration.

2. ~~`tests/test-apex-ai.sh` does not exist.~~ **Closed** in `f25c8e8`: 43
   assertions, shellcheck-clean, wired into the `rust` job. Written here rather
   than by the §14 branch because that branch was interrupted five times by
   transient API errors and a missing suite was the worse outcome.

## Independent verification of the boot scripts' safety, done here

`AGENTS.md`'s boot-path rules are the reason this was checked directly rather
than read off the commit messages:

| check | result |
| --- | --- |
| `bootctl install/update`, `bootupctl`, `grub2-install`, `grub2-mkconfig`, `efibootmgr` on any executable line of `files/scripts/boot-v2/*` | none |
| writes to `/boot`, `/boot/efi` or `/efi` on any executable line | none |
| how a guest ESP is written | `mcopy -i "$ESP"` — mtools into a **FAT image file** |

The last one is stronger than the rule required. The rule asked for a
loopback-mounted image; mtools does not mount anything at all, so there is no
mountpoint that could resolve to the host's ESP even by mistake. Entries land
under `/EFI/APEX/`, which is §22's own requirement for an APEX identity rather
than a Fedora-named path.

## The boot-v2 CI workflow, checked against the release-tier rules

`AGENTS.md` says a `core` rebuild makes the next fleet update multi-gigabyte,
and that promotion must happen only after verification and signing. A new
workflow is exactly where that gets broken by accident, so it was checked
rather than read off its own header comment:

| requirement | how it holds |
| --- | --- |
| must not rebuild `core` | no `Containerfile.core` reference outside a comment |
| must not publish or promote | no `podman push`, no `ghcr.io`, no `cosign sign` anywhere |
| least privilege | `permissions: contents: read` — no `packages: write`, no `id-token: write`, so it **cannot** push to GHCR or mint a keyless identity even by mistake |
| no unattended runs | triggers are `pull_request` and `workflow_dispatch` only; no `push:` and no `schedule:` |

The permissions line is the load-bearing part. A workflow that merely *does
not* push today can start pushing with one careless step; one that has no
`packages: write` token cannot.

## 9.3's shell half — apex-shell `p2/remote-agent-status`

Four commits off `origin/main` (`6f2e55d`), 10 files, +2718/-8. Pushed, not
merged. Counts re-run here rather than taken on trust:

| suite | result |
| --- | --- |
| `remote-agents-test.js` | 89, all pass |
| `check-remote-agents.sh` | 48 assertions, plus 12 mutants (all confirmed applied) and 13 self-test verdicts |
| `run-remote-agent-smoke.sh` | 18 |
| pre-existing suites | 864, unchanged and green — `check-compositor-backends` 28, `check-plugin-platform` 98, `check-blueprint-editor` 75, `check-idle-inhibit` 17 all match |

### Two defects it avoided that would have shipped quietly

**`apex host list --json` returns an object keyed by host name, not an array.**
The pattern immediately next door in `AgentService` is
`if (Array.isArray(fresh))`, used twice. Copying it reflexively would have left
the remote section permanently empty *with nothing logged* — the worst kind of
failure, because it looks like "no remote agents". `parseHostList` now rejects a
top-level array by name, so a future CLI change fails loudly instead.

**Reusing `SessionRow` for remote sessions would have killed local agents.** Its
controls call `AgentService.kill(session.id)`, and a remote id belongs to
another machine's runtime — so Stop on a remote row would have terminated an
unrelated *local* agent. Remote rows are read-only, and the page prints the
`apex host run -t …` line instead.

### Where it went, and why not the top bar

The fourth section of the Agent Center, below the local sessions. The argument
against a bar indicator is a constraint rather than taste: a bar item should
appear only when there is something to show, but *knowing* whether there is
costs an ssh — so any always-present indicator must poll at idle, which is the
defect. Conditional appearance is kept where it is free: the section exists iff
a device is registered, and reading the registry is a local file read.

**Zero remote queries at idle, measured** — 22 s with the page closed against a
15 s sweep interval produced 0 registry reads and 0 device queries, counted by a
shim `apex` that logs every invocation. A device whose `caps.agentd` is false,
and one that has never been probed, were queried 0 times.

### Stated as reasoned, not observed

Nothing rendered was seen — zero Qt ERROR and zero WARN with every delegate
instantiated against real `SessionInfo` records is not the same as "the rows
look right". Real ssh was never exercised either: the shim exits instantly
where a dead host takes ~8 s, so "a dead host delays those behind it" is
reasoned. Both are the agent's own words, and they are the right words.

## Verification checkpoint

Run against the committed tree (`git archive HEAD`, so no agent's in-progress
edits could contaminate it), in the `apex-rust` container on the katana:

```
cargo clippy --all-targets --locked -- -D warnings
```

**Clean** across all six crates — `apexd-core`, `apexd`, `apex`,
`apex-agent-core`, `apex-agentd`, `apex-aid`. That is what CI runs, verbatim.

| | count | failures |
| --- | --- | --- |
| Rust tests | 944 | 0 |
| `tests/test-boot-v2.sh` (no argument, `static` job) | 60 | 0 |
| `tests/test-boot-v2.sh --with-binary` (`rust` job) | 85 | 0 |
| `tests/test-apex-ai.sh` | 43 | 0 |
| `tests/test-apex-host.sh` | 53 | 0 |
| `tests/test-apex-dispatch.sh` | 55 | 0 |

The boot suite's last assertion is worth copying elsewhere: it hashes its
fixture tree before and after and asserts the digest is unchanged, which is how
you prove a read-only verb is read-only rather than asserting that it looked
read-only.

### One thing I got wrong while reviewing, and how

I read `rust:` as starting at line 348 of `pr-validation.yml` and concluded the
boot suite's unfiltered half was in the wrong job. Line 348 is a *comment*
containing the word `rust`; the job starts at 356 and the wiring was correct all
along. The grep that misled me is the same shape as the one this project has
been bitten by five times — a pattern satisfied by prose rather than by code —
and it is worth recording that it catches the person checking for it too.
Verifying the job boundaries before acting is what kept it from becoming a
wrong "fix".

## Row 10 — Boot v2, and the two firmware traps that would have made it fake

Measured on the katana, kernel `7.1.5-cachyos1.fc43.x86_64`.

| step | what was measured |
| --- | --- |
| 2, prototype in CI and VMs | a UKI built from the booted APEX deployment — kernel 16,758,856 B, initramfs 386,072,073 B, ~390 MB signed PE — boots under `OVMF_CODE_4M.secboot` with only the ephemeral APEX cert in db. sd-stub printed `Booting initrd of APEX-OS dracut-107-8.fc43`; the real initramfs ran to dracut's `pre-mount` hook and powered off cleanly |
| 3, signed UKIs and APEX EFI paths | unsigned, foreign-signed and one-byte-tampered `.cmdline` UKIs each failed to reach userspace; the foreign one proven *validly* signed by a key not in db. `microcode=embedded-in-initrd`, detected in dracut's leading cpio rather than demanded again |
| 4, boot counting and rollback | four boots walked `apex-new+3-0.efi` → `+2-1` → `+1-2` → `+0-3`; the fifth selected the unsuffixed `apex-good.efi`; a sixth stayed there. Exact filename pairs asserted at every step |
| 5, measured boot + TPM LUKS2, opt-in | PCR 11 **changes** between two UKIs signed by the same PCR key and the same keyslot still opens with no re-enrollment; a `.pcrsig` from a different key is refused; the **recovery key unlocks in the same boot that was refused**; a marker written through the mapper in boot 1 is read back in boot 2 |
| 6, encryption by default | **not done, deliberately** — §22 gates it on recovery and hardware edge cases being proven, and one software TPM in one VM is not that |
| 7, legacy BIOS on GRUB | preserved; nothing installs a bootloader |

Reproducibility: same `SOURCE_DATE_EPOCH` gives byte-identical output, a
different one gives different bytes.

### The two findings that would have made every Secure Boot assertion vacuous

**Fedora's 2 MB `OVMF_CODE.secboot.fd` has no TCG2 protocol.** Secure Boot
enforcement works, so the signing assertions pass — but sd-stub sets no
`StubPcr*` variables, PCR 11 stays 64 zero bytes, and every TPM unlock fails
with "No signature for current PCR policy", which reads exactly like a broken
policy rather than like firmware that cannot measure. Only
`OVMF_CODE_4M.secboot.qcow2` measures (367 `Tcg2` event-log lines). `lib.sh`
converts the 4 MB pair once and **refuses to fall back**.

**`OVMF_VARS.secboot.fd` ships Red Hat *and Microsoft* certificates
pre-enrolled**, and `virt-fw-vars --no-microsoft` only means "add no more". A
varstore built on it leaves the firmware trusting Microsoft's UEFI CA — so
"only APEX-signed images load" becomes **unfalsifiable**: the test passes while
proving nothing, because a foreign image would have loaded too. Only the
pristine `OVMF_VARS_4M.qcow2` is correct. This is the most consequential
vacuous-pass this project has found, because the assertion it defeats is the
Secure Boot product invariant itself.

### Upstream behaviour measured rather than assumed

**Boot counting is not a property of the entry type.** systemd-boot 258 counts
type #2 UKIs *and* type #1 entries that use the `linux` key, and does **not**
count type #1 entries that use `efi`. So a fully `/EFI/APEX`-named UKI path is
possible; type #2 remains the default only because `bootctl list --json` — what
`apex boot status` reads — reports the tally and `.osrel` title only for entries
it recognises as UKIs. All three rows are asserted.

### Bugs the boot assertions caught in their own subjects

Every one had a green-looking symptom:

* `apex-boot-health` was non-executable in the repo, so every exit-code check
  got 126 and read as "the gate refused". The assertions now demand rc **1**,
  not merely non-zero — "it failed" was true while "it failed for the reason
  under test" was not.
* `printf | python3 - <<HEREDOC` discards the pipe, so the recovery notice was
  never written.
* `if ! cmd; then rc=$?` captures the negation rather than exit 10, leaving
  stale notices.
* A dotted JSON accessor returned `null` for every entry-level check, because
  entry ids contain a literal dot.
* **The APEX initramfs has `cat` and `tr` but not `dd`** — and `2>/dev/null`
  turned "command not found" into a written marker, so the probe reported
  success from a missing binary.
* A 29-byte write to a dm-crypt mapper is `EINVAL`.
* `local n="$1" disk="…$n…"` leaves `$n` unset, because bash expands every word
  before assigning any of them.
