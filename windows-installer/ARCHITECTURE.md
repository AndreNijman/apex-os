# How a Windows program installs APEX, and why it is not the obvious way

`bootc install` is Linux software. It opens block devices, runs `mkfs`, writes
an ostree or composefs store through a Linux kernel's filesystem drivers, and
relabels files with the target's SELinux policy. None of that has a Windows
implementation and none of it is going to get one. So "install APEX from a
Windows app" cannot mean "run the normal installer", and the first thing this
directory has to do is say what it means instead.

Three routes were on the table. This is which one was taken, and the measured
facts that killed the other two — because the facts, not the preference, are
what a stranger needs in order to disagree with this later.

---

## The decision

**Route 2: stage a payload, and complete the installation on the first boot.**

The Windows program does four things and then stops:

1. it verifies and takes exclusive ownership of one empty partition the user
   chose,
2. it writes an **APEX bootstrap environment** into that partition — a kernel,
   an initramfs and an answers file, not an operating system,
3. it adds a small, self-contained boot directory to the shared Windows ESP,
   and
4. it appends one firmware boot entry to the **end** of `BootOrder`, leaving
   Windows the default.

The first time that entry is selected, APEX's own live environment comes up and
runs `installer/apex-install` in `mode=partition`, against the same partition,
on Linux, as a normal APEX installation. The real installer formats the target,
runs `bootc install to-filesystem`, creates the account, relabels it with the
target policy and queues the MOK enrolment. The Windows program never imitates
any of that.

The division of labour is the point: **Windows moves bytes; Linux installs the
operating system.** Every step Windows performs is a file copy or a firmware
variable, both of which Windows genuinely can do, and both of which are exactly
reversible.

---

## Why not Route 1 — "write a prepared filesystem image to the partition"

This is the tempting one. Build the root filesystem in CI, ship it as an image,
have Windows write it to the chosen partition, add a boot entry, done. Three
measured facts stop it.

### 1. A prepared root image does not carry the half that has to boot

APEX is pivoting to systemd-boot with bootc's composefs backend
(`docs/boot-v2.md:12`, Andre's decision of 2026-09-20). On that backend `/boot`
**is** the FAT ESP — the kernel, the initramfs and the loader entry all live
there, not on the root filesystem:

> a loose `vmlinuz` and `initrd` under `/EFI/Linux/bootc_composefs-<verity>/`,
> a `.conf` in `/loader/entries/`, and the kernel command line on that file's
> `options` line — `docs/boot-v2.md:292-295`

So on the path APEX is moving to, "write the root image" installs nothing that
can start. The bootable half is a separate transaction into a partition shared
with Windows, and see the ESP-size section below for why that transaction does
not fit.

### 2. The per-machine configuration is not optional, and it is Linux-only

`installer/apex-install` does not finish when `bootc install` returns. It then
runs, inside the new deployment:

- `useradd --root "$deploy" -m -G wheel -s "$ushell"` (`installer/apex-install:1142`),
- `chroot "$deploy" /usr/sbin/setfiles -F "$spec" …` with the **target's** policy
  (`:1161`), because the live environment runs `selinux=0` and every file it
  creates is otherwise unlabelled (`:502`),
- `mokutil --import` to queue Secure Boot enrolment (`:2535`).

The relabel is not cosmetic. The installer treats its absence as fatal and says
why:

> cannot SELinux-relabel the new account files … Without the relabel, login
> would be denied. The OS is installed on $DISK but the account is not usable.
> — `installer/apex-install:1154`

A Windows program cannot run `setfiles`, cannot run `useradd --root`, and
cannot write a MOK request into the deployment. So **a first boot on Linux is
mandatory no matter which route is chosen.** Once that is true, Route 1's only
remaining advantage over Route 2 — "no second stage" — does not exist.

### 3. A generic image cannot be its own boot reference

On the composefs backend the ESP directory is named by the image's verity
digest (`/EFI/Linux/bootc_composefs-<verity>/`). That name is a property of the
image bootc produced, computed at install time. An image prepared elsewhere and
copied in by a program that cannot compute it is an image whose boot entry has
to be guessed.

---

## The ESP is the constraint that shapes everything

The rule for this tool is that the ESP is shared with Windows, is never
reformatted, and is only ever added to. That makes the ESP's **free space** a
hard budget, and the two APEX boot paths want wildly different amounts of it.

| path | what lands on the ESP | size |
| --- | --- | --- |
| ostree + GRUB (what `apex-install` installs today) | `EFI/BOOT/` + `EFI/fedora/` — shim, grub, mm, CSV, `grub.cfg`, `bootuuid.cfg`; kernels and BLS entries stay on the root filesystem | **≈ 7.47 MiB**, measured in `docs/m0-results.md:766-776` |
| systemd-boot + composefs (where APEX is going) | the whole of `/boot` — `vmlinuz` + `initrd` per deployment, sd-boot, loader entries | **374 MiB per deployment**, ≈ **1.1 GiB** for booted + rollback + a staging third — `docs/boot-v2.md:239-242` |

A stock Windows ESP is **100 MB**. That is not a guess: it is what Windows Setup
creates, and `lab/autounattend.xml` deliberately asks for exactly that so the
lab measures the real constraint rather than a comfortable one.

### The number, measured rather than quoted

Taken from inside a real Windows Server 2022 guest in `windows-installer/lab/`,
by mounting the ESP and asking Windows how much of it is left:

```
esp-total-bytes: 100663296     96.0 MiB
esp-used-bytes :  29087744     27.7 MiB   EFI/Microsoft/Boot + EFI/Boot
esp-free-bytes :  71575552     68.3 MiB
```

Almost all of the 27.7 MiB Windows uses is `EFI/Microsoft/Boot` — `bootmgfw.efi`
plus 33 language directories of `.mui` files and 16 boot fonts. That is the
floor on a clean install with nothing else on the machine; a real laptop with a
vendor diagnostic partition entry or a second Linux will have less.

So, against the two paths:

| path | needs | fits in 68.3 MiB? |
| --- | --- | --- |
| ostree + GRUB | 7.47 MiB | **yes**, with 60 MiB to spare |
| systemd-boot + composefs | ~1.1 GiB | **no**, short by a factor of 16 |

One deployment alone on the composefs path is 374 MiB — still five times the
whole free space. There is no version of this that fits.

`docs/boot-v2.md:58` already records that even an **APEX** machine's ESP is too
small for the composefs path — 600 MiB on the L16 against the ~1.1 GiB needed.
A 100 MB Windows ESP is not close.

### The consequence, stated plainly

**A Windows-side APEX install cannot use the systemd-boot + composefs path into
a shared Windows ESP.** Not "should not" — the boot files do not fit, by an
order of magnitude, and an ESP cannot be grown in place without moving the
partition after it (`docs/boot-v2.md:241`).

So this tool targets the **ostree + GRUB** backend, whose ESP cost is 7.47 MiB
and which fits inside what Windows leaves over. `installer/apex-install` passes
neither `--bootloader` nor `--composefs-backend` today, so that is also what it
already produces — the Windows path and the Linux path install the same thing.

This is a real tension with the systemd-boot decision and it is not this unit's
to resolve. Written down here so the boot units see it: **either dual-boot
installs started from Windows stay on the ostree/GRUB backend, or a
Windows-side installer has to create a second, larger ESP — which is a GPT
modification, and everything else in this design exists to avoid one.** The
lab measures the actual free bytes in a real Windows ESP so that conversation
starts from a number.

---

## Why not Route 3 — "run the installer under WSL / a shipped hypervisor"

Rejected without much argument, for the record:

- WSL2 has no raw block-device access to the physical disk the user chose, and
  installing WSL or Hyper-V is a system-wide change to the user's Windows
  machine that an installer has no business making. `bootc install` also wants
  a privileged container runtime; that is a second install.
- Shipping a hypervisor turns a "portable exe" into a driver install.
- Neither removes the mandatory Linux first boot from §2 above. They add a
  second Linux environment and keep the one that was already needed.

---

## What the Windows side writes, precisely

### Into the chosen partition

A FAT32 filesystem containing the APEX bootstrap:

```
/apex/vmlinuz              the APEX kernel
/apex/initramfs.img        the APEX live initramfs
/apex/answers              the install answers the GUI collected
/apex/stage.json           the transaction journal's guest-visible half
```

FAT32 rather than ext4 because the loader has to read it before Linux exists,
and because the Windows side can write it without shipping an ext4
implementation. It is a *staging* filesystem, not the future root: the first
boot's `apex-install` formats this partition as btrfs and installs into it, so
everything above is read into memory by the loader and is gone afterwards by
design.

The payload is a live environment, not an operating system image. The OS itself
is fetched by `apex-install`'s existing netinstall path, which already carries
its own guards — default route, DNS, and a ≥ 22 GB non-tmpfs scratch check. A
400 MB bootstrap that pulls a verified image beats a 5 GB payload that has to
survive being copied into RAM before its own partition is reformatted.

### Into the shared Windows ESP

One new directory, created with create-new semantics, containing only files
that did not exist before:

```
/EFI/APEX-<transaction-id>/    the loader and its configuration
```

Nothing is written to `EFI/Microsoft`, `EFI/BOOT` or the Windows BCD. No
existing file is opened for writing, even if its hash matches what would have
been written. Every pre-existing path's hash is recorded before the transaction
and re-checked after it.

### Into the firmware

One new `Boot####` variable and one appended entry at the **end** of
`BootOrder`. No `BootNext`, no reordering, no replacing an earlier APEX entry.
Windows stays the default boot option, and the assertion for that is a
before/after dump of the firmware variables compared outside the guest — not an
intention written in a comment.

---

## What is undone by "undo", and what is not

Reversible, because it was additive:

- the `Boot####` variable and its position in `BootOrder`, removed only when
  the current `BootOrder` still matches what the journal recorded,
- the `EFI/APEX-<transaction-id>/` directory, removed only when every file in
  it still hashes to what the journal recorded.

Not reversible, and the user is told so before it happens:

- the contents of the chosen partition. It was verified empty first, every
  byte of it, which is why "empty" is checked rather than assumed — but zeroes
  that get overwritten do not come back.

Firmware writes and FAT writes are not atomic with each other. That is a
release blocker with a name: it needs fault-injection tests at every write
boundary, in VM firmware, and it is not something an Undo button solves. The
transaction journal exists so that an interrupted run can be *diagnosed*; it
does not make the interruption safe.

---

## What this means for the code

The priority order the work follows, and why it is that order:

1. **Enumeration** — Windows' own view of the disks, cross-checked against the
   existing raw-bytes GPT parser. Two independent sources that must agree.
2. **Selection and confirmation** — by size, label, GPT name, disk model and
   serial. **Never by device index.** NVMe enumeration on Andre's own hardware
   has reordered across three ordinary reboots; a tool that says "Disk 1" is a
   tool that erases the wrong disk on the fourth.
3. **Ownership and locking** — refuse anything Windows has mounted or is using,
   and name the drive letter in the refusal.
4. **Payload deployment** — through the locked volume handle, which confines
   the writes to the extent by construction rather than by arithmetic.
5. **The bootloader transaction** — additive, journalled, reversible.
6. **Undo.**

Steps 4 and 5 are the ones that change a machine. Neither may run against
anything but a virtual disk until the review gates in `README.md` are closed.
