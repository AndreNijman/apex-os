# systemd-boot pivot — the blessing defect, and the per-machine mechanism

Unit `sdboot-image`, round 36. This continues
`ROADMAP/evidence/sdboot-image-20260920-lab.md`, which holds the install/boot/
upgrade transcripts. Nothing here touched a real boot path: no `bootc install`
ran this round, no container was launched privileged, and every command below
is either a read of the booted APEX system, a read of the previous round's
saved serial logs, or a `podman build` of an unprivileged container.

## 1. The predecessor's §7 was stale within the hour — the APEX image WAS booted

The evidence file says "The APEX image itself … has not been through this." Its
last commit is 08:17 AWST; `/var/lab-scratch/sdboot-lab/` then grew four 43 GB
APEX installs between 08:24 and 08:57 (`apex.img`, `apexdiag.img`,
`apexfix.img`, `apexfix2.img`) with serial logs for the first three. The APEX
image **does** install and boot on `--composefs-backend --bootloader systemd`:
`apex.serial` reaches `graphical.target` in 46.7 s with
`apex-flatpak-preinstall.service` finishing and the ESP automount mounting and
unmounting cleanly.

Those runs were made to answer one question, and they answered it badly enough
that it is the headline of this file.

## 2. `systemd-bless-boot` cannot rename a loader entry on a FAT ESP

From `apexfix.serial`, APEX image, SELinux **Enforcing**, composefs backend,
systemd-boot, entry hand-renamed to carry a counter:

```
systemd-bless-boot[1609]: Failed to rename
    '/loader/entries/bootc_fedora-43-1+2-1.conf'
 to '/loader/entries/bootc_fedora-43-1.conf': Permission denied
systemd[1]: systemd-bless-boot.service: Main process exited, status=1/FAILURE
audit: type=1400 avc: denied { rename } for pid=1609 comm="systemd-bless-b"
    name="bootc_fedora-43-1+2-1.conf" dev="vda2" ino=139
    scontext=system_u:system_r:init_t:s0
    tcontext=system_u:object_r:dosfs_t:s0 tclass=file permissive=0
```

The same boot's diagnostic unit ruled out every other explanation, in the same
directory, seconds apart:

```
DIAG-enforce: Enforcing
DIAG-mount:   /dev/vda2 /boot vfat rw,nosuid,nodev,noexec,relatime,…,errors=remount-ro
DIAG-context: system_u:object_r:dosfs_t:s0 /boot | … /boot/loader | … /boot/loader/entries
DIAG-manual-rename:
  renamed '…/bootc_fedora-43-1+2-1.conf' -> '…/bootc_fedora-43-1+2-1.conf.TESTMV'
  renamed '…/bootc_fedora-43-1+2-1.conf.TESTMV' -> '…/bootc_fedora-43-1+2-1.conf'
DIAG-bless-rerun: Marked boot as 'good'. (Boot attempt counter is at 1.)
                  rc=0
DIAG-ls-after:    bootc_fedora-43-1.conf
```

So: the filesystem is writable, the directory is writable, the file is
renameable, and **the same binary succeeds when run from a different domain**.
It is SELinux and nothing else. A drop-in adding `Requires=boot.mount` and
`SYSTEMD_LOG_LEVEL=debug` changed nothing, which is the run recorded above.

### Why, from the policy rather than from the message

On the booted APEX system (systemd 258.10-1.fc43, bootc 1.16.10):

```
$ matchpathcon /usr/lib/systemd/systemd-bless-boot
/usr/lib/systemd/systemd-bless-boot    system_u:object_r:init_exec_t:s0
```

`init_exec_t` means PID 1 executing it performs no domain transition: the
process stays `init_t`. Fedora 43's policy has
`systemd_bless_boot_generator_t` for the **generator** and no domain at all for
the worker. Queried out of the live binary policy with `python3-setools`:

| rule | verdict |
| --- | --- |
| `init_t` → `dosfs_t:file` `rename` / `create` / `unlink` / `open` | **DENY** (all four) |
| `init_t` → `dosfs_t:dir` `add_name` / `write` | **DENY** |
| `init_t` → `boot_t:file` `rename` / `create` / `unlink` | **DENY** |
| `init_t` → `boot_t:dir` `add_name` | **DENY** |
| domains allowed `rename` on `dosfs_t:file` | 22, including `bootupd_t` |

The `boot_t` row matters: it rules out the obvious workaround. Mounting the ESP
with `context=system_u:object_r:boot_t:s0` would relabel it and still fail.

On a GRUB machine this never surfaces, because the ESP is written by `bootupd`,
whose domain was built for it:

```
bootupd_t dosfs_t:file = append create getattr ioctl link lock map open read
                         rename setattr unlink watch watch_reads write
bootupd_t dosfs_t:dir  = add_name create getattr ioctl link lock open read
                         remove_name rename reparent rmdir search setattr
                         unlink watch watch_reads write
bootupd_t efivarfs_t:file = getattr ioctl lock open read
```

`efivarfs_t` read is the other half systemd-bless-boot needs, to read
`LoaderBootCountPath`.

### The fix, and the two rules it did NOT need

Sending the blessing into `bootupd_t` with `SELinuxContext=` needs the kernel
to allow `init_t → bootupd_t:process transition` and
`bootupd_t → init_exec_t:file entrypoint`. Expanding attributes (the first
query missed these because the rules are written against the `daemon`
attribute, not the type):

```
init_t -> bootupd_t:process       = … noatsecure rlimitinh siginh transition …   ALLOW
bootupd_t -> init_t:fd            = use                                          ALLOW
bootupd_t -> init_t:unix_stream_socket = connectto …                             ALLOW
bootupd_t -> init_exec_t:file     = map                                          <- only map
```

So exactly one thing is missing, and `files/system/selinux/apex_sdboot.te` is
one `allow` line. Built and installed in a real container build, then read back
out of the **binary** policy:

```
$ podman build … -f coretest/Containerfile .
+ checkmodule -M -m -o apex_sdboot.mod apex_sdboot.te
+ semodule_package -o apex_sdboot.pp -m apex_sdboot.mod
+ semodule -N -i apex_sdboot.pp
+ semodule -l | grep -qx apex_sdboot
+ python3 /usr/share/apex-os/selinux/verify-apex-sdboot.py
apex_sdboot verified in /etc/selinux/targeted/policy/policy.35:
    bootupd_t -> init_exec_t:file ['entrypoint','execute','getattr','map','open','read']
```

**Not yet proven:** the blessing actually succeeding in a booted guest with the
module installed. The policy says it will and the AVC says why it did not; the
boot that closes the loop is gate 3 in `docs/boot-v2.md`.

`checkmodule` refuses a module whose declared name differs from the output
basename, so the file is `apex_sdboot.te` and not `apex-sdboot.te` — the first
build attempt failed exactly there.

## 3. Per-machine configuration: what the shipped systemd actually supports

Read out of the booted APEX image's own man pages, systemd 258.10-1.fc43.

**A per-machine kernel command line is impossible under Secure Boot**, not
merely awkward — `systemd-stub(7)`:

> If UEFI SecureBoot is enabled and the ".cmdline" section is present in the
> executed image, any attempts to override the kernel command line by passing
> one as invocation parameters to the EFI binary are ignored.

**And it does not need to be.** From `sdtest-cfs.serial`, a composefs install's
own kernel line:

```
Command line: initrd=\EFI\Linux\bootc_composefs-1558ee8d…\initrd rw
    console=ttyS0,115200n8 systemd.journald.forward_to_console=1
    composefs=1558ee8d…
```

No `root=`, no per-machine UUID. The only variable token is the composefs
verity digest, which is a property of the image being booted.

**Addons are ruled out for per-machine use** — `systemd-stub(7)` on
`/loader/addons/*.addon.efi`:

> In case Secure Boot is enabled, these files will be validated using keys in
> UEFI DB, Shim's DB or Shim's MOK, and only loaded if the check passes.

APEX's signing key is a CI secret that must never reach a user's machine, so a
machine cannot produce an addon its own firmware will accept.

**Credentials are the mechanism.** `systemd-stub(7)` collects
`<uki>.efi.extra.d/*.cred` into `/.extra/credentials/` and
`/loader/credentials/*.cred` into `/.extra/global_credentials/`, generating a
cpio the initrd sees, measured into **PCR 12**. No signature check.
`systemd.system-credentials(7)` in this systemd lists, among 40-odd:

```
vconsole.keymap, vconsole.keymap_toggle, vconsole.font, …
cryptsetup.passphrase, cryptsetup.tpm2-pin, cryptsetup.fido2-pin, …
system.hostname, system.machine_id, fstab.extra, tmpfiles.extra,
systemd.extra-unit.*, systemd.unit-dropin.*, udev.rules.*, network.*
```

`vconsole.keymap` has been a well-known credential since v253 and is consumed
by `systemd-vconsole-setup.service` — which is L-002's keymap problem solved by
a file the installer writes, with no initramfs regeneration anywhere.

**And bootc already creates the partition type that discovery needs.** From
this round's own install of the APEX image
(`localhost/apex-sdboot:bless2`, `--composefs-backend --bootloader systemd`):

```
Device         Start      End  Sectors Size Type
/dev/loop2p1    2048     4095     2048   1M BIOS boot
/dev/loop2p2    4096  2101247  2097152   1G EFI System
/dev/loop2p3 2101248 90175487 88074240  42G Linux root (x86-64)
```

`Linux root (x86-64)` is the Discoverable Partitions type GUID
`4f68bce3-e8cd-4db1-96e7-fbcaf984b709`. So the contract handed to
`luks-installer` is not asking for something new — it is asking the installer
to keep doing what `bootc install to-disk` already does, on a path where the
installer lays out the table itself. (The 1 MiB BIOS boot partition is visible
here too, on the composefs backend, with nothing written into it.)

**LUKS is discovered, not named** — `systemd-gpt-auto-generator(8)`:

> When systemd is running in the initrd the `/` partition may be encrypted with
> LUKS as well. In this case, a device mapper device is set up under the name
> `/dev/mapper/root`, and a `sysroot.mount` is set up that mounts the device
> under `/sysroot`.

and, when the kernel was booted through `systemd-stub` and reported a
measurement, the volume identifiers and the encryption key are measured into
**PCR 15**. Discovery is by GPT partition **type GUID**, so it needs no karg.

### The standing caveat

A plaintext `.cred` on the ESP is unauthenticated: anyone who can write the ESP
can change it. PCR 12 makes it detectable by a TPM policy; nothing refuses to
boot on its own. Hence the rule written into `docs/boot-v2.md`: non-secret
machine settings plaintext, anything that unlocks the disk through
`systemd-creds encrypt --with-key=tpm2`.

**Not yet proven:** a `.cred` on the ESP changing the keymap at a real LUKS
prompt in a guest. That is gate 2, and it is the measurement `luks-installer`
will want to see before trusting the contract.

## 4. Legacy BIOS, on this machine, today

The predecessor's evidence says the BIOS boot partition exists on the L16 and
was never written to. Re-checked this round, and **half of that is wrong in the
direction that matters**: the partition does not exist on this machine at all.

```
$ python3 -c 'import json;d=json.load(open("/boot/bootupd-state.json"));print(list(d["installed"]))'
['EFI']

$ ls /usr/lib/bootupd/updates/
EFI  BIOS.json  EFI.json

$ lsblk -o NAME,PARTTYPENAME,SIZE /dev/nvme0n1
nvme0n1                           1.9T
├─nvme0n1p1 EFI System            600M
├─nvme0n1p2 Linux extended boot     2G
├─nvme0n1p3 Linux filesystem    396.3G
├─nvme0n1p4 Linux extended boot     2G
└─nvme0n1p5 Linux filesystem      1.5T
```

No `BIOS boot` partition. The reason is in `installer/apex-install`: it has two
modes, and only one of them lets bootc create a partition table.

* **disk mode** runs `bootc install to-disk --wipe`, and bootc lays out the
  table itself — that is where the 1 MiB BIOS boot partition the predecessor
  measured on loopback images comes from, on both backends.
* **partition mode** runs `bootc install to-filesystem` into a root the
  installer made and an ESP that already exists. No table is created, so no
  BIOS boot partition is either. The L16's layout is partition mode.

Neither mode passes `--bootloader` or `--composefs-backend`, so both install
ostree + GRUB today.

`installer/build-live-iso.sh` does build an El Torito core image and an
isohybrid MBR with a VESA `vga=791` handoff, so the **installer media** boots
legacy BIOS. What no evidence supports is an **installed** APEX system booting
it: on whole-disk installs the partition is created and left empty, and on
partition-mode installs like this laptop it is not created at all.

## 5. What this round did NOT do, stated so nobody reads it as done

* No `bootc install` ran. No loopback install, no VM boot, no new disk image.
* The blessing fix is proven at the policy level and not at the boot level.
* Secure Boot: still never measured on the composefs path.
* No sealed UKI has been built from the APEX image.
* No machine was migrated, and none should be until the five gates in
  `docs/boot-v2.md` are closed.

## 6. The blessing, proven in a booted guest — gate 3 is closed

APEX image + `systemd-boot-unsigned` + the `apex_sdboot` module + the drop-in,
installed through `tests/lab/bootc-install-lab` (`--bootloader systemd --
--composefs-backend`), booted twice under OVMF. Both host-side launches went
through `tests/lab/nvram-guard`; all three verdicts were `verified — boot
variables identical before and after`.

The install's own partition table, which also settles the LUKS-discovery
contract and the BIOS question on this backend:

```
/dev/loop2p1    2048     4095     2048   1M BIOS boot
/dev/loop2p2    4096  2101247  2097152   1G EFI System
/dev/loop2p3 2101248 90175487 88074240  42G Linux root (x86-64)
Bootloader: systemd
Installation complete!
```

**Boot 1** — entry renamed by hand to carry a counter, then poweroff:

```
LAB-enforce: Enforcing
LAB-entries: bootc_fedora-43-1.conf
LAB-bcp: ABSENT
LAB-entries-after: bootc_fedora-43-1+3-0.conf
```
Zero AVCs in the whole boot.

**Boot 2** — systemd-boot decremented the counter and the blessing ran:

```
systemd-bless-boot[1210]: Marked boot as 'good'. (Boot attempt counter is at 1.)
systemd[1]: Finished systemd-bless-boot.service - Mark the Current Boot Loader Entry as Good.

LAB-enforce:       Enforcing
LAB-bcp:           LoaderBootCountPath-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f
LAB-bless-ctx:     SELinuxContext=-system_u:system_r:bootupd_t:s0
LAB-bless-result:  active / success
    Drop-In: /usr/lib/systemd/system/systemd-bless-boot.service.d
             └─10-apex-bless-boot-esp.conf
    Process: 1210 ExecStart=… (code=exited, status=0/SUCCESS)
LAB-entries-after: bootc_fedora-43-1.conf      <- the +3-0 suffix is gone
LAB-avc:           <empty>
LAB-health:        active
```

Same image lineage, same backend, same enforcing SELinux as the run that
failed with `Permission denied` and `avc: denied { rename } … init_t →
dosfs_t`. The counter is stripped, the deployment is blessed, and nothing was
denied.

### One caveat that has to be stated, because it changes what is proven

**`bootupd_t` is a permissive domain in Fedora 43** — one of 41, confirmed with
setools (`type.ispermissive`). So denials inside it are logged and allowed. Two
consequences:

* The blessing's zero AVCs are still meaningful, and it is worth being exact
  about why: a permissive domain *logs* what it would have denied, and nothing
  was logged for `systemd-bless-boot`. That the audit path works in this guest
  is not an assumption either — the counter's denials in §7 came through the
  same path in the same boot. So every permission the blessing used was
  genuinely allowed, the entrypoint included, which is what the `apex_sdboot`
  module grants.
  **The in-guest `LAB-runtime-policy:` line came back EMPTY** — the setools
  query against the loaded policy printed nothing and its failure was swallowed
  by the probe's `2>&1 | tail -2`. So the module-is-loaded conclusion rests on
  the absence of an entrypoint AVC, not on a direct read of the kernel's
  policy. That is a sound inference and it is not the direct measurement, and
  the next run should just fix the probe.
* But the drop-in alone would probably work today even without the module,
  because permissiveness would tolerate the missing entrypoint. The module is
  therefore **necessary** for the day Fedora makes `bootupd_t` enforcing;
  whether it is **sufficient** is unmeasured, because `dontaudit` rules hide
  gaps even in a permissive domain. The next guest run should do `semodule -DB`
  before boot 2 so anything dontaudit'd surfaces.

## 7. The counter must NOT be sent into that domain — measured, then reverted

The same guest tested `apex-boot-count` with the symmetric drop-in, by building
a staged set by hand and stopping the unit. It renamed the right file:

```
LAB-staged-before: bootc_lab-43-0.conf bootc_lab-43-1.conf
LAB-count-result:  success
LAB-staged-after:  bootc_lab-43-0.conf bootc_lab-43-1+3-0.conf
```

and it should not be read as a pass, because of what it logged doing it:

```
avc: denied { read open getattr } comm="bash" path="/etc/passwd"
     scontext=system_u:system_r:bootupd_t:s0 tcontext=passwd_file_t permissive=1
avc: denied { read open }        comm="cat"  path="/proc/cmdline"
     scontext=system_u:system_r:bootupd_t:s0 tcontext=proc_t     permissive=1
```

`/proc/cmdline` is where the helper reads the booted deployment's `composefs=`
digest — the whole basis on which it chooses which entry to count. On an
enforcing `bootupd_t` that read fails, the helper takes its own safe path and
logs "no composefs= token … refusing", and the counter is silently never
written. It passed here only because the domain is permissive.

The root cause is one type_transition, queried out of the live policy:

```
type_transition init_t bin_t:process       unconfined_service_t;   EXISTS
type_transition init_t init_exec_t:process …                       NONE
```

`/usr/libexec/apex-boot-count` is `bin_t`, so systemd already runs it in
`unconfined_service_t`, which carries `files_unconfined_type` and renames a
`dosfs_t` file unaided. `/usr/lib/systemd/systemd-bless-boot` is `init_exec_t`,
gets no transition, and stays in `init_t` — which is the entire defect, and why
only the blessing needs the module.

There is independent evidence for the unconfined path from the predecessor's
own diagnostic: `bless-diag.service` ran `/usr/local/bin/bless-diag.sh`, a
`bin_t` script started by systemd on the APEX image with SELinux enforcing, and
its `mv` on the very entry `systemd-bless-boot` could not rename **succeeded**.
That was visible the whole time and was read as "the filesystem is fine" rather
than as "a `bin_t` helper is in a different domain".

So the drop-in and the `bin_t` allow were reverted, the module is back to one
rule, and `Containerfile.base` and `tests/test-boot-v2.sh` now assert that
`apex-boot-count` carries **no** `SELinuxContext=` — the mistake is easier to
re-make than to find.

**Still not proven:** the reverted counter renaming a staged entry in a guest.
The two things it rests on are each measured — the type_transition above, and
`bless-diag.sh`'s successful `mv` — but the exact shipped combination has not
been through a boot. It is the cheap half of the next lab run.
