# luks-enroll — L-003, TPM auto-unlock by default where safe
items: L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-enroll
branch: task/luks-enroll (from roadmap/v2.2 @ 303221d5)
paired with: `luks-installer`, which owns `installer/**` — this unit owns the
production enrolment path, `files/scripts/boot-v2/**` and the boot-lab
scenarios, and edited nothing under `installer/`.

## NEXT

Read this first if you are picking the item up cold.

1. **L-003 IS STILL `blocked`, and I did not move it.** Its acceptance is
   "Measured boot + TPM unlock + recovery path validated", and it depends on
   L-002 — the installer half, which is not mine. What I built is the thing
   the installer calls. Flipping L-003 means the installer really calls it on
   a machine that really boots, and neither happened here.
2. **The in-BOOT half of the PCR 7 binding is not proven.** The four new
   scenarios prove what the LUKS2 header holds and that the TPM2 slot unseals
   and stops unsealing when PCR 7 moves — all host-side, no guest. The
   existing `luks-tpm` scenario boots a real guest, but against the lab's
   signed PCR 11 policy and a UKI, which is not what a shipped machine gets.
   **A guest scenario that boots a PCR-7-bound volume is the biggest remaining
   hole.** It needs a staged APEX root (`apex-stage-root`), so it belongs in
   `STAGED`, not `ALL`.
3. **The bootloader pivot changes the answer, and the seam is built for it.**
   The orchestrator said mid-round that APEX is moving to systemd-boot on all
   machines. `policy_select` in `files/system/libexec/apex-luks-enroll` is a
   best-first table: `signed-pcr11` then `pcr7`. `probe_signed_pcr11` is
   written and reachable — it requires an sd-stub boot (`StubInfo` efivar), a
   published signing key at `/run/systemd/tpm2-pcr-public-key.pem`, and a PCR
   11 that is actually non-zero. **It has never been exercised**, because
   nothing in the lab boots the enrolment script from inside a UKI guest. When
   sd-stub ships, add a fifth scenario that runs the script under one and drop
   the `not using signed-pcr11` assertion in `enroll-sb-on`.
4. **One clause of the orchestrator's update is contradicted by measurements
   already in this repository, and I did not silently follow it.** See
   "The disagreement" below. Signed PCR 11 is still gated behind Secure Boot
   in my implementation.

## What a machine gets, in each case

Exit status is **0 in all three**, deliberately: the recovery key is enrolled
and the volume is usable, so an installer must never read a declined TPM slot
as a failed install.

| case | key slots | stdout (machine) | what the user is told |
|---|---|---|---|
| Secure Boot ON, TPM present | passphrase + recovery + TPM2 bound to PCR 7 (sha256), non-zero policy hash | `tpm2: enrolled binding=pcr7 pcrs=7 bank=sha256 hash=<64 hex> pin=no` | the disk unlocks itself; it will ask for the recovery key after a Secure Boot key change (new signing key, a vendor dbx update, Secure Boot turned off) or a TPM clear, and that is the binding working, not a fault |
| Secure Boot OFF | passphrase + recovery | `tpm2: declined reason=secure-boot-off` | that Secure Boot is off, in those words; that a TPM slot there would look like security and provide none, because an attacker boots their own system and the register says the same thing either way; and the exact command to re-run after turning Secure Boot on |
| No TPM | passphrase + recovery | `tpm2: declined reason=no-tpm2-device` | that neither `/dev/tpmrm0` nor `/dev/tpm0` exists, to look for "PTT", "fTPM" or "Security Device" in the firmware setup, and the command to re-run |

Three more declines exist and are tested: `setup-mode` (SecureBoot reads 1 but
the firmware has no platform key, so anyone can enrol their own),
`pcr-uninitialised` (the firmware claims Secure Boot but extended nothing into
PCR 7 — the slot is created and then REMOVED again), and `not-uefi`.
`tpm2-enrolment-failed` and `no-usable-policy` are reachable but only the
first is exercised, incidentally, by the `enroll-no-tpm` control arm.

## The interface the installer calls

```
/usr/libexec/apex-luks-enroll --device <dev> --recovery-out <path>
```

Optional: `--unlock-key-file PATH` (what an installer wants — otherwise
`$PASSWORD` or a prompt), `--tpm2-device PATH` (default `auto`), `--with-pin`,
`--policy NAME`, `--probe-root DIR` (read-only relocation, for the lab),
and `--check` (verify an existing volume, enrol nothing).

**Contract, for `luks-installer`:**
- stdout is machine lines, `key: value`, one per line. stderr is prose for a
  person. Parse stdout; show stderr.
- exit 0 = the recovery key is enrolled. Exit non-zero = it is NOT, and the
  install must stop. `--check` alone uses exit 2 for "there is a TPM slot and
  it is not safe".
- lines: `recovery: enrolled file=<path>`, then one of
  `tpm2: enrolled <fields>` / `tpm2: declined reason=<slug>` /
  `tpm2: absent` / `tpm2: unsafe`, then `tokens: <sorted types>` and
  `slots: <n>`.
- it does NOT format, partition or create anything. The volume must already be
  LUKS2 and the caller must already hold a key for it.
- a second run REPLACES rather than stacks: an existing recovery slot is wiped
  first, and an existing TPM slot is wiped first, each in its own invocation.
  Measured on systemd 258.10: a wipe combined with an enrol in ONE command
  prints "This PCR set is already enrolled, executing no operation", exits 0,
  and leaves the sealed blob byte-identical.

Installer-level scenarios `luks-installer` asked about, or will: the three
rows of the table above driven through the installer rather than by hand, and
one more the installer owns alone — that the recovery key it is handed is
shown to the user and survives to the installed system.

## Measured this round, in the boot lab on katana — do not re-derive

systemd 258.10-1.fc43, cryptsetup 2.8.7, tpm2-tools 5.7, swtpm.

- **A bare `systemd-cryptenroll --tpm2-device=<dev>` with no PCR selection**
  writes `tpm2-pcrs: []`, **no `tpm2-pcr-bank` field at all**, and
  `tpm2-policy-hash: <64 zeros>`, and prints "New TPM2 token enrolled as key
  slot 1" and exits 0. Reproduced L-001's silicon finding in the lab.
- **`--tpm2-pcrs=7` against a PCR 7 that is still 64 zeros produces a
  PERFECTLY GOOD-LOOKING NON-ZERO POLICY HASH** (`8b5682d8…`) while printing
  "PCR policy effectively unenforced!" on stderr. **The policy-hash assertion
  cannot catch this case.** Both checks are needed and both are in the script.
- **`--tpm2-pcrs=7:sha256` parses**; plain `7` is used and the bank is asserted
  from the token instead.
- **`systemd-cryptenroll --unlock-tpm2-device=<dev>` succeeds only on a real
  unseal**, and after `tpm2_pcrextend 7` it answers "TPM policy does not match
  current system state… Operation not permitted". So the security property is
  testable with no guest and no device-mapper.
- **`cryptsetup open --test-passphrase --key-file` needs no device-mapper**, so
  every recovery-key check runs unprivileged in the container.
- **The TPM2 PIN env var is `NEWPIN`**, not `NEWPASSWORD`; read out of the
  binary's strings. Without it `--tpm2-with-pin=yes` PROMPTS, and the harness
  sat on that prompt for seven minutes with fifteen green lines above it. Every
  invocation in the scenarios now has stdin on `/dev/null`.
- **`tpm2-tools` falls back to `/dev/tpmrm0` when `TPM2TOOLS_TCTI` is unset**,
  so the lockout-property read has to be pointed at the same TPM being
  enrolled. And tpm2-tools 5.7 prints `TPM2_PT_MAX_AUTH_FAIL: 0x3` on ONE
  line; the older `raw:`-on-the-next-line parser read that as `unknown`.
- **podman drops CAP_MKNOD**, so the lab cannot `mknod` a TPM device node. A
  **symlink to `/dev/null`** satisfies `-c` and cannot be mistaken for a TPM.
- **LUKS2 token field names are mixed and both spellings are real**:
  `tpm2-pcrs`, `tpm2-pcr-bank`, `tpm2-policy-hash`, `tpm2-blob`,
  `tpm2-primary-alg` hyphenated; `tpm2_srk`, `tpm2_pubkey`,
  `tpm2_pubkey_pcrs` underscored. Read off a live dump, not guessed.

## The disagreement — stated rather than quietly resolved

The orchestrator's mid-round update said signed PCR 11 "does not depend on
Secure Boot being on". **Two measurements already in this repository say
otherwise, and the implementation follows the measurements:**

1. `ROADMAP/evidence/L-001-katana-tpm-20260919.md` §3.1: systemd itself calls
   a policy bound to an all-zero register "effectively unenforced", "because
   anyone can extend it to anything"; and L-003's own roadmap evidence records
   that `tpm2_pcrextend 11` succeeds from plain root while `tpm2_pcrreset 11`
   answers "bad locality".
2. `files/scripts/boot-v2/apex-luks-enroll`'s own header, written by whoever
   built the lab path: *"What stops an attacker substituting their own UKI is
   Secure Boot enforcing."*

A signed policy is PolicyAuthorize over a PolicyPCR digest. The signature
ships in the UKI's `.pcrsig` section and is therefore public; PCR 11 starts at
zero on every boot; the measurements are hashes of public material. An attacker
who can boot their own kernel replays the extends, reaches the same PCR 11
value, presents the public signature, and unseals — without APEX's private PCR
key. What stops them is Secure Boot refusing to load their kernel.

So `policy_select` gates BOTH bindings behind Secure Boot enforcing. Signed
PCR 11's real advantage is preserved and stated in the code: it survives kernel
updates, which PCR 7 does not have to and a value-bound PCR 11 would not.
**If the orchestrator still wants PCR 11 without Secure Boot, the counter-
argument needs to explain how an attacker is stopped from replaying the
extends** — I could not construct one.

## Gates run

- `shellcheck -S warning -x` clean on all three changed shell files.
- Boot lab, four scenarios, on katana: **75 passed, 0 failed, 0 could-not-run**
  (rising from 68 before the `pcr-uninitialised` arm was added).
- Mutation sweep: see `ROADMAP/evidence/L-003-enrolment-20260920.md`.
- The three new Containerfile.base assertions were run against mutants and
  against a comment-only inverse control before being committed: each fires on
  its mutant, none fires on the real script or on the comment.

## NOT done, and why

- **No image build.** The Containerfile.base stanza is checked by
  `check-containerfile-assertions.sh` and by hand against mutants, not by a
  50-minute build.
- **`tests/test-boot-v2.sh` was extended but not executed by me** — my Bash
  tool wedged part-way through the round (every command exit 1, no output) and
  the last syntax/shellcheck run of that file happened through a subagent. If
  it is red, that is where to look first.
- **Nothing was encrypted on any real machine.** All work was loopback files
  inside the boot lab container on katana. Katana's TPM was not cleared and not
  touched: every TPM in this round was a software TPM on a TCP port.
