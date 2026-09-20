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

```
$ python3 -c 'import json;d=json.load(open("/boot/bootupd-state.json"));print(list(d["installed"]))'
['EFI']
```

The 1 MiB BIOS boot partition `bootc install` creates on every install has
never been written to on the L16. `bootupd` ships `BIOS.json` alongside
`EFI.json`, and `installer/build-live-iso.sh` really does build an El Torito
core image and an isohybrid MBR — so the **installer media** boots legacy BIOS
and an **installed APEX system on this machine does not have a BIOS
bootloader**.

## 5. What this round did NOT do, stated so nobody reads it as done

* No `bootc install` ran. No loopback install, no VM boot, no new disk image.
* The blessing fix is proven at the policy level and not at the boot level.
* Secure Boot: still never measured on the composefs path.
* No sealed UKI has been built from the APEX image.
* No machine was migrated, and none should be until the five gates in
  `docs/boot-v2.md` are closed.
