# Boot v2 — signed UKIs, boot counting, measured boot

Roadmap §22. This is the reference for the systemd-boot + Unified Kernel Image
path: what exists, what it was measured to do, how a machine gets onto it, and
how to get back.

**Andre decided on 2026-09-20 that APEX moves to systemd-boot on every
machine.** The next section is that decision and the design it forces. The rest
of the document — the ESP layout, the measurements, the enrolment procedure,
recovery — is unchanged and is what the decision is built on.

## The pivot to systemd-boot — the decision, and what it costs

**Decision, Andre, 2026-09-20: APEX moves to systemd-boot on every machine.**
Taken with the risks in front of him — it changes how every machine boots, on
an ESP shared with Windows on at least one of them, and there is no rollback
for an ESP something overwrote. This section is the design that has to satisfy
the constraints the old default was protecting, not an argument about the
decision.

Everything below that says *measured* is a command that ran in the VM lab, and
the transcript is in `ROADMAP/evidence/sdboot-image-20260920-lab.md` or
`ROADMAP/evidence/sdboot-image-20260921-decision.md`.

### The pivot is a storage-backend change, not a bootloader flag

This is the load-bearing finding, and it is the real reason GRUB stayed:

```
bootc install to-disk --via-loopback … --bootloader systemd   # ostree backend
error: Installing to disk: bootupd is required for ostree-based installs
```

`bootupd` **is** in the image; it only ships a grub2+shim payload, so on the
ostree backend `--bootloader systemd` has nothing to install and bootc refuses
rather than producing an unbootable disk. Two structural reasons sit behind
that refusal, both visible on the L16: ostree's BLS entries and kernels live on
**btrfs** under `/boot`, which systemd-boot cannot read at all — it reads only
what UEFI's simple-file-system protocol reads, which is FAT; and the ostree
cmdline carries a per-deployment `ostree=` and a per-machine `root=UUID=`,
neither of which can be inside a UKI signed in CI.

`bootc install … --bootloader systemd --composefs-backend` installs, boots and
upgrades. So every APEX machine that boots systemd-boot is a machine whose root
storage is composefs-backed, and **there is no in-place ostree → composefs
converter**: `bootc --help` on 1.16.10 and 1.16.13 lists no migration verb.

### What boots through systemd-boot today, measured

| | state |
| --- | --- |
| `bootc install --bootloader systemd --composefs-backend` | installs, and the guest reaches a login prompt under OVMF |
| `bootc upgrade` on that machine | writes a correct new entry; dedupes the ESP directory when kernel+initramfs are unchanged, writes a second one when the initramfs moves |
| rollback by boot counter | **proven** — at `+0-3` sd-boot selected the previous deployment by itself, with `systemd-bless-boot` masked throughout |
| `systemd-bless-boot` on the APEX image | **fails, enforcing SELinux denial** — see below. Unfixed, every deployment would roll back on its fourth boot |
| Secure Boot on this path | not yet measured; every lab boot was non-SB OVMF |
| a sealed UKI on this path | not yet built; `bootc container ukify` exists and has not been run |
| the ESP an existing APEX machine has | **too small** — 600 MiB on the L16 against ~1.1 GiB needed |

### `systemd-bless-boot` cannot rename an entry on a FAT ESP. Measured.

Without blessing there is no pivot: the counter would run every deployment down
to `+0-N` and roll it back, which is the failure the old default was avoiding.
On the APEX image, installed composefs + systemd-boot, with the entry renamed
to carry a counter, the blessing fails:

```
systemd-bless-boot[1609]: Failed to rename
    '/loader/entries/bootc_fedora-43-1+2-1.conf'
 to '/loader/entries/bootc_fedora-43-1.conf': Permission denied
audit: avc: denied { rename } for comm="systemd-bless-b"
    name="bootc_fedora-43-1+2-1.conf" dev="vda2"
    scontext=system_u:system_r:init_t:s0
    tcontext=system_u:object_r:dosfs_t:s0 tclass=file permissive=0
```

It is not a mount-option or a FAT problem: in the same boot, `mv` on the same
file in the same directory succeeded, and re-running the same binary from a
shell succeeded (`Marked boot as 'good'`, rc=0). The difference is the SELinux
domain, and the cause is structural rather than an APEX mistake:

* `/usr/lib/systemd/systemd-bless-boot` is labelled `init_exec_t`, so when PID 1
  runs it the process stays in `init_t`. Fedora 43's policy has
  `systemd_bless_boot_generator_t` for the *generator* and **no domain at all
  for the worker**.
* Queried out of the live policy with `setools`, `init_t` has no `rename`,
  `create`, `unlink` or `open` on `dosfs_t:file`, and no `add_name`/`write` on
  `dosfs_t:dir`. It has none of them on `boot_t` either, so relabelling the ESP
  with a `context=` mount option would not have helped.
* On a GRUB machine this never comes up, because the ESP is written by
  `bootupd`, whose domain `bootupd_t` **does** have full management of
  `dosfs_t` — that is the domain Fedora built for exactly this job.

APEX's fix is to send the blessing into that domain rather than widen `init_t`:
a drop-in sets `SELinuxContext=-system_u:system_r:bootupd_t:s0` on
`systemd-bless-boot.service`, and a one-rule policy module grants the only
permission the transition is missing — `bootupd_t` may use `init_exec_t` as an
entrypoint. `init_t → bootupd_t:process transition` and `bootupd_t`'s journal
and fd access are already allowed through the `daemon` attribute, checked
rather than assumed.

The `-` prefix covers one family of failure and it is worth naming which.
`systemd.exec(5)`: with the dash, *"failing to set the SELinux security context
will be ignored, but it's still possible that the subsequent execve() may fail
when the security policy doesn't allow the transition."* So a machine with
SELinux disabled or permissive, or a policy with no `bootupd_t`, blesses
unconfined and is fine. A machine that is **enforcing with the module missing**
is not: `setexeccon` succeeds, the kernel denies the `execve`, the unit fails,
and the deployment rolls itself back on the fourth boot. What guards that is
`Containerfile.base` asserting across the tier boundary that `semodule -l`
still lists `apex_sdboot` — the policy module and the drop-in are a pair, and
removing either one alone is the dangerous edit.

### Per-machine configuration under a signed UKI

This is the decision other units build against. Under a sealed UKI the
initramfs is inside the signed PE, so **anything that must differ per machine
can no longer be regenerated locally** — `Containerfile.release` already
asserts the initramfs is baked at build time, and a signature makes that
irreversible rather than merely preferred. L-002's keymap is the concrete case:
a user who cannot type their passphrase on a `us` layout is stranded at the
LUKS prompt, and "regenerate the initramfs at install time" is precisely what a
signed UKI forbids.

**APEX's mechanism is systemd credentials placed on the ESP.** The kernel
command line stays image-static inside the UKI; nothing per-machine goes into
it.

Why, read out of the shipped systemd (258.10) rather than from memory:

* **A per-machine cmdline is impossible, not merely awkward.**
  `systemd-stub(7)`: *"If UEFI SecureBoot is enabled and the `.cmdline` section
  is present in the executed image, any attempts to override the kernel command
  line by passing one as invocation parameters to the EFI binary are ignored."*
  So on a Secure Boot machine the `options` line of a loader entry is dead, and
  a machine-specific cmdline would need a machine-specific signature.
* **The cmdline does not need to be per-machine on this path.** Measured from
  the lab guest's own `/proc/cmdline` on a composefs install:
  `rw <kargs> composefs=<verity>` — no `root=`, no per-machine UUID. The
  composefs digest is a property of the image being booted, so it belongs
  inside the signed PE.
* **UKI addons are ruled out for per-machine use.** `systemd-stub(7)`: addons
  *"will be validated using keys in UEFI DB, Shim's DB or Shim's MOK, and only
  loaded if the check passes"*. APEX's signing key is a CI secret that must
  never reach a user's machine (`AGENTS.md`, and boot-path rule 4), so a
  machine cannot produce an addon its own firmware will accept. Addons remain
  useful for *image-wide* configuration signed in CI — which buys nothing over
  putting it in the UKI.
* **Credentials are not signature-gated, and have consumers already.** The stub
  collects `/loader/credentials/*.cred` and `<uki>.efi.extra.d/*.cred` into
  `/.extra/global_credentials/` and `/.extra/credentials/` in the initrd, and
  measures them into **PCR 12**. `systemd.system-credentials(7)` in this very
  systemd lists `vconsole.keymap`, `vconsole.font`, `cryptsetup.passphrase`,
  `system.hostname`, `fstab.extra`, `systemd.unit-dropin.*`, `udev.rules.*` and
  more as well-known credentials — so the keymap fix is a file the installer
  writes, not a dracut change.

The rules that come with choosing credentials, and they are not optional:

1. **A plaintext `.cred` on the ESP is unauthenticated.** Anyone who can write
   the ESP can change it. It is measured into PCR 12, so a TPM policy bound to
   PCR 12 notices — but nothing refuses to boot on its own.
2. So: **non-secret machine settings** (keymap, console font, hostname) may be
   plaintext. **Anything that unlocks the disk** must be
   `systemd-creds encrypt --with-key=tpm2`, which only this machine's TPM can
   decrypt, and which therefore cannot be lifted off the ESP onto another box.
3. **LUKS is discovered, not named.** With the `options` line dead under Secure
   Boot, `rd.luks.uuid=` and `root=UUID=` are both unavailable. The replacement
   is GPT Discoverable Partitions: `systemd-gpt-auto-generator(8)` in this
   systemd states that in the initrd an encrypted `/` is set up as
   `/dev/mapper/root` with a generated `sysroot.mount`, from the partition
   **type GUID** alone, and measures the volume key into PCR 15 when the kernel
   was booted through `systemd-stub`. gpt-auto stands down if `root=` is on the
   cmdline, which on this path it is not.

**The contract `luks-installer` builds against**, therefore:

* the root partition must carry the x86-64 root type GUID
  `4f68bce3-e8cd-4db1-96e7-fbcaf984b709`, so gpt-auto finds it with no kargs;
* the installer must not add `root=`, `rd.luks.uuid=` or
  `rd.vconsole.keymap=` to the command line — they will be ignored on a Secure
  Boot machine and are a trap on a non-Secure-Boot one, because they work there
  and stop working when Secure Boot is turned on;
* the keymap is written as `vconsole.keymap` in
  `<ESP>/loader/credentials/vconsole.keymap.cred`, plaintext, at install time;
* the TPM2 unlock path stays `apex-luks-enroll`'s; a passphrase handed to the
  initrd as a credential must be TPM2-encrypted.

### How reversible this is for an existing machine — it is not, and here is why

"Additive before destructive" is achievable as a **one-shot test** and is not
achievable as a migration.

*What is possible today* is the enrolment procedure further down this document:
build a UKI from the machine's current deployment, `bootctl install` onto the
existing ESP, add a new `BootXXXX`, and use `BootNext` so the first reboot is a
single test with GRUB untouched and still first in `BootOrder`. That is a real
additive test and it is the right way to see systemd-boot boot this hardware.

*What it is not* is a migration, for three measured reasons:

* **`bootc upgrade` will not maintain it.** On the ostree backend, updates keep
  writing BLS entries under `/boot` on btrfs, which sd-boot cannot read. The
  hand-built UKI is a snapshot of one deployment and goes stale at the next
  update.
* **The ESP does not fit two deployments.** The L16's kernel + initramfs is
  16.9 MB + 375.6 MB = **374 MiB per deployment**; bootc keeps booted and
  rollback, plus a third transiently while an update stages — about **1.1 GiB**.
  The L16's ESP is **600 MiB**, and an ESP cannot be grown in place without
  moving the partition after it. bootc's own composefs default is 1 GiB, which
  is itself under the transient peak, so APEX must ask for more than bootc's
  default. The two unused 2 GiB XBOOTLDR partitions on the L16 cannot absorb
  this: `strings` on the `bootc` binary contains **zero** occurrences of
  `xbootldr` or the XBOOTLDR type GUID.
* **ostree → composefs has no converter.** So switching an existing machine is
  a reinstall with repartitioning, and its reverse is another reinstall.

The honest statement for a user: **migrating an existing APEX machine to
systemd-boot means backing up and reinstalling.** New installs get it from the
installer, where the partition table is being created anyway and the ESP can be
sized correctly the first time.

### Legacy BIOS — what "all machines" costs

systemd-boot is a UEFI application. There is no BIOS systemd-boot, so a machine
that boots legacy BIOS cannot be on this path at all. What APEX actually
supports today, checked rather than assumed:

* `installer/apex-install` has two modes. **Disk mode** runs `bootc install
  to-disk --wipe`, and bootc lays out the table itself — including a **1 MiB
  BIOS boot partition on every install, UEFI or not, on both backends**.
  **Partition mode** runs `bootc install to-filesystem` into a root the
  installer made and an ESP that already exists, so no table is created and no
  BIOS boot partition is either.
* `bootupd` ships a BIOS payload (`/usr/lib/bootupd/updates/BIOS.json`).
* **The L16 has no BIOS boot partition at all** — `lsblk -o PARTTYPENAME` shows
  an EFI System partition, two `Linux extended boot` partitions and two
  filesystems, and nothing else. It was installed in partition mode. Its
  `/boot/bootupd-state.json` records only the `EFI` component, so even where
  the partition does get created, nothing is written into it.
* The **live ISO genuinely boots legacy BIOS**: `installer/build-live-iso.sh`
  builds an El Torito core image and an isohybrid MBR with a VESA `vga=791`
  handoff specifically for BIOS.

So the cost is smaller than it looks and must still be stated plainly: **APEX's
installer media boots legacy BIOS; no APEX installation is known to have ever
booted legacy BIOS from disk**, because on whole-disk installs the bootloader
was never written there and on partition-mode installs the partition does not
exist. The pivot therefore drops a configuration that was created but never
functional, and on this laptop was not even created. If BIOS-from-disk is ever
wanted, it is GRUB on the ostree backend — the two paths cannot be the same
image's default, and that is a product decision, not a build flag.

### Before this is pointed at katana or the L16

In order, and none of them are optional:

1. Secure Boot on the composefs path, in the VM lab, under OVMF with the APEX
   certificate in `db`. Nothing above was measured with Secure Boot on.
2. A sealed UKI built by `bootc container ukify` from the APEX image, booting
   in that guest — including the credential mechanism above actually changing
   the keymap at a LUKS prompt.
3. The blessing fix proven on the APEX image: entry counted, boot, suffix
   stripped, `bootc status` still showing a usable rollback.
4. A Windows-entry assertion: an `efibootmgr -v` capture before and after an
   install into a lab disk that carries a `Windows Boot Manager` entry,
   compared with `cmp`, failing if anything moved. Katana's APEX `Boot0000`
   lives on the **Windows** ESP, so this is the one that protects two operating
   systems rather than one.
5. An ESP sizing decision, since 1 GiB is under the peak APEX needs.

Until all five are done, **no real machine is migrated**, and the L16 is not a
candidate at all — it was unbootable on the morning of 2026-09-20 because a
loopback install saw the host's EFI variables (`BOOT-BREAKAGE-2026-09-20.md`).

### What the image does today

The old default is still what ships, and that is a sequencing statement, not a
contradiction: nothing in the image runs `bootctl install`, `bootctl update`,
`bootupctl`, `grub2-install` or `efibootmgr -c`, and `tests/test-boot-v2.sh`
scans every shipped unit and helper for those commands on **executable** lines
and fails the build if one appears. What the pivot has added to the image so
far is the machinery that has to exist *before* a default can move:
`systemd-boot-unsigned` and `systemd-ukify` in `Containerfile.core`, the
blessing fix, and the boot-counting unit below. Enrolment is still the human
procedure further down.

The two units that implement boot counting carry
`ConditionPathExists=/sys/firmware/efi/efivars/LoaderBootCountPath-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`.
That variable is set only when systemd-boot booted the machine **with a boot
counter in effect**, so on a GRUB machine neither unit starts, and a failed
condition is a skip rather than a failure.

The condition is written against `LoaderBootCountPath` specifically, and that
choice is load-bearing. An earlier note here said the laptop had no `Loader*`
variables at all and only the katana did; re-checked 2026-09-21, **that is
wrong, and wrong in the direction that matters**. The L16 carries `LoaderInfo`,
`LoaderDevicePartUUID` and `LoaderSystemToken` too, and its `LoaderInfo` reads:

```
$ dd if=/sys/firmware/efi/efivars/LoaderInfo-4a67b082-… bs=1 skip=4 status=none | tr -d '\0'
GRUB 2.12
```

GRUB 2.12 sets `LoaderInfo` itself. A condition written against it — the
obvious "is systemd-boot involved" test — would fire these units on **every**
APEX machine, all of which boot GRUB. `LoaderBootCountPath` is set only by
systemd-boot and only when a counter is in effect. Any further conditioned unit
must use the same variable, or `entries.srel` where it cannot (see
`apex-boot-count` below).

### Boot counting has to be written by APEX, because bootc writes none

Four installs and two upgrades produced entry filenames of the form
`bootc_<osid>-<ver>-<N>.conf` with **no `+N-M` counter**, and a `loader.conf`
whose `timeout` and `console-mode` are both commented out. On a machine
installed exactly as bootc leaves it, `LoaderBootCountPath` never appears,
`apex-boot-health.service` and `apex-boot-notice.service` are both skipped by
their condition, `systemd-bless-boot` never runs, and there is no automatic
rollback. The health gate is inert until something writes the counter.

Renaming the live entry does not survive an upgrade: `bootc upgrade` writes a
fresh, uncounted set into `/boot/loader/entries.staged/` and
`bootc-finalize-staged.service` replaces the whole `entries/` directory at
shutdown, discarding anything APEX put in a filename. Renaming inside
`entries.staged/` **does** survive — measured across five boots, ending with
sd-boot selecting the previous deployment by itself at `+0-3`.

So APEX ships `apex-boot-count.service`: it renames the newest staged entry to
carry `+3-0`, and it runs at shutdown ordered so that its work happens **before**
`bootc-finalize-staged.service` swaps the directory in. It is a rename in a
directory bootc is about to install, not a write to a live boot path, and it is
conditioned on `/boot/loader/entries.staged` existing so it is inert on a GRUB
machine and on a machine with nothing staged.

## What exists

| piece | file | what it does |
| --- | --- | --- |
| UKI builder | `files/scripts/boot-v2/apex-mkuki` | kernel + initramfs + signed cmdline + microcode + `.apexinf` metadata in one PE image |
| stage an APEX root | `files/scripts/boot-v2/apex-stage-root` | copies kernel/initramfs/os-release out of a booted deployment or image |
| ESP authoring | `files/scripts/boot-v2/apex-mkesp` | systemd-boot at `/EFI/APEX/`, UKIs at `/EFI/Linux/apex-<id>+N-M.efi` |
| ephemeral keys | `files/scripts/boot-v2/apex-sb-keys` | Secure Boot, PCR-policy and deliberately-untrusted keypairs |
| SB firmware vars | `files/scripts/boot-v2/apex-sb-vars` | an OVMF varstore with the APEX certificate as the only `db` entry |
| LUKS2 + TPM, shipped | `files/system/libexec/apex-luks-enroll` | the enrolment path a machine uses: a recovery key always, and a TPM slot bound to whichever policy this machine can enforce |
| LUKS2 + TPM, lab | `files/scripts/boot-v2/apex-luks-enroll` | signed PCR 11 policy plus a recovery key, against a software TPM. A different program from the row above; see "TPM-bound unlock" |
| VM harness | `files/scripts/boot-v2/run-scenarios` | fifteen scenarios, all booting real guests under Secure Boot enforcing |
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

**Two machines and two dates, because one header covered both for four rounds
and had stopped being true.** The UKI, Secure Boot, reproducibility and
boot-counting figures are from the katana on 2026-09-03 — a real APEX machine,
`VARIANT_ID=gaming`, kernel `7.1.5-cachyos1.fc43.x86_64`. The LUKS and TPM edge
cases below them, from 2026-09-14 onward, ran on the L16 against a root staged
from kernel `7.2.3-cachyos2.fc43.x86_64`, which is what each run's `.apexinf`
line records. Every guest ran under `OVMF_CODE_4M.secboot` with only the
ephemeral APEX certificate enrolled as PK/KEK/db. Nothing here is a
prediction.

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

**TPM clear, firmware change, suspend/resume and a missing TPM** — L-001's
edge cases and L-002's, all against the same software TPM. Counts come from the
container logs with `State.FinishedAt` checked on each, so `Exited (0)` is not
taken at face value.

* **TPM clear — 22 passed, 0 failed.** Four boots, one UKI throughout, so a
  refusal cannot be the PCR policy in disguise. A real `TPM2_Clear` runs between
  boots 1 and 2, confirmed by the owner primary key's *name* changing rather
  than by `tpm2_clear` exiting 0. Boot 2 refuses the TPM unlock, the recovery
  key opens the volume in that same boot, and the plaintext marker written
  before the clear comes back — so the user has their disk and not an opened
  keyslot. It was 20 passed for two rounds: the guest emitted that marker line
  and this scenario read past it without asserting on it.
* **Firmware change — 18 passed, 0 failed, 1 could-not-run.** Fedora's real
  `DBXUpdate` blob moves PCR 7 (76 → 21340 bytes) and the signed PCR 11 policy
  still unlocks, which is why boot-v2 chose this policy. The
  could-not-run is PCR 0: moving it needs a second OVMF build differing **only**
  in code, and the lab image's two 4 MB builds differ in Secure Boot enforcement
  as well, so swapping them would conflate "the firmware changed" with "Secure
  Boot was turned off".
* **Firmware code change — 22 passed, 0 failed, 1 could-not-run.** The
  could-not-run above is now answered. Two Fedora builds of OVMF that differ in
  edk2 revision and in nothing else — `edk2-d46aa46c8361` (20250812) and
  `edk2-2970e5699ba6` (20260812), both Secure Boot, both 4 MB, both booted from
  a fresh copy of one varstore template — move PCR 0 from
  `0FA5AE84ABAB76C8D…` to `0E33803144670784C…`. PCR 11 does not move
  (`22C006FB6217982C0…` throughout), the signed PCR 11 policy still unlocks, and
  a control volume bound **by value** to PCR 0 refuses in that same boot. The
  512-byte marker written under the old firmware is read back under the new one.
  PCR 7 is **identical** across the two revisions (`933DE452FF745C293…`), which
  is worth knowing on its own: an edk2 version change moves the code register
  and leaves the Secure Boot policy register alone.

  The supplied firmware is not taken on trust. Its revision is read out of the
  binary rather than off its filename, and it has to refuse an unsigned UKI
  before anything rests on it — without that, a build with Secure Boot compiled
  out would move PCR 0 for the obvious reason and the run would report that a
  firmware update had not broken the policy, about a firmware that had stopped
  checking signatures. This unit's scratch directory contains that
  trap: a non-Secure-Boot build saved as `OVMF_CODE_4M.secboot.fd`.

  The could-not-run that remains is the honest one: an OVMF binary swap is not
  a vendor UEFI capsule. No microcode, no ACPI tables, no option ROMs and no
  management-engine firmware moved with it.
* **No TPM at all — 16 passed, 0 failed, 1 could-not-run.** L-002's wording is
  that the default must not strand users, and the case with no scenario was the
  machine where nothing answers: a TPM switched off in firmware setup, a board
  replaced under warranty, a disk moved. One volume, booted twice, with the TPM
  device as the only difference. With it, the TPM unlocks and writes the marker.
  Without it the guest reports `tpm-device=absent`, systemd says *"No TPM2
  hardware discovered and EFI firmware does not see it either, falling back to
  traditional unlocking"*, the recovery key opens the volume in that boot, and
  the marker written through the TPM path comes back. The qemu exit status is
  an assertion rather than a note in the log: a guest hanging on a TPM that will
  never answer strands the user the same way as one that refuses with no
  fallback.

  Two things that boot does not show. The guest ran with `headless=1`, which
  turns a passphrase prompt into a refusal — on a real machine the same
  situation shows the user a prompt, and whether that prompt is legible is a
  plymouth question this lab does not touch. And a machine that has **never**
  had a TPM is a different case: it could not carry a `systemd-tpm2` token at
  all, so it never reaches either boot.
* **Suspend/resume — 17 passed, 0 failed, on edk2-20250812.** The volume stays
  open across S3, and a mapper created *after* the resume reads the marker back,
  so the plaintext came off the disk and not out of the page cache of a device
  that had been open since before the sleep. Two non-colluding witnesses: the
  guest's own `/sys/power/suspend_stats`, and qemu asked separately over QMP.
  `s3-mode=deep`, so it was S3 and not suspend-to-idle.

**The shipped OVMF build cannot resume from S3, and it is an edk2 regression.**
On `edk2-ovmf-20260812-4.fc43` the guest suspends and qemu wakes it — the QMP
record has `suspended_seen`, `wakeup_sent` and `resumed_seen` all true, with the
suspend at 107.656 s and the wake at 108.161 s — and then the firmware stops in
its own resume path:

```
SEC: S3 resume
PeiInstallPeiMemory MemoryBegin 0x7FF70000, MemoryLength 0x90000
PopulateMemoryTypeInformation: No Memory Type Information HOB found, S4 resume is likely to fail
ASSERT_EFI_ERROR (Status = Not Found)
ASSERT MemoryServices.c(203): !(((RETURN_STATUS)(Status)) >= 0x8000000000000000ULL)
```

A DEBUG build deadloops on that assert, so the harness kills qemu at the timeout
and the guest never prints anything at all. Changing one variable at a time
places it:

| build | edk2 | Secure Boot + SMM | S3 resume |
| --- | --- | --- | --- |
| in-image `OVMF_CODE_4M.secboot` | 20260812 | on | asserts |
| in-image `OVMF_CODE_4M` (non-secboot) | 20260812 | off | asserts the same way |
| `OVMF_CODE_4M.secboot` from edk2-ovmf-20250812-18.fc43 | 20250812 | on | resumes, 17/0 |

Turning Secure Boot and SMM off changes nothing; going back one edk2 version
fixes it. So the S3 criterion is measurable, on the older firmware, and neither
APEX nor the lab is at fault. `APEX_BOOTLAB_FW` points the harness at a
pre-converted firmware cache, which is how the harness takes the older build.
Each build's version comes out of the binary itself (`strings … | grep edk2-`),
not from its filename.

**A firmware fact worth knowing before debugging anything here.** Fedora's 2 MB
`/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd` does Secure Boot but has **no TCG2
protocol**: sd-stub sets no `StubPcr*` variables, PCR 11 reads as 64 zeros, and
every TPM unlock fails with *"No signature for current PCR policy in TPM2
signature JSON"* — which reads exactly like a broken PCR policy. Only
`OVMF_CODE_4M.secboot.qcow2` measures (367 `Tcg2` lines in the firmware debug
log). `lib.sh` converts the 4 MB pair to raw once and refuses to fall back to
the 2 MB build.

## L-001, criterion by criterion, and the part a VM cannot do

L-001's acceptance is one sentence: *"Real TPM, firmware update, TPM clear,
suspend/resume, recovery key paths tested"*. The lab covers four of those five.
It cannot cover the first from a VM, which is what keeps L-001 `partial` and
L-002 and L-003 blocked behind it.

| criterion | scenario | counts | firmware | what it shows |
| --- | --- | --- | --- | --- |
| **Real TPM** | — | — | — | **nothing. Every row below is `swtpm`.** |
| firmware update, Secure Boot policy (PCR 7) | `luks-firmware-change` | 18 / 0 / 1 | shipped 20260812 | Fedora's real `DBXUpdate` moves PCR 7 (76 → 21340 bytes); the signed PCR 11 policy still unlocks; a by-value PCR 7 control refuses in the same boot |
| firmware update, firmware code (PCR 0) | `luks-firmware-code` | 22 / 0 / 1 | 20250812 → 20260812 | two edk2 revisions differing only in code move PCR 0; PCR 11 and PCR 7 do not; the policy still unlocks; a by-value PCR 0 control refuses in the same boot |
| TPM clear | `luks-tpm-clear` | 22 / 0 | shipped 20260812 | a real `TPM2_Clear` between boots, confirmed by the owner primary key's *name* changing; the TPM then refuses the next boot, **the recovery key opens the volume in it**, and the marker written before the clear reads back |
| suspend/resume | `luks-s3` | 17 / 0 | **20250812 only** | the open volume survives S3 and a mapper created *after* the resume reads the plaintext back; two non-colluding witnesses; `s3-mode=deep` |
| suspend/resume | `luks-s3` | could-not-run | shipped 20260812 | the firmware asserts in its own S3 resume path — an edk2 regression, isolated one variable at a time |
| recovery key paths | `luks-tpm-clear`, `luks-no-tpm` | — | shipped 20260812 | refusal and recovery **in one boot**, twice, and the second reads the marker back, so it is the same disk and not an opened keyslot alone |
| no TPM at all (L-002's wording) | `luks-no-tpm` | 16 / 0 / 1 | shipped 20260812 | the TPM device is gone; systemd refuses the unlock instead of hanging; the recovery key produces the same plaintext |

Read the table with its one caveat in front of it: **an emulated TPM is the
subject of every row.** `swtpm` is a faithful implementation of the TPM 2.0
command set, so everything above is a real statement about APEX's policy, the
LUKS2 header shape and systemd's behaviour. It is not a statement about
anybody's silicon.

### What silicon changes that no VM can substitute

Each of these is a mechanism `swtpm` does not have, not a matter of degree.

* **`Shutdown(STATE)` / `Startup(STATE)` across a real S3.** The TPM has to
  save and restore its volatile state through a power transition it does not
  control. Vendor fTPM bugs in that pair are why suspend/resume is on
  L-001's list at all. `swtpm` keeps its state in a file on a host that never
  slept.
* **A TPM clear issued from firmware setup under physical presence.** The lab
  issues `TPM2_Clear` over an mssim TCP socket from the host. On a machine the firmware
  asserts physical presence, on most boards behind a menu item that also resets
  other platform state, and on some boards that menu is the only way to do it at
  all.
* **PCR 0 from a vendor UEFI capsule.** `luks-firmware-code` swaps one OVMF
  binary for another. A real update ships microcode, ACPI tables, option ROMs
  and often management-engine firmware in one capsule, applied by the firmware
  to itself across a reboot it schedules. Both move PCR 0; only one of them can
  fail halfway.
* **Dictionary-attack lockout.** Real TPMs count failed authorisations and lock
  out, with vendor-set thresholds and recovery times. Nothing above ever
  triggered it. This matters the moment APEX adds a PIN to the unlock path.
* **fTPM and PTT quirks.** AMD's fTPM has a documented stutter; a BIOS update
  can reset an fTPM and take every sealed object with it — which is a TPM clear
  the user did not ask for; Intel PTT is a different implementation again. A
  discrete TPM chip is a fourth case.
* **Machines that do not offer S3.** The guest selected `deep` by name out of
  `/sys/power/mem_sleep`. Many current laptops offer only `s2idle`, and the
  question "does the TPM come back" has a different answer when the platform
  never fully slept. Check the L16 for `deep` before assuming it has S3.

### The run somebody with hardware would have to do

Run these steps rather than reading them. They destroy things: clearing a TPM
loses every sealed object on the machine.

**Not the L16.** Andre's laptop is out of scope for this program: its TPM is not
to be cleared and its boot path is not to be touched.

**Before anything:**

1. **The machine must already boot through a signed UKI.** The enrolment
   procedure below this section is that work. The check is not optional:
   `cat /sys/class/tpm/tpm0/pcr-sha256/11` must not be 64 zeros. On a stock
   APEX install it **is** zeros — measured on the L16 on 2026-09-13 — because
   the shipped image boots GRUB through bootupd and only `sd-stub` extends
   PCR 11. A signed PCR 11 policy on a GRUB machine binds to nothing.
2. **Write the recovery key down, store it somewhere that is not that disk, and
   use it once before going further.** Print it, reboot, decline the TPM unlock,
   type it in. A key nobody has typed is a key nobody has tested.
3. **Know what else on the machine is bound to that TPM.** BitLocker on a dual
   boot, another Linux's LUKS, `systemd-creds` secrets, a TPM-backed SSH agent.
   `systemd-cryptenroll --tpm2-device=list` and
   `tpm2_getcap handles-persistent`, recorded before, is how you know what you
   are about to break. A TPM clear takes all of it.
4. **A live USB you have booted at least once**, and the key sequence for this
   machine's firmware setup menu.

**Run 1 — the baseline.** Reboot and confirm the TPM unlocks with no prompt.
Record: `cat /sys/class/tpm/tpm0/pcr-sha256/{0,7,11}`, and
`cryptsetup luksDump <dev>` showing exactly one `systemd-tpm2` token and one
`systemd-recovery` token.

**Run 2 — TPM clear.** Clear the TPM through the **firmware**, not with
`tpm2_clear` over the resource manager: the point is the path a user takes. On a
board whose `/sys/class/tpm/tpm0/ppi/tcg_operations` answers `5  4: User not
required` that path is `echo 5 > …/ppi/request` plus a reboot, and it needs
nobody at the machine; otherwise it is the firmware setup menu. Reboot. Record
that the TPM unlock is **refused**, that the recovery key opens the volume in
that same boot, and the three PCR values afterwards. Then re-enrol in **two
separate invocations** — wipe the old slot, then enrol — and record that the
`tpm2-blob` in the header changed. The single-command form reports success and
changes nothing; see the Recovery table below.

**Verify the clear; do not infer it from the reboot.** A firmware that silently
declined the request looks identical from the outside, and `ppi/response` is no
help — on the MSI board it reads `5 0: Success` the moment the request is
written, before any reboot. Read TPM state instead: `lockoutAuthSet`,
`tpm2_getcap handles-persistent`, `tpm2_getcap handles-nv-index`,
`TPM2_PT_NV_COUNTERS` and `TPM2_PT_LOCKOUT_COUNTER` all move. `tpmGeneratedEPS`
does **not** — `TPM2_Clear` leaves the endorsement seed alone, so an unchanged
`tpmGeneratedEPS` is not evidence the clear failed.

**Run 3 — a real firmware update.** `fwupdmgr get-updates`, then
`fwupdmgr update`. Record PCR 0 and PCR 7 before and after, and whether the
signed PCR 11 policy still unlocks with no prompt. **This is the run a VM
cannot stand in for**, and the expected result is that unlock still works: the
keyslot is bound to the PCR signing key, and PCR 11 measures the UKI rather than
the firmware. Anything else is a finding worth the whole exercise.

**Run 4 — suspend and resume.** With the volume open:
`cat /sys/power/mem_sleep` (record whether `deep` is even offered),
`cat /sys/power/suspend_stats/success`, `systemctl suspend`, resume, then the
same counter again. Record that the volume still reads. Then close the mapper, reopen it through
the TPM, and record whether that **fresh** unseal works. Those are two
questions, and the second is the one vendor firmware gets wrong.

**Run 5 — the TPM goes away.** Disable the TPM (or fTPM/PTT) in firmware setup.
Record that the boot **prompts for the recovery key rather than hanging**, that
the data is intact, and how long the prompt took to appear. Re-enable it and
record that TPM unlock returns with no re-enrolment — the sealed object is still
bound to the same SRK, which a clear would have destroyed and a disable does
not.

Each run's output belongs in this document, under a heading naming the machine,
its firmware version and its TPM (`tpm2_getcap properties-fixed | grep -i
manufacturer`). Until runs 1, 2 and 5 exist for one machine, nothing
satisfies L-001's word "Real", and L-002 and L-003 stay blocked behind it.

### MSI Katana GF76 12UG · firmware E17L3IMS.110 (AMI, 2025-09-08) · Intel PTT (`INTC`/`ADL`, fw 0x2580012)

2026-09-19. The first entry under that instruction. Full record, every row with
its command and its output: `ROADMAP/evidence/L-001-katana-tpm-20260919.md`.

Runs done on a **loopback LUKS volume**, because this machine's system disk is
not encrypted and converting it was out of scope. Secure Boot is **OFF** here,
so every PCR 7 row is provisional.

| run | verdict |
|---|---|
| 1 — baseline enrol and unlock | **PASS.** One `systemd-tpm2` token, one `systemd-recovery` token. TPM unseal **0.29 s** (×3); recovery-key unlock 0.032 s; the same plaintext back through both |
| 2 — TPM clear | **PASS**, with Andre's explicit approval, 2026-09-19. PPI operation 5 + a reboot, unattended over SSH, 29 s of downtime. Verified five ways rather than assumed: `lockoutAuthSet` 1 → 0, five persistent handles → none, thirteen NV indices → five, four NV counters → zero, DA counter 2 → 0. A volume enrolled **before** the clear then gave `tpm-unlock=REFUSED` → `recovery-unlock=SUCCESS` → marker identical, **in one boot**. Cost, authorised in advance: Andre's Windows Hello PIN. No data — there is no BitLocker |
| 3 — real firmware update | **COULD-NOT-RUN.** `fwupdmgr get-updates` offers nothing for System Firmware on this board |
| 4 — suspend and resume | **PASS.** s2idle, 90 s, RTC wake. Open volume survived and still read; a **fresh** unseal after resume worked in 0.295 s; all 24 PCRs byte-identical across the cycle. `deep` is offered and untested |
| 5 — the TPM goes away | **COULD-NOT-RUN.** PPI op 2 (Disable) is also *"User not required"*, but once firmware hides the TPM the `ppi` directory goes with it, so re-enabling needs somebody in firmware setup. Not symmetric; not attempted |

Runs 1, 2 and 4 are done on this machine. **Run 5 is the only one left that
satisfying L-001's word "Real" still needs**, and it needs a hand on the
hardware — not another agent.

Four more things the lab could not have told us, from the Run 2 session:

5. **`ppi/response` is not a discriminator.** It reads `5 0: Success` the
   instant the request is written, before any reboot. Only TPM state proves a
   clear happened.
6. **The part returns two different codes for "TPM unlock failed", and systemd
   prints one sentence for both.** `0x18b` (`TPM_RC_HANDLE`) means the SRK at
   `0x81000001` is absent and is recoverable — the SRK is deterministic from the
   storage seed, so recreating it restores unlock with no re-enrolment. `0x1df`
   (`TPM_RC_INTEGRITY`) means the seed itself was rolled, i.e. the TPM was
   cleared, and re-enrolment is the only way back. Both surface as
   `Failed to unseal secret using TPM2: State not recoverable`.
7. **`systemd-cryptenroll` provisions the SRK; `systemd-cryptsetup` never
   does.** And `--wipe-slot=tpm2 --tpm2-device=auto`'s *"executing no
   operation"* is a no-op on the LUKS header only — it creates and persists a
   key at `0x81000001`, a handle shared with every other OS on the machine.
8. **A TPM clear moves PCR 1 permanently on this board** — the firmware's own
   log grew by three events on PCR 1, and the new value was identical again on
   the next boot. Which three is unrecoverable: the earlier run kept the
   per-register event count and not the event list. PCR 0, 4 and 7 stayed
   byte-identical across all three boots. Never bind a keyslot to PCR 1.

And one that is not about TPMs at all: **`/dev/nvme0n1` is not a stable name on
this machine.** Three boots were logged; the first two enumerated the two NVMe
controllers one way and the third — an ordinary reboot with nothing special
about it — enumerated them the other way, so the disk that had been Windows's
took APEX's name. The controllers are probed asynchronously and the index falls
out of the race. **Any procedure that protects a disk by device name protects
the wrong one sooner or later** — use the serial, the PCI function, the PARTUUID
or the filesystem label. This is not katana-specific advice; katana is just
where it was caught.

Four things the lab could not have told us, all measured here:

1. **`systemd-cryptenroll --tpm2-device=auto` binds to NO PCRs on systemd
   258.10.** `tpm2-hash-pcrs` empty, `tpm2-policy-hash` 32 zero bytes. Not
   PCR 7 — nothing. Always pass `--tpm2-pcrs=` or `--tpm2-public-key-pcrs=`
   explicitly. Every enrolment call site in this tree does, and the line numbers
   that used to be quoted here are gone on purpose: a citation into a file that
   keeps changing goes stale silently, so the property is asserted instead.
   `Containerfile.base` refuses to build an image in which
   `/usr/libexec/apex-luks-enroll` names a TPM2 device on an executable line
   with no PCR selection beside it, `tests/test-boot-v2.sh` asserts the same
   thing on every pull request with both controls, and the shipped script reads
   the token back after enrolling and refuses one with no policy. Reproduced in
   the boot lab against swtpm on 2026-09-20, so the finding is no longer
   silicon-only: `tpm2-pcrs: []`, **no `tpm2-pcr-bank` field at all**,
   `tpm2-policy-hash` 64 zeros, `New TPM2 token enrolled as key slot 1`, exit 0.
2. **`systemd-pcrextend` is a silent no-op on a GRUB machine.** *"Kernel stub
   did not measure kernel image into PCR 11, skipping userspace measurement,
   too."* — **exit status 0**, PCR 11 unchanged. So the four phase policies
   `systemd-measure sign --current` produces by default can never be reached on
   a machine that has not been through the enrolment procedure below. Enrolling
   a phase policy there yields a volume whose TPM unlock fails at boot, not at
   enrolment.
3. **A zero PCR 11 is a local denial-of-service.** `tpm2_pcrextend 11:…` works
   from plain root, and `tpm2_pcrreset 11` answers `bad locality`. Every
   PCR-11-bound keyslot then refuses until the next reboot. One more reason the
   UKI path matters: after `sd-stub` has extended PCR 11, an attacker can no
   longer choose the value.
4. **The signed PCR 11 policy works on Intel PTT**, which is the mechanism this
   document chose and which had only ever run against `swtpm`. A signature from
   the wrong key is refused by systemd on the fingerprint; a **forged signature
   under the right fingerprint is refused by the part itself**,
   `Esys_VerifySignature … 0x2db` (`TPM_RC_SIGNATURE`). Moving PCR 11 breaks
   unlock; re-signing for the new value restores it.

Two corrections to the bullets above this section, now that a real part has been
asked:

* **Dictionary-attack lockout is no longer untriggered.** Intel PTT here:
  `MAX_AUTH_FAIL` 32, `LOCKOUT_INTERVAL` 7200 s, `LOCKOUT_RECOVERY` 86400 s, and
  **a successful authorisation does not clear the counter** — only the 2-hour
  decay or `TPM2_DictionaryAttackLockReset`, which needs `lockoutAuth`. If APEX
  ever ships `--tpm2-with-pin`, a user who mistypes 32 times over any span of
  time locks the TPM for up to a day and APEX cannot reset it.
* **`deep` being offered is not the same as `deep` working.** This machine lists
  `[s2idle] deep` and suspends as s2idle. The TPM came back from s2idle intact.
  S3 remains untested on silicon.

One thing to add to the prerequisites, learned the hard way: **check
`/sys/class/tpm/tpm0/ppi/tcg_operations` before trusting that a TPM clear needs
physical presence.** Operation 5 reading `4: User not required` means any root
process can schedule a clear that the firmware performs unattended at the next
boot — a data-loss primitive reachable from a shell on a machine with a
TPM-bound volume.

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

# Every host-side launch of the boot lab goes through nvram-guard. It reads
# `efibootmgr -v` and the efivarfs Boot* digests before and after and fails the
# run if this machine's boot entries moved. It cannot prevent a write; it turns
# "found at the next power-on" into "found when the command returns", which is
# the difference between a five-minute fix and an unbootable laptop
# (BOOT-BREAKAGE-2026-09-20.md). AGENTS.md boot-path rule 6. The check belongs
# HERE and not inside run-scenarios, because inside the container there are no
# host EFI variables to read.
./tests/lab/nvram-guard -- \
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

# ── 3. check the ESP has room, BEFORE writing anything to it ──
#   A UKI is the kernel plus the initramfs: 374 MiB on the L16 today. Know the
#   number for this machine and the free space you have, and stop here if it
#   does not fit. This is a ONE-deployment test, not a migration — two APEX
#   deployments plus a staged third need about 1.1 GiB of ESP.
du -b "/usr/lib/modules/$(uname -r)"/{vmlinuz,initramfs.img}
df -h /boot/efi

# ── 4. install systemd-boot and place the UKI ──
#   `bootctl install` writes the ESP and adds an NVRAM entry. Nothing in this
#   repository runs it for you.
#   Capture the boot entries first: this is what you compare against, and it is
#   what proves the Windows entry on a dual-boot machine was not touched.
sudo efibootmgr -v | tee ~/efibootmgr.before
sudo bootctl install
sudo mkdir -p /boot/efi/EFI/Linux
#   +3-0 is the boot counter: three tries, none used.
sudo cp ~/bootlab-work/apex-<deployment>.efi \
        /boot/efi/EFI/Linux/apex-<deployment>+3-0.efi
sudo bootctl list                   # the entry must appear, with 3 tries left

#   Per-machine settings go beside the UKI as credentials, because a signed
#   UKI's command line cannot carry them. A keymap, for instance — the setting
#   that decides whether you can type your own LUKS passphrase:
#   The directory does not exist on a GRUB machine's ESP; create it first.
sudo mkdir -p /boot/efi/loader/credentials
printf 'de' | sudo tee /boot/efi/loader/credentials/vconsole.keymap.cred
#   Plaintext is right for a keymap and WRONG for anything that unlocks the
#   disk: a .cred on the ESP is unauthenticated and anyone who can write the
#   ESP can change it. It is measured into PCR 12, so a TPM policy notices, but
#   nothing refuses to boot on its own. Secrets go through
#   `systemd-creds encrypt --with-key=tpm2`.

# ── 5. compare the boot entries. Additive means ADDITIVE. ──
sudo efibootmgr -v | tee ~/efibootmgr.after
#   The existing entries must be byte-identical; only a new one may appear.
diff <(grep -v '^BootOrder:' ~/efibootmgr.before) \
     <(grep -v '^BootOrder:' ~/efibootmgr.after)
#   On a machine that also boots Windows, this is the line that matters:
cmp <(grep -i 'Windows Boot Manager' ~/efibootmgr.before) \
    <(grep -i 'Windows Boot Manager' ~/efibootmgr.after) \
  && echo "Windows entry unchanged"
#   If the Windows entry moved, STOP and restore it from ~/efibootmgr.before
#   before rebooting.

# ── 6. make the first reboot a TEST, not a commitment ──
#   BootNext applies to exactly one boot and the firmware clears it. GRUB stays
#   first in BootOrder, so if systemd-boot does not come up, the boot after
#   that is the machine you had this morning.
sudo efibootmgr | grep -i 'Linux Boot Manager'     # note its BootXXXX number
sudo efibootmgr --bootnext XXXX
sudo systemctl reboot
#   after it comes up:
apex boot status
#   expect: Bootloader systemd-boot · Signed UKI yes · Boot counting in effect
#           and the entry listed as "good" — systemd-bless-boot stripped the
#           +N-M suffix because apex-boot-health.service passed.
```

If it does **not** come up: the boot counter is doing its job. Three failed
attempts and systemd-boot selects the previous entry by itself. GRUB is still
installed and still first in the firmware's boot order, and `BootNext` is
already spent, so the next power-on is the machine you had before.

**This is a test and not a migration, and the difference is not pedantry.** The
UKI you just placed is a snapshot of one deployment. `bootc upgrade` on this
machine keeps writing ostree BLS entries under `/boot`, which systemd-boot
cannot read, so the UKI goes stale at the next update and nothing refreshes it.
Getting a machine onto systemd-boot *permanently* means installing it onto the
composefs backend, which means reinstalling — see the pivot section at the top.

### TPM-bound unlock

There are **two scripts called `apex-luks-enroll`** and they are different
programs. Getting them confused is the one mistake this section exists to stop.

| | `files/scripts/boot-v2/apex-luks-enroll` | `/usr/libexec/apex-luks-enroll` |
| --- | --- | --- |
| who runs it | the boot lab | the installer, or you |
| what it targets | an image file it creates itself | a LUKS2 volume that already exists |
| what TPM | **only** a software TPM at a named swtpm state directory; `auto` and any `/dev` path are refused outright | the real one |
| the binding | always a signed PCR 11 policy | whichever binding this machine can enforce — it asks |
| in the image | no | yes, shipped by `Containerfile.base` |

On a real machine:

```bash
sudo /usr/libexec/apex-luks-enroll \
     --device /dev/<your-luks-partition> \
     --recovery-out /root/apex-recovery-key.txt
```

It enrols a recovery key first, before anything else, on every path. A volume
that has a TPM binding and no recovery path turns a firmware update into a
data-loss event, so the script never opens that window. Move the key file off
the encrypted disk before you reboot, not after.

It adds a TPM key slot only where this machine can enforce one, and when it
declines it tells you why in words you can act on:

| what it finds | what you get | the machine-readable line |
| --- | --- | --- |
| Secure Boot on, TPM present, no sd-stub | a TPM slot bound to **PCR 7** | `tpm2: enrolled binding=pcr7 pcrs=7 bank=sha256 hash=… pin=no` |
| Secure Boot on, booted through sd-stub, PCR 11 really extended | a TPM slot bound to the **signed PCR 11** policy, which survives kernel updates | `tpm2: enrolled binding=signed-pcr11 …` |
| Secure Boot **off** | recovery key only | `tpm2: declined reason=secure-boot-off` |
| Secure Boot reports on, but the firmware is in Setup Mode | recovery key only | `tpm2: declined reason=setup-mode` |
| no TPM device | recovery key only | `tpm2: declined reason=no-tpm2-device` |
| Secure Boot on but PCR 7 never extended | recovery key only, and the slot it made is removed again | `tpm2: declined reason=pcr-uninitialised` |

**It exits 0 in every one of those rows.** A declined TPM slot still leaves you
a volume with a recovery key on it, so an installer must not stop there. Only a
failure to enrol the recovery key exits non-zero.

**Why no TPM slot without Secure Boot, for either binding.** PCR 7 records the
Secure Boot policy, and with Secure Boot off it records the *disabled* policy
whichever kernel boots — so an attacker boots their own system and the TPM
unseals for them. The signed PCR 11 policy is no better off, and that is the half people miss:
the signature it needs ships in the UKI's `.pcrsig` section and is public, PCR 11 starts at zero on every boot, `tpm2_pcrextend 11`
works from plain root, and every digest that reaches the signed value is a hash
of public material. An attacker who can boot any kernel replays the extends and
unseals without APEX's private key. What stops them is Secure Boot refusing to
load their kernel. If Secure Boot cannot be enabled on a machine, `--with-pin`
is the other route — read what it prints about the lockout counter first.

**Never `--tpm2-device=auto` with no PCR selection.** Measured on systemd
258.10: the bare form exits 0, prints "New TPM2 token enrolled", and writes a
token with `tpm2-pcrs: []`, no `tpm2-pcr-bank` field and a policy hash of 32
zero bytes — sealed to the TPM storage key and nothing else, so any OS on that
hardware unseals it. To check a volume somebody else enrolled:

```bash
sudo /usr/libexec/apex-luks-enroll --check --device /dev/<partition>
```

which exits 2 and says what is wrong if the TPM slot carries no policy.

On the UKI path the UKI must be built with `--pcr-key` so it carries the
matching `.pcrsig`. A UKI without one is refused by the policy, which is the
designed behaviour and not a fault.

## Recovery

| situation | what to do |
| --- | --- |
| the new deployment will not boot | do nothing for three attempts; systemd-boot selects the previous blessed entry itself. `apex boot status` then shows the failed entry as `OUT OF TRIES` and announces the rollback. |
| the machine boots but the desktop does not | `apex-boot-health.service` fails, the entry is never blessed, and the same automatic rollback happens. `journalctl -u apex-boot-health` names the unit that was not active. |
| you want GRUB back | GRUB was never removed. Select it from the firmware boot menu, then `sudo efibootmgr` (as yourself, deliberately) to put it back at the front of `BootOrder`. Delete `/boot/efi/EFI/Linux/apex-*.efi` to stop offering the UKI path. |
| TPM unlock stops working after a firmware update | the recovery key. Then re-check: with a **signed** PCR 11 policy a firmware update should not break unlock, because the keyslot is bound to the signing key and PCR 11 measures the UKI, not the firmware. The lab has now measured both halves of a firmware change and neither breaks it — the Secure Boot policy register (`luks-firmware-change`, a real `DBXUpdate`) and the firmware code register (`luks-firmware-code`, two edk2 revisions). Still nobody has tried it on silicon with a vendor capsule: katana was asked on 2026-09-19 and `fwupdmgr get-updates` offers nothing for its System Firmware, so there was no capsule to apply. What katana *did* show is the half that matters — re-signing for a moved PCR 11 restores unlock on real Intel PTT. If a vendor capsule ever breaks it, that is a finding worth recording here. |
| TPM unlock stops working after a kernel update | this should not happen — it is the property the policy was chosen for, and it is measured in the `luks-tpm` scenario. Use the recovery key, then check that the new UKI carries a `.pcrsig` signed by the enrolled key: `python3 files/scripts/boot-v2/pe-section.py <uki> .pcrsig`. |
| you rotated the PCR signing key | every existing keyslot is bound to the old public key. Enroll the new one with `systemd-cryptenroll --tpm2-public-key=<new>` **before** removing the old, and keep the recovery key usable throughout. |
| the TPM was cleared, or the disk moved to another machine | the sealed object is gone: it was bound to that TPM's SRK. Only the recovery key opens the volume. Re-enroll afterwards **in two separate invocations** — wipe the old slot first, then enroll. Doing both in one command reports success and changes nothing; see below. |
| the TPM is switched off in firmware setup, or the board was replaced | the recovery key, and the volume opens with the data intact — measured as `luks-no-tpm`. The sealed object is still in the header, so re-enabling the same TPM restores unlock with no re-enrolment; a *cleared* TPM is the row above and does need one. In a lab guest the refusal is immediate; on a machine the boot shows a passphrase prompt instead, and nobody has checked how legible that prompt is. |
| TPM unlock refuses with `Esys_Load … 0x18b` and the TPM was **not** cleared | the persistent SRK at `0x81000001` is missing, not the key material. `systemd-cryptsetup` only *uses* that handle; `systemd-cryptenroll` is what creates it, and the SRK is derived deterministically from the storage primary seed — so recreating it restores unlock with **no re-enrolment** and no change to the header. Measured on katana 2026-09-19 by evicting the handle and putting it back. From the initrd there is no way to do this, so the recovery key is the only way into that boot. |
| TPM unlock refuses and nothing about the machine changed | check PCR 11 before blaming the firmware. It can be extended from plain root and **cannot be reset** (`tpm2_pcrreset 11` → `bad locality`), so anything with root can deny every PCR-11-bound unlock until the next reboot. Measured on katana 2026-09-19. A reboot restores it; the recovery key works throughout. |
| you lost the recovery key and the TPM state | the data is gone. This is why enrollment prints the key and this document says to store it off the encrypted disk. |

**The one-command re-enrolment after a TPM clear is a no-op that exits 0.**
Measured on systemd 258.10-1.fc43, and it is the command a user reaches for
after a firmware TPM clear:

```
$ systemd-cryptenroll --wipe-slot=tpm2 --tpm2-device=… --tpm2-public-key=… VOLUME
This PCR set is already enrolled, executing no operation.
$ echo $?
0
```

The header is byte-identical afterwards. systemd de-duplicates against the token
already in the header — it compares the public key and the PCR set, and the TPM
plays no part, and that check runs **before** it acts on the wipe. So on a
cleared TPM the whole invocation does nothing while reporting success: the
header keeps a blob sealed to a seed that no longer exists, and the next boot
fails exactly as it did before the "recovery" (`Esys_Load rc 0x1df`, *"Key
enrolled in superblock most likely does not belong to this TPM"*). Wiping in its
own invocation first produces a new blob and a working unlock.

**Confirmed on silicon after a real firmware clear**, 2026-09-19, Intel PTT:
`This PCR set is already enrolled, executing no operation.`, `rc=0`, `tpm2-blob`
sha256 unchanged, and the next unlock still refused — with `0x1df` rather than
the `0x18b` the first post-clear attempt gave, because the invocation had
meanwhile persisted a fresh SRK at `0x81000001`. Two separate invocations then
changed the blob and unlock worked in 298 ms. So the paragraph above is not a
lab artefact: it is what a user gets.

The `luks-tpm-clear` scenario missed this at first, and why is worth keeping:
every header assertion it already made — exactly one `systemd-tpm2` token, the
public-key policy, PCR 11 — held true of the no-op character for character. It
now compares the sealed **blob** before and after, and an unchanged blob is a
named failure.

## What §22 asks for and this does not do

* **Step 6, encryption by default: not implemented, deliberately.** §22 gates
  it on "once recovery and hardware edge cases are proven". A TPM clear, a PCR 7
  change, a PCR 0 change, an S3 cycle and a missing TPM are now each measured.
  **Two of the five show a recovery key opening a volume the same boot had
  refused: the TPM clear, and the missing TPM.** The two firmware scenarios show
  a refusal and no recovery, because their control volumes carry no recovery key
  on purpose — a fallback would hide the refusal that is the entire point of an
  instrument. The S3 scenario shows
  neither; it never touches a recovery key, and what it checks is that the
  volume stays open across the suspend.

  An earlier version of this paragraph said the PCR 7-bound control volume was
  refused and then opened with the recovery key. **That was wrong and is
  withdrawn.** The control has no recovery key by construction, the guest probe
  has no recovery path for it, and no `recovery-unlock` line appears in any
  serial log from any firmware-change boot. Checked against the logs rather
  than against the source, because the source is what made the claim plausible.

  And every one of the five ran against `swtpm` in a VM. No silicon TPM has been
  through an enrol-and-recover cycle. The installer cannot encrypt a disk in the
  first place — every `crypto_LUKS` branch in `installer/apex-install` is a
  refusal to overwrite an existing header, not a path that creates one. A
  default that fails on any of this costs a user their disk. It stays opt-in.
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

Every launch below is wrapped in `tests/lab/nvram-guard`, and that is not
decoration. The check has to live on the HOST side: `run-scenarios` runs inside
the container, where `/sys/firmware/efi/efivars` does not exist, so a check in
there would inspect nothing. AGENTS.md boot-path rule 6. A `bootc install
--via-loopback` goes through `tests/lab/bootc-install-lab` instead, which
builds the podman argv itself and cannot be talked out of the efivars mask.

```bash
# on the katana, or any box with /dev/kvm and podman
podman build -t apex-bootlab -f bootlab/Containerfile .
mkdir -p ~/bootlab-work/out
./tests/lab/nvram-guard --label bootlab -- \
podman run --rm --device /dev/kvm -v ~/bootlab-work:/work:z \
    -v "$PWD:/work/repo:z" apex-bootlab -c \
    '/work/repo/files/scripts/boot-v2/run-scenarios --work /work/out'

# the two scenarios that need a real APEX image are requested by name, and
# hard-fail rather than skipping if the staged root is missing:
sudo files/scripts/boot-v2/apex-stage-root --output ~/bootlab-work/out/apex-root
./tests/lab/nvram-guard --label bootlab-apex -- \
podman run --rm --device /dev/kvm -v ~/bootlab-work:/work:z \
    -v "$PWD:/work/repo:z" apex-bootlab -c \
    '/work/repo/files/scripts/boot-v2/run-scenarios --work /work/out apex-image luks-tpm'
```

`run-scenarios --list` prints every scenario name, and
`tests/test-boot-v2.sh` fails if a scenario exists that `--list` does not name:
a default run skips an unregistered scenario and reports green without
it.

One scenario takes an input the lab image cannot produce. `luks-firmware-code`
needs a **second** Secure Boot OVMF build, a different edk2 revision from the
image's, because moving PCR 0 needs two firmwares differing only in code:

```bash
# the older Fedora build, extracted from its rpm — any different revision works
./tests/lab/nvram-guard --label bootlab-fw-alt -- \
podman run --rm --device /dev/kvm -v ~/bootlab-work:/work:z \
    -v "$PWD:/work/repo:z" -e APEX_BOOTLAB_FW_ALT=/work/OVMF_CODE_4M.secboot.qcow2 \
    apex-bootlab -c \
    '/work/repo/files/scripts/boot-v2/run-scenarios --work /work/out luks-firmware-code'
```

Without it the scenario records a could-not-run naming the package to fetch and
the sha256 of a known-good image, and asserts nothing. With a firmware that is
the same revision, or that does not enforce Secure Boot, it fails rather than
reporting a result: both are checked before anything rests on the input. The toolchain-free
assertions — unit conditions, the boot-path tripwire, the health gate's exit
codes, the writer/reader schema parity — are `./tests/test-boot-v2.sh`, which
needs no VM and runs on every pull request in `pr-validation.yml`'s `static`
job. `./tests/test-boot-v2.sh --with-binary` adds the `apex boot status`
fixture cases and runs in the `rust` job.
