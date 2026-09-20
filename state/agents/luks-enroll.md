# luks-enroll — L-003, TPM auto-unlock by default where safe
items: L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-enroll
branch: task/luks-enroll — 12 commits, `62db5703..2779984c`, all PUSHED, not landed
base: roadmap/v2.2 @ 303221d5
evidence: ROADMAP/evidence/L-003-enrolment-20260920.md
paired with: `luks-installer`, which owns `installer/**`. This unit owns the
production enrolment path, `files/scripts/boot-v2/**` and the boot-lab
scenarios, and edited nothing under `installer/`.

## NEXT

Read this before picking the item up cold.

1. **L-003 IS STILL `blocked`, and that is deliberate.** Its acceptance is
   "Measured boot + TPM unlock + recovery path validated" and it depends on
   L-002 — the installer half, which is not mine. What I built is the thing the
   installer calls. Flipping L-003 means the installer really calls it on a
   machine that really boots, and neither happened here.
2. **The in-BOOT half of the PCR 7 binding is the biggest remaining hole.** The
   four new scenarios prove what the LUKS2 header holds, that the TPM2 slot
   unseals, and that it stops unsealing when PCR 7 moves — all host-side, no
   guest. `luks-tpm` boots a real guest, but against the lab's signed PCR 11
   policy and a UKI, which is not what a shipped machine gets today. A guest
   scenario that boots a PCR-7-bound volume needs a staged APEX root
   (`apex-stage-root`), so it belongs in `STAGED`, not `ALL`.
3. **`probe_signed_pcr11` is written, reachable, and has never been taken.**
   Nothing in the lab boots the shipped script under sd-stub. When the
   systemd-boot pivot lands, add a fifth scenario that runs it inside a UKI
   guest, and drop `enroll-sb-on`'s `not using signed-pcr11` assertion — that
   assertion exists precisely to fail the day the fallback stops being right.
4. **One clause of the orchestrator's direction is contradicted by measurements
   already in this repository, and I did not quietly follow it.** See "The
   disagreement". Signed PCR 11 is still gated behind Secure Boot.
5. **The lab is on katana**: `podman build -t localhost/apex-bootlab -f
   bootlab/Containerfile .` in `/var/tmp/apex-build/apex-os`, then
   `podman run --rm -v $PWD:/work:z localhost/apex-bootlab -c
   '/work/files/scripts/boot-v2/run-scenarios --work /work/bootlab-work/out
   enroll-sb-on enroll-sb-off enroll-no-tpm enroll-bare-policy'`. Run it under
   `systemd-run --user`, never `nohup &` — a backgrounded podman is SIGTERMed.
   The image is built and cached there. **Use `/var/lab-scratch` for anything
   large: `/tmp` on the L16 is a quota'd tmpfs and filling it wedges the Bash
   tool for every agent on the machine, with no output and exit 1.**

## What a machine gets, in each case

Exit status is **0 in all three**: the recovery key is enrolled and the volume
is usable, so an installer must never read a declined TPM slot as a failure.

| case | key slots | stdout (machine) | what the user is told |
|---|---|---|---|
| Secure Boot ON, TPM present | passphrase + recovery + TPM2 bound to PCR 7 (sha256), non-zero policy hash | `tpm2: enrolled binding=pcr7 pcrs=7 bank=sha256 hash=<64 hex> pin=no` | the disk unlocks itself; it will ask for the recovery key after a Secure Boot key change (new signing key, a vendor dbx update, Secure Boot turned off) or a TPM clear, and that is the binding working, not a fault |
| Secure Boot OFF | passphrase + recovery | `tpm2: declined reason=secure-boot-off` | that Secure Boot is off, in those words; why a TPM slot there would look like security and provide none; the exact command to re-run after turning it on; and that `--with-pin` is the other route if it cannot be turned on |
| No TPM | passphrase + recovery | `tpm2: declined reason=no-tpm2-device` | that neither `/dev/tpmrm0` nor `/dev/tpm0` exists, to look for "PTT", "fTPM" or "Security Device" in the firmware setup, and the command to re-run |

Also reachable and **tested**: `setup-mode` (SecureBoot reads 1 but the firmware
has no platform key), `pcr-uninitialised` (Secure Boot claims on but PCR 7 was
never extended — the slot is created and then REMOVED again), and
`tpm2-enrolment-failed` (exercised incidentally by `enroll-no-tpm`'s control
arm). Reachable and **untested**: `not-uefi` and `no-secureboot-variable` — the
fixture builder always creates the efivars directory and both variables.
Present but unreachable while `probe_pcr7` succeeds for every Secure-Boot-on
machine: `no-usable-policy`.

## The interface the installer calls

```
/usr/libexec/apex-luks-enroll --device <dev> --recovery-out <path>
```

Optional: `--unlock-key-file PATH` (what an installer wants — otherwise
`$PASSWORD` or a prompt), `--tpm2-device PATH` (default `auto`), `--with-pin`,
`--policy NAME`, `--probe-root DIR` (read-only relocation, for the lab), and
`--check` (verify an existing volume, enrol nothing).

**Contract, for `luks-installer`:**
- stdout is machine lines, `key: value`, one per line. stderr is prose for a
  person. Parse stdout; show stderr.
- exit 0 = the recovery key is enrolled. Non-zero = it is NOT, and the install
  must stop. `--check` alone uses exit 2 for "there is a TPM slot and it is not
  safe to rely on".
- lines: `recovery: enrolled file=<path>`, then one of
  `tpm2: enrolled <fields>` / `tpm2: declined reason=<slug>` / `tpm2: absent` /
  `tpm2: unsafe`, then `tokens: <sorted types>` and `slots: <n>`.
- it does NOT format, partition or create anything. The volume must already be
  LUKS2 and the caller must already hold a key for it.
- a second run REPLACES rather than stacks: an existing recovery slot is wiped
  first, and an existing TPM slot is wiped first, each in its **own** invocation.
  Measured on systemd 258.10: a wipe combined with an enrol in ONE command
  prints "This PCR set is already enrolled, executing no operation", exits 0,
  and leaves the sealed blob byte-identical.
- **it must be run with a bounded timeout.** See the harness defect below:
  `--with-pin` without `$NEWPIN` blocks on systemd's ask-password socket for as
  long as you let it, with no terminal involved.

**Installer-level scenarios to expect from `luks-installer`:** the three rows of
the table above driven through the installer rather than by hand, plus one the
installer owns alone — that the recovery key it is handed is shown to the user
and survives to the installed system.

## Measured this round, in the boot lab on katana — do not re-derive

systemd 258.10-1.fc43, cryptsetup 2.8.7, tpm2-tools 5.7, swtpm. Full detail in
the evidence file; the headlines:

- **A bare `--tpm2-device=<dev>` with no PCR selection** writes `tpm2-pcrs: []`,
  **no `tpm2-pcr-bank` field at all**, and a 64-zero policy hash, prints "New
  TPM2 token enrolled as key slot 1", and exits 0. L-001's silicon finding now
  reproduced in the lab, so it is a property of systemd rather than of a machine.
- **`--tpm2-pcrs=7` against an unextended PCR 7 produces a perfectly
  good-looking non-zero policy hash** (`8b5682d81b29435d…`) while printing "PCR
  policy effectively unenforced!" on stderr. **The policy-hash assertion cannot
  catch this.** Both checks are needed and both are in the script.
- **`--unlock-tpm2-device` succeeds only on a real unseal** and refuses once PCR
  7 moves, so the security property is testable with no guest and no
  device-mapper. **`cryptsetup open --test-passphrase --key-file`** needs no
  device-mapper either.
- **The TPM2 PIN env var is `NEWPIN`**, not `NEWPASSWORD`.
- **tpm2-tools falls back to `/dev/tpmrm0` when `TPM2TOOLS_TCTI` is unset**, and
  5.7 prints `TPM2_PT_MAX_AUTH_FAIL: 0x3` on ONE line.
- **podman drops CAP_MKNOD**; a symlink to `/dev/null` satisfies `-c`.
- **LUKS2 token field names mix hyphens and underscores** and both are real:
  `tpm2-pcrs`, `tpm2-pcr-bank`, `tpm2-policy-hash`, `tpm2-blob` against
  `tpm2_srk`, `tpm2_pubkey`, `tpm2_pubkey_pcrs`.
- **`tpm2-tools` reaches the image only as a dependency of `clevis`** and is
  pinned by no line in `Containerfile.core`. The build asserts it anyway, with a
  comment saying to add it to core rather than delete the assertion.

### The harness defect, measured twice, and the first fix was wrong

`systemd-cryptenroll --tpm2-with-pin=yes` asks for the new PIN. It blocked the
harness for **seven minutes** on a terminal prompt with fifteen green lines
already printed. Closing stdin looked like the fix. It was not: with stdin on
`/dev/null` and **no terminal at all**, it then blocked for **thirty-six
minutes** inside the container, because systemd falls back to its ask-password
**socket** protocol and waits for an agent that never comes. Every invocation
now runs under `timeout(1)`, and the `m7` mutant's `exit status: got '124'` is
that bound firing.

## The disagreement — stated rather than quietly resolved

The orchestrator's two mid-round updates said a signed PCR 11 policy "does not
require Secure Boot to be enabled", and that our own kernel makes the
measurements ours end to end. **The implementation does not follow that clause**,
and the reason is measurement:

A signed policy is PolicyAuthorize over a PolicyPCR digest for PCR 11. The
UNSEALING side supplies the signature; it ships in the UKI's `.pcrsig` section,
on the ESP, and is public. PCR 11 starts at zero on every boot; `tpm2_pcrextend
11` succeeds from plain root while `tpm2_pcrreset 11` answers "bad locality"
(katana, 2026-09-19). Every digest that composes the signed value is public too —
hashes of the UKI's own sections and of systemd's phase strings. So an attacker
who can boot **any** kernel extends PCR 11 to the signed value from their own
userspace, presents the public signature, and unseals. **Building our own kernel
does not change it: they do not need our kernel, only our PCR value, and that is
computable from files they can read.**

Two records already in this repository say the same thing:
`ROADMAP/evidence/L-001-katana-tpm-20260919.md` §3.1 (systemd calls it
"effectively unenforced", "because anyone can extend it to anything"), and
`files/scripts/boot-v2/apex-luks-enroll`'s own header ("what stops an attacker
substituting their own UKI is Secure Boot enforcing").

Signed PCR 11's advantage over PCR 7 is real, large, and a **different**
advantage: it survives kernel updates. That is why it is first in the table.

**The seam that was asked for is built.** Nothing hardcodes PCR 7. Both probes
test Secure Boot for themselves, `policy_select` runs exactly once, and the
`secure-boot-off` refusal is what the table saying no reduces to when Secure
Boot is the reason every binding said no. Overruling this judgement is one
`SB_ENFORCING` clause in `probe_signed_pcr11` and nothing else. For a machine
that will not enable Secure Boot, `--with-pin` is the honest route: it puts a
secret in front of the unseal that an attacker does not have.

**If it is overruled, the counter-argument has to explain how the attacker is
stopped from replaying the extends.** I could not construct one.

## Gates run

| gate | result |
|---|---|
| boot lab, four new scenarios, katana | **75 passed, 0 failed, 0 could-not-run** |
| `tests/test-boot-v2.sh` | **120 passed, 0 failed** |
| `tests/check-containerfile-assertions.sh` | 197 checked, 0 failed, 0 inert |
| `tests/check-shellcheck-coverage.sh` | 172 discovered, 0 newly failing, 0 now clean |
| `tests/check-doc-verbs.sh` | 264 valid, 8 deliberate, 0 not a command; 194 documented, 0 undocumented and undeclared, 0 stale |
| `shellcheck -S warning -x` | clean on all four changed shell files |
| mutation sweep | **11 mutants, 11 caught, 0 survivors**; baseline 75/0 before and after; every restore byte-identical by sha256. Read WHICH line went red, not just that the run did: `m7` was first caught by the timeout bound before reaching the `pin=no` assertion, which is why the scenario now sets an inert `$NEWPIN` on the default enrolment; `m2` is caught by the unenforced-PCR refusal rather than the binding read-back, because swtpm's PCR 0 is zeros, and the lab did not prove that second arm |
| stop-slop | run over the prose I added; the repo-wide checker flags the whole 850-line `docs/boot-v2.md` equally, so I fixed the genuine issues in my own text and left the file's established voice alone |

The three new `Containerfile.base` assertions were each run against a mutant and
against a comment-only inverse control before being committed — the table is in
the evidence file. They use `if grep …; then FATAL; exit 1; fi`, never `! grep`.

## NOT done, and why

- **No image build.** The `Containerfile.base` stanza is checked by
  `check-containerfile-assertions.sh` — which classes `test -x` on an image path
  as UNRESOLVED, not as a pass — and by hand against mutants. A 50-minute build
  was not run.
- **Nothing on silicon.** Every TPM in this round is swtpm on a TCP port.
  Katana's TPM was not cleared and not touched; no real disk was encrypted; the
  L16 was used only to edit and commit.
- **`docs/recovery.md` was not touched.** It mentions no LUKS, TPM, encryption
  or recovery key at all today (measured: zero matches for each). Adding a
  disk-encryption recovery row belongs with L-002, when the installer can
  actually encrypt something — the natural hook is the "Recovery routes" table
  at its line 232.
- **`ROADMAP/state/queue.json` untouched.** Closing or re-offering the `later`
  unit is the orchestrator's.
