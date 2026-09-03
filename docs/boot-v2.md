# Boot v2 — signed UKIs, boot counting, measured boot

Roadmap §22. This is the reference for the systemd-boot + Unified Kernel Image
path: what exists, what it was measured to do, how a developer opts a machine
into it, and how to get back.

## GRUB is the default, and that is not a temporary state

**Every published APEX image — `daily`, `gaming-mesa`, `gaming-nvidia` — boots
through GRUB, and will for this generation of APEX.** The systemd-boot + UKI
path described here is **opt-in**, per machine, by hand.

§23's implementation table reads "Boot v2: composefs + systemd-boot + UKIs +
measured boot", which alone sounds like a bootloader swap. §22 says the
opposite. Its own title is *"do not switch to Limine as the main path"*, its
recommendation is to **keep GRUB for the current APEX generation** while the
OSTree/bootc install path depends on it, and to keep it for legacy BIOS
regardless. Its anti-goal is explicit: *do not switch bootloaders for
aesthetics; change the boot architecture only when it improves reliability,
verification and rollback.*

So a change that makes systemd-boot the default for a published flavor
violates the section it claims to implement, and the Secure Boot product
invariant in `AGENTS.md` at the same time. `AGENTS.md`'s boot-path rules carry
this as rule 5.

Two consequences visible in the shipped image:

* Nothing in the image runs `bootctl install`, `bootctl update`, `bootupctl`,
  `grub2-install` or `efibootmgr -c`. `tests/test-boot-v2.sh` scans every
  shipped unit and helper for those commands on **executable** lines and fails
  the build if one appears. Enrollment is the human procedure below.
* The two units that implement boot counting carry
  `ConditionPathExists=/sys/firmware/efi/efivars/LoaderBootCountPath-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`.
  That variable is set only when systemd-boot booted the machine **with a boot
  counter in effect**, so on a GRUB machine neither unit starts, and a failed
  condition is a skip rather than a failure.

  The condition is written against `LoaderBootCountPath` specifically, and that
  choice is load-bearing. Measured on both real machines: the laptop has no
  `Loader*` variables at all, but **the katana carries `LoaderInfo`,
  `LoaderDevicePartUUID` and `LoaderSystemToken` while still booting GRUB 2.12**.
  A condition written against `LoaderInfo` — the obvious "is systemd-boot
  involved" test — would have fired these units on a machine that never boots
  through systemd-boot. Any further conditioned unit must use the same variable.

## What exists

| piece | file | what it does |
| --- | --- | --- |
| UKI builder | `files/scripts/boot-v2/apex-mkuki` | kernel + initramfs + signed cmdline + microcode + `.apexinf` metadata in one PE image |
| stage an APEX root | `files/scripts/boot-v2/apex-stage-root` | copies kernel/initramfs/os-release out of a booted deployment or image |
| ESP authoring | `files/scripts/boot-v2/apex-mkesp` | systemd-boot at `/EFI/APEX/`, UKIs at `/EFI/Linux/apex-<id>+N-M.efi` |
| ephemeral keys | `files/scripts/boot-v2/apex-sb-keys` | Secure Boot, PCR-policy and deliberately-untrusted keypairs |
| SB firmware vars | `files/scripts/boot-v2/apex-sb-vars` | an OVMF varstore with the APEX certificate as the only `db` entry |
| LUKS2 + TPM | `files/scripts/boot-v2/apex-luks-enroll` | signed PCR 11 policy plus a recovery key, against a software TPM |
| VM harness | `files/scripts/boot-v2/run-scenarios` | ten scenarios, all booting real guests under Secure Boot enforcing |
| health gate | `files/system/libexec/apex-boot-health` | the `boot-complete.target` gate, and the rollback notice |
| reporting | `apex boot status` | read-only; what verified this boot and what the counter believes |
| CI | `.github/workflows/boot-v2.yml` | builds the lab, boots the scenarios, publishes nothing |

Build-time tooling (`ukify`, `qemu`, `sbsign`, `swtpm`) lives in the
`apex-bootlab` container built from `bootlab/Containerfile`, never as packages
on a host. Installing it onto an APEX box is the machine drift `AGENTS.md`
prohibits, and the build box is a real APEX machine.

## The ESP layout, and the one directory APEX does not own

```
/EFI/APEX/systemd-bootx64.efi        the loader, at an APEX-owned path
/EFI/BOOT/BOOTX64.EFI                the removable-media fallback
/EFI/Linux/apex-<deployment>+N-M.efi the UKIs
/loader/loader.conf                  timeout 0, editor no
```

§22 asks for "APEX-owned EFI paths, not Fedora-named paths". `\EFI\Linux` is
the systemd-boot interface — a spec path, not a vendor name like the
`\EFI\fedora` §22 is reacting to — and APEX owns the **filenames**, which is
what the menu, `bootctl list` and `apex boot status` display.

A fully `/EFI/APEX`-named UKI path is also possible, and the choice was made on
measurement rather than assumption. Measured with systemd-boot 258.10-1.fc43:

| entry | boot counter applied? |
| --- | --- |
| type #2, `/EFI/Linux/apex-t2+3-0.efi` | **yes** → `+2-1` |
| type #1 `.conf` with `efi /EFI/APEX/uki/apex-t1.efi` | **no** (the entry booted fine) |
| type #1 `.conf` with `linux /EFI/APEX/uki/apex-t3.efi` | **yes** → `+2-1` |

So the tally is not a property of the entry type: it is skipped for entries
named with the `efi` key. Type #2 is the default anyway, because `bootctl`
reports the tally, the `.osrel`-derived title and the embedded kernel version
only for entries it recognises as UKIs — and `apex boot status` reads
`bootctl list --json`. A type #1 layout would need APEX to author an entry file
per deployment *in addition to* the UKI, which is the two-artifacts-must-agree
drift a UKI exists to remove. All three rows are asserted, so a future systemd
that starts counting `efi` entries fails the suite instead of silently changing
the trade-off.

## Boot counting and automatic rollback

APEX writes no boot counter, and cannot. The boots that need counting are the
ones that never reach userspace — a kernel that panics, an initramfs that
cannot find its root, a driver that hangs before the display comes up. Nothing
in userspace can increment a counter for a boot that never got there.

systemd-boot decrements the count **before the kernel starts**, by renaming the
entry file in the ESP. What APEX contributes is the definition of a healthy
boot, contributed through the upstream extension point:

```
apex-boot-health.service   Before=boot-complete.target, RequiredBy= it
        ↓ non-zero exit
boot-complete.target       not reached
        ↓
systemd-bless-boot.service Requires=boot-complete.target — does not run
        ↓
the +N-M suffix survives   the next boot spends another try
        ↓ at +0-3
systemd-boot selects the previous, blessed entry (no suffix at all)
```

`RequiredBy`, not `WantedBy`: with `WantedBy` the target would be reached with
the check failing, which blesses a broken deployment — the outcome §22 forbids
with *"do not mark an update successful merely because the kernel started."*

**Health is an explicit short list**, not `systemctl is-system-running` and not
upstream's `systemd-boot-check-no-failures`, both of which fail on *any* failed
unit. `AGENTS.md` requires optional hardware and services to fail without
degrading the boot transaction, so an APEX machine can legitimately be
`degraded` — an absent fan controller, a Bluetooth adapter that did not appear.
Rolling the OS back over one of those would be worse than the fault. The list:

* the default target (`systemctl get-default`) is active
* `apexd.service` is active
* `systemd-logind.service` is active
* the system bus (`dbus-broker.service`, or `dbus.service` if that is what the
  image has) is active
* `greetd.service` is active, **only** when the default target is
  `graphical.target` — requiring a greeter on a deliberately headless machine
  would roll it back for working as configured

`apex-boot-notice.service` runs *after* `boot-complete.target`, so the notice
it writes always reads "you were rolled back and the machine is now working"
rather than appearing on a machine that is still failing. There is deliberately
no `apex boot ack` verb: the notice is cleared automatically on a boot where no
entry is exhausted, so it tracks reality instead of tracking whether somebody
dismissed it.

## Measured boot: the PCR policy, and the one that was rejected

The hard requirement is a policy that survives a legitimate kernel update. On
an image-based OS the kernel changes on every update, so a policy that breaks
when the kernel changes breaks on every update — and the user meets it at the
one moment they cannot get a shell.

**Chosen: a signed PCR 11 policy.** `ukify` computes the PCR 11 values the UKI
will produce in each boot phase, signs them with an RSA key, and embeds the
signatures as `.pcrsig` with the public half as `.pcrpkey`.
`systemd-cryptenroll --tpm2-public-key=` then binds the LUKS2 keyslot to the
**public key**, through a TPM2 `PolicyAuthorize`, rather than to a measurement.
Any UKI signed by that key satisfies the policy, so a kernel update needs no
re-enrollment and no user interaction. The trust anchor becomes a key APEX
already owns and already protects — the same shape as the Secure Boot chain.

**Rejected: `systemd-pcrlock`.** It predicts the firmware PCRs (0, 2, 4, 7)
from the TPM event log and writes the resulting policy into a TPM NV index.
Three problems, all about the update path rather than the security model:

* `systemd-pcrlock make-policy` must be re-run, with TPM access, after every
  kernel install. A missed run is an unbootable machine.
* The policy lives in an NV index, so a firmware reset or a TPM clear loses it
  — and firmware resets are exactly the "hardware edge case" §22's step 6 wants
  proven before encryption goes on by default.
* It binds firmware measurements, so a UEFI update also invalidates it.

**The honest limitation**: signed PCR 11 attests the UKI and the boot *phase*.
It does not attest the firmware or the Secure Boot state. What stops an attacker
substituting their own UKI is Secure Boot enforcing, the layer that makes only
APEX-signed images loadable. The two are complementary and neither alone is
enough.

## What was measured

All figures below are from the katana on 2026-09-03 — a real APEX machine,
`VARIANT_ID=gaming`, kernel `7.1.5-cachyos1.fc43.x86_64` — with guests under
`OVMF_CODE_4M.secboot` and only the ephemeral APEX certificate enrolled as
PK/KEK/db. Nothing here is a prediction.

**A UKI from the real APEX image boots.** Kernel 16,758,856 bytes, the real
APEX initramfs 386,072,073 bytes, the signed UKI ~390 MB. sd-stub printed
`Booting initrd of APEX-OS dracut-107-8.fc43 (Initramfs)`, the real APEX
initramfs ran to dracut's `pre-mount` hook, reported `NAME="APEX-OS"` from
`/etc/initrd-release` and the command line the image was signed with, and
powered off cleanly. `.apexinf` recorded `microcode=embedded-in-initrd`, which
is how §22's "microcode in the UKI" is satisfied on the real image: dracut
prepends an uncompressed `kernel/x86/microcode` cpio (`AuthenticAMD.bin` 304,866
bytes, `GenuineIntel.bin` 16,778,240 bytes) and `apex-mkuki` detects it rather
than demanding a duplicate `--ucode`.

**Secure Boot refuses everything else.** Unsigned, foreign-signed, and
one-byte-tampered-in-`.cmdline` UKIs each failed to reach userspace. Every
mutant is proven to exist and to differ from the original before the boot, and
the foreign one is proven to be *validly* signed by a key that is not in `db` —
"unsigned is refused" would be a much weaker claim.

**Reproducibility — of the payload, not of the signature.** Two *unsigned*
builds with the same `SOURCE_DATE_EPOCH` are byte-identical, and a different
epoch changes the bytes, so the property is controlled rather than accidentally
true.

The **signed** UKI is not byte-reproducible and cannot be: `sbsign` records a
signing time in the PKCS#7 structure, and there is no flag to omit it. Measured
directly — one unmodified `linuxx64.efi.stub` signed twice, two seconds apart,
with one key, gave `c266048303a17056…` and `66d64b06a66db973…`.

This was found the hard way. The scenario originally asserted that two *signed*
builds were byte-identical. It passed repeatedly, then failed on a CI run whose
only changes were to documentation — because two signings match exactly when
they land in the same second. So the assertion now covers the unsigned payload,
the signed artifact is asserted to *verify* rather than to be identical, and the
harness reports whether the signed bytes happened to match without asserting it
either way: a lucky run must not read as evidence of a property that does not
hold.

**Boot counting.** Four boots walked `apex-new+3-0.efi` →`+2-1` →`+1-2`
→`+0-3`, the fifth selected the unsuffixed `apex-good.efi`, and a sixth stayed
there. Exact filename pairs are asserted at every step, in both directions: the
counted entry decrements and the blessed entry never grows a suffix.

**Measured boot and TPM-bound LUKS2**, with `swtpm`:

* the enrolled state unlocks, with PCR 11 at a real measurement (e.g.
  `4CBAF0A3342F8628AB22E15C8EEB4FF4A60DE4F2B8D9DAC52473943A24C4BCC3`), and the
  signature and public key delivered to
  `/run/systemd/tpm2-pcr-{signature.json,public-key.pem}`
* PCR 11 **changes** between two UKIs signed by the same PCR key, and the same
  keyslot still opens with no re-enrollment — the kernel-update requirement,
  measured rather than reasoned
* a UKI whose `.pcrsig` was made by a **different** key is refused
  (`Failed to unseal secret using TPM2`)
* the **recovery key unlocks in the same boot that was refused**, so "it
  refuses" and "it is recoverable" are not two green checks that never met
* a 512-byte marker written through the mapper device in the first boot is read
  back in the second, so the volume really decrypted rather than a device
  merely appearing

**A firmware fact worth knowing before debugging anything here.** Fedora's 2 MB
`/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd` does Secure Boot but has **no TCG2
protocol**: sd-stub sets no `StubPcr*` variables, PCR 11 reads as 64 zeros, and
every TPM unlock fails with *"No signature for current PCR policy in TPM2
signature JSON"* — which reads exactly like a broken PCR policy. Only
`OVMF_CODE_4M.secboot.qcow2` measures (367 `Tcg2` lines in the firmware debug
log). `lib.sh` converts the 4 MB pair to raw once and refuses to fall back to
the 2 MB build.

## Enrolling a machine — the human procedure

**Read this whole section before running any of it.** There is no rollback for
an ESP you overwrote or an EFI variable you replaced, because the thing that
would perform the rollback is what you broke. Nothing in APEX automates these
steps, and that is deliberate.

Irreversible, in order of how bad it is to get wrong:

1. **Enrolling a Secure Boot key writes your firmware.** It is done from the
   firmware's own setup UI or with `mokutil`, by you, on a machine you can put
   into Setup Mode. APEX ships no script that touches `db`, `KEK` or `PK`, and
   CI and VMs only ever get ephemeral keys.
2. **`bootctl install` writes the ESP** and creates an EFI boot entry. If the
   machine shares its ESP with another OS, this is where that OS's loader gets
   displaced.
3. **`systemd-cryptenroll` changes a LUKS2 header.** Take the recovery key it
   prints and store it somewhere that is not the encrypted disk, before you
   reboot. Not after.

Before starting: know how to reach your firmware's boot menu on this machine,
and have a live USB you have actually booted once.

```bash
# ── 0. what is the machine doing now? ──
apex boot status                    # expect: Bootloader grub, boot counting not in effect
sudo bootc status                   # note the booted and rollback deployments
sudo ostree admin pin 0             # pin the current deployment before anything risky

# ── 1. build a UKI from THIS machine's image, in the boot lab ──
git clone https://github.com/AndreNijman/apex-os.git ~/build/apex-os
cd ~/build/apex-os
podman build -t apex-bootlab -f bootlab/Containerfile .
mkdir -p ~/bootlab-work
sudo files/scripts/boot-v2/apex-stage-root --output ~/bootlab-work/apex-root
#   ^ reads /usr/lib/modules/<kver>/{vmlinuz,initramfs.img} and os-release.
#     Root because initramfs.img is mode 0600. It writes only the output dir.

# The command line must be the one THIS machine boots with, because a UKI's
# cmdline is inside the signed image and cannot be edited at the boot menu.
# Take it from the running system, not from this document:
cat /proc/cmdline

podman run --rm -v ~/bootlab-work:/work:z apex-bootlab -c '
  /work/apex-os/files/scripts/boot-v2/apex-mkuki \
      --output /work/apex-<deployment>.efi \
      --from-root /work/apex-root \
      --cmdline "<the cmdline from /proc/cmdline>" \
      --deployment "<a short id: the ostree deployment checksum works>" \
      --variant "$(. /etc/os-release; echo "$VARIANT_ID")" \
      --sb-key /work/keys/sb/key.pem --sb-cert /work/keys/sb/cert.pem'
```

`--sb-key`/`--sb-cert` are **your** key, from outside the repository. A private
key never enters the tree, and `.gitignore` blocks the patterns.

```bash
# ── 2. enroll the certificate in firmware. THIS TOUCHES FIRMWARE. ──
#   Reboot into firmware setup, put Secure Boot into Setup Mode, and enroll
#   the DER certificate from a USB stick, or use mokutil and answer the
#   MokManager prompt on the next boot:
sudo mokutil --import /path/to/your-cert.der
#   Verify before going further:
mokutil --list-enrolled | grep -i "<your CN>"

# ── 3. install systemd-boot and place the UKI ──
#   `bootctl install` writes the ESP and adds an NVRAM entry. Nothing in this
#   repository runs it for you.
sudo bootctl install
sudo mkdir -p /boot/efi/EFI/Linux
#   +3-0 is the boot counter: three tries, none used.
sudo cp ~/bootlab-work/apex-<deployment>.efi \
        /boot/efi/EFI/Linux/apex-<deployment>+3-0.efi
sudo bootctl list                   # the entry must appear, with 3 tries left

# ── 4. reboot, and check ──
sudo systemctl reboot
#   after it comes up:
apex boot status
#   expect: Bootloader systemd-boot · Signed UKI yes · Boot counting in effect
#           and the entry listed as "good" — systemd-bless-boot stripped the
#           +N-M suffix because apex-boot-health.service passed.
```

If it does **not** come up: the boot counter is doing its job. Three failed
attempts and systemd-boot selects the previous entry by itself. GRUB is still
installed and still in the firmware's boot order; pick it from the firmware boot
menu.

### TPM-bound unlock (opt-in, developer feature)

Only after the above works, and only on a machine whose data you can afford to
lose. `apex-luks-enroll` is a **boot-lab** script: it targets an image file and
a software TPM, and refuses `--tpm2-device=auto` and any `/dev` path outright.
The equivalent on a real machine is `systemd-cryptenroll` run by you:

```bash
# Recovery FIRST. A volume with a TPM binding and no recovery path turns a
# firmware update into a data-loss event.
sudo systemd-cryptenroll --recovery-key /dev/<your-luks-partition>
#   Write the modhex key down. It is 8 groups of 8 characters. It is shown once.

sudo systemd-cryptenroll --tpm2-device=auto \
     --tpm2-public-key=/path/to/pcr-public-key.pem \
     --tpm2-public-key-pcrs=11 \
     /dev/<your-luks-partition>
```

and the UKI must then be built with `--pcr-key` so it carries the matching
`.pcrsig`. A UKI without one will be refused by the policy, which is the
designed behaviour and not a fault.

## Recovery

| situation | what to do |
| --- | --- |
| the new deployment will not boot | do nothing for three attempts; systemd-boot selects the previous blessed entry itself. `apex boot status` then shows the failed entry as `OUT OF TRIES` and announces the rollback. |
| the machine boots but the desktop does not | `apex-boot-health.service` fails, the entry is never blessed, and the same automatic rollback happens. `journalctl -u apex-boot-health` names the unit that was not active. |
| you want GRUB back | GRUB was never removed. Select it from the firmware boot menu, then `sudo efibootmgr` (as yourself, deliberately) to put it back at the front of `BootOrder`. Delete `/boot/efi/EFI/Linux/apex-*.efi` to stop offering the UKI path. |
| TPM unlock stops working after a firmware update | the recovery key. Then re-check: with a **signed** PCR 11 policy a firmware update should not break unlock, because the keyslot is bound to the signing key and PCR 11 measures the UKI, not the firmware. If it did break, that is a finding worth recording here. |
| TPM unlock stops working after a kernel update | this should not happen — it is the property the policy was chosen for, and it is measured in the `luks-tpm` scenario. Use the recovery key, then check that the new UKI carries a `.pcrsig` signed by the enrolled key: `python3 files/scripts/boot-v2/pe-section.py <uki> .pcrsig`. |
| you rotated the PCR signing key | every existing keyslot is bound to the old public key. Enroll the new one with `systemd-cryptenroll --tpm2-public-key=<new>` **before** removing the old, and keep the recovery key usable throughout. |
| the TPM was cleared, or the disk moved to another machine | the sealed object is gone: it was bound to that TPM's SRK. Only the recovery key opens the volume. Re-enroll afterwards. |
| you lost the recovery key and the TPM state | the data is gone. This is why enrollment prints the key and this document says to store it off the encrypted disk. |

## What §22 asks for and this does not do

* **Step 6, encryption by default: not implemented, deliberately.** §22 gates it
  on "once recovery and hardware edge cases are proven". What is proven is one
  software TPM in one VM. Real firmware updates, real TPM clears, real
  suspend/resume, and machines with no TPM at all are not covered, and a
  default that fails on any of them costs a user their disk. It stays opt-in.
* **No NVRAM management.** `apex-mkesp` writes `/EFI/BOOT/BOOTX64.EFI` so the
  VM boots by the removable-media path, because creating a real boot entry
  means `efibootmgr` and that is not something a script in this repository does
  near a real machine. On hardware, `bootctl install` creates the entry and the
  operator runs it.
* **`bootctl` is the only systemd-boot tooling in the image, and it only
  reads.** `systemd-boot-unsigned` and `systemd-ukify` are not installed:
  adding them would be a package transaction, which belongs in
  `Containerfile.core`, and a `core` rebuild makes the next fleet update
  multi-gigabyte. Nothing in the shipped boot-counting path needs them —
  `systemd-bless-boot`, `boot-complete.target` and the bless-boot generator
  are all in the `systemd` package the image already has, and
  `Containerfile.base` asserts each one is present rather than assuming it.
* **No composefs work.** §23's row names composefs, and APEX already boots on a
  composefs root — the katana does, today, through GRUB. Nothing here changes
  that, and nothing here needed to.
* **The `apex boot status` entry list needs root**, because the ESP is mode
  0700. Without it the command reports `entries: unavailable` **with the
  reason**, never an empty list: an empty list is indistinguishable from "no
  deployment has failed", which is the answer that would hide a rollback.

## Running the harness yourself

```bash
# on the katana, or any box with /dev/kvm and podman
podman build -t apex-bootlab -f bootlab/Containerfile .
mkdir -p ~/bootlab-work/out
podman run --rm --device /dev/kvm -v ~/bootlab-work:/work:z \
    -v "$PWD:/work/repo:z" apex-bootlab -c \
    '/work/repo/files/scripts/boot-v2/run-scenarios --work /work/out'

# the two scenarios that need a real APEX image are requested by name, and
# hard-fail rather than skipping if the staged root is missing:
sudo files/scripts/boot-v2/apex-stage-root --output ~/bootlab-work/out/apex-root
podman run --rm --device /dev/kvm -v ~/bootlab-work:/work:z \
    -v "$PWD:/work/repo:z" apex-bootlab -c \
    '/work/repo/files/scripts/boot-v2/run-scenarios --work /work/out apex-image luks-tpm'
```

`run-scenarios --list` prints every scenario name. The toolchain-free
assertions — unit conditions, the boot-path tripwire, the health gate's exit
codes, the writer/reader schema parity — are `./tests/test-boot-v2.sh`, which
needs no VM and runs on every pull request in `pr-validation.yml`'s `static`
job. `./tests/test-boot-v2.sh --with-binary` adds the `apex boot status`
fixture cases and runs in the `rust` job.
