# Disk encryption in the APEX-OS installer

This is the installer's half of L-002, "Enable LUKS2 by default". The other
half — what is enrolled into the LUKS2 header, and when a TPM keyslot is safe —
is `/usr/libexec/apex-luks-enroll` and `docs/boot-v2.md`.

The acceptance line for the item is *"encryption default does not strand users
across qualified hardware and recovery cases"*, so this document is organised
around the ways a person can be locked out, and what stops each one.

---

## What happens when encryption is on

`installer/apex-install` is handed `encrypt=yes` and a `lukspass=` in its
answers file, and builds the disk itself instead of letting `bootc install
to-disk` do it:

| part | size | type | filesystem | mounted at |
|------|------|------|-----------|------------|
| p1 | 1 GiB | `ef00` EFI System | FAT32 | `/boot/efi` |
| p2 | 1 GiB | `8300` Linux filesystem | ext4, **not encrypted** | `/boot` |
| p3 | rest | `8309` Linux LUKS | LUKS2 → btrfs | `/` |

Then `bootc install to-filesystem` installs into the opened volume, with the
`/boot` partition already mounted so bootc picks up its UUID.

**Why `/boot` is outside the encryption.** APEX boots GRUB through bootupd.
GRUB can be talked into reading some LUKS2 headers, but not all, and the
failure mode of getting that wrong is a machine that does not boot at all
rather than one that asks for a passphrase. A separate plain `/boot` is what
Fedora ships for encrypted installs and what this repository's own runbook
specified (`docs/m4-install-runbook.md` §5b, `docs/m0-results.md` §9). The
kernel and initramfs living in the clear leak nothing: they are the same public
bytes as the published image, and their integrity is Secure Boot's job.

**Why the partition type is `8309` and not the discoverable root GUID.** The
initramfs carries `systemd-gpt-auto-generator`, which acts on the discoverable
*root* type. Typing the LUKS partition as generic "Linux LUKS" keeps exactly
one thing responsible for opening it: the `rd.luks.*` kernel arguments below.

**Why not `bootc install to-disk --block-setup tpm2-luks`.** It exists, and it
would have been three words instead of a branch. Read what it does: it binds
the unlock to *"presence of the default tpm2 device"* — no passphrase slot, no
recovery slot — and on systemd 258 `--tpm2-device=auto` binds to **no PCRs at
all** (measured on real hardware; L-002 round 31). That is an encrypted disk
with one key, held by a chip that a firmware update, a TPM clear or a
motherboard swap takes away. It is the stranding case, spelled as a flag.
bootc's own help points at `to-filesystem` for LUKS.

---

## The keys, and the order they are created in

1. `cryptsetup luksFormat --type luks2 --pbkdf argon2id`, passphrase on stdin.
   Never on a command line: argv is world-readable in `/proc`.
2. `/usr/libexec/apex-luks-enroll --device <dev> --recovery-out <file>`, run out
   of the **image being installed**, not the live environment — enrolment policy
   belongs to the system it applies to. The existing passphrase is offered both
   as `$PASSWORD` (systemd-cryptenroll's own variable) and on stdin.
3. The installer then **proves both keys open the volume** with `cryptsetup
   open --test-passphrase`, before the ten-minute `bootc install` starts. A
   recovery key that was printed but does not work is worse than none, and
   finding that out afterwards means finding it out on the user's machine.
4. Whether a TPM keyslot exists is **observed in the header**
   (`cryptsetup luksDump` → `systemd-tpm2` token), never taken on trust. Only
   then is `rd.luks.options=<uuid>=tpm2-device=auto` added.

If the helper is missing from the image, fails, produces no recovery key, or
produces one that does not open the volume, the install **refuses** — with the
disk carrying no data, so nothing is locked away. An encrypted disk whose only
key is one passphrase is one forgotten password away from being a brick.

---

## The recovery key: where it goes and how to use it

It reaches the user by two routes, because each fails differently.

1. **On screen.** The engine emits `APEX-INSTALL-RECOVERY-KEY: <key>` on
   stdout. The graphical installer captures that line, keeps it out of the
   scrolling install log, and shows it on the final page in large monospace —
   with a checkbox that must be ticked before **Reboot now** is enabled. A user
   who reboots past it never sees it again: the installer's RAM disk goes with
   the reboot.
2. **On the USB stick they booted from**, as a 0600
   `apex-recovery-key-<hostname>.txt` on the live ISO's own ESP. The file says,
   in as many words, that a stick in the same bag as the laptop protects
   nothing, and to move the key elsewhere and delete it.

It is **never** written to the target's ESP or `/boot`. Those live on the
machine that is encrypted, and an attacker holding the laptop has them.
It is **never** passed through `note()` or `log()`: the install log is copied
onto the stick in the clear on every failure.

**Why the recovery key survives a wrong keyboard.** systemd's recovery key is
written in a modhex alphabet whose characters sit in the same place on QWERTY,
QWERTZ and AZERTY. So it is also the answer to "the console keymap is wrong" —
it can be typed on a machine whose layout is not the owner's.

---

## The keyboard, which is the other way to be locked out

The passphrase prompt is a **kernel console prompt**. It has one keymap, no
input method, no compose key and no way back. Two separate defects made that
prompt lie about which keyboard it was using.

**1. `/etc/vconsole.conf` was given an XKB layout name.** `loadkeys` does not
speak XKB. Measured against the keymap tree the image ships: of the 99 layouts
`xkeyboard-config` offers, **36 are not loadable console keymap names**.
`loadkeys` then fails and the console silently keeps the kernel's built-in map,
which is `us`. The installer now converts through systemd's own
`/usr/share/systemd/kbd-model-map`, preferring a keymap file of that exact name
when one exists (which is the order systemd itself uses), and validating the
table's answer against the keymap tree before writing it — the table names a
`ge` keymap that Fedora 43's kbd does not ship. After conversion, **0 of 99**
resolve to something `loadkeys` cannot load.

**2. Nothing put the choice where the initramfs could read it.** The initramfs
is built **in the image, at build time, `--no-hostonly`**, so it cannot contain
this machine's `/etc/vconsole.conf` and is identical on every machine. The one
per-machine channel that exists today is the kernel command line:
`systemd-vconsole-setup`, which runs inside the initrd, parses
`vconsole.keymap=` from `/proc/cmdline` (verified by reading the strings of the
binary the image ships, and by the `i18n` dracut module delegating to it rather
than installing its own cmdline hook). So an encrypted install now gets

```
vconsole.keymap=<console keymap>   rd.vconsole.keymap=<console keymap>
```

added, and **only when the keymap is not `us`** — an ordinary US install gains
no argument that could later override `localectl set-keymap`.

On an encrypted machine the kernel argument is therefore the source of truth
for the console keymap, and `/etc/vconsole.conf` says so in a comment. That is
deliberate: changing only the file would leave the *unlock* prompt on the old
layout, which is the lockout this exists to prevent.

### The seam for systemd-boot and UKIs

A UKI carries its command line inside the signed PE, and `systemd-stub` ignores
a command line handed to it by the boot loader when Secure Boot is on. So the
`--karg` route above stops being per-machine the moment APEX pivots to
sd-boot + UKIs.

The replacement that needs no re-signing is a **system credential**:
`systemd-vconsole-setup` reads `vconsole.keymap` as a credential, and both
`systemd-boot` (`\loader\credentials\*.cred`) and `systemd-stub`
(`\EFI\Linux\<uki>.efi.extra.d\*.cred`) pass credentials from the ESP into the
initrd. A **UKI addon** contributing a `.cmdline` section is the other
candidate. Whichever the boot-path work settles on, the installer's side is one
function — everything upstream of it (XKB → console keymap) is unchanged, and
what has to be handed to the machine is a single string.

---

## What is refused, and why

Every one of these fires **before anything is written**, and says so.

| case | what happens |
|------|--------------|
| `encrypt=` missing from the answers file | refused. Not "defaults to no" — a dropped key would silently produce an unencrypted machine — and not "defaults to yes", which would encrypt CI disks. |
| `encrypt=yes` with `mode=partition` | refused: one partition has nowhere to put an unencrypted `/boot`. The user is told to choose whole-disk or turn encryption off. |
| a passphrase with a character outside printable ASCII | refused: the boot prompt has no accents, no emoji and no input method. The **login** password is not restricted. |
| a passphrase shorter than 8 characters | refused. |
| `encrypt=yes` with a non-btrfs root | refused: an untested combination is not something to discover on somebody's only disk. |
| `cryptsetup`, `sgdisk`, `mkfs.*` or dm-crypt missing | refused, naming what is missing and offering the unencrypted route. |
| the image has no `/usr/libexec/apex-luks-enroll` | refused, naming the path. |
| the target already carries a `crypto_LUKS` header (partition mode) | refused, exactly as before. That guard is unchanged. |
| unattended (`apex.unattended`, test builds only) with `apex.encrypt=yes` | refused: that path has nobody to show a recovery key to. |

---

## Recovering a machine you cannot get into

1. **The passphrase is refused and you are sure it is right.** The keyboard is
   probably not the one you think. Type the **recovery key** instead, at the
   same prompt — its letters are in the same place on nearly every layout.
2. **You have lost the passphrase.** The recovery key. It is on the USB stick
   you installed from, unless you deleted it, and it was on the last screen of
   the installer.
3. **Automatic (TPM) unlocking stopped working** after a firmware update, a TPM
   clear or a motherboard swap. The passphrase still works; if it does not, the
   recovery key does. `docs/boot-v2.md` has the measured behaviour of each of
   those cases.
4. **You have neither key.** There is no fourth option, and there is not
   supposed to be. That is what the acknowledgement checkbox on the last
   installer screen is for.

Once inside, `sudo systemd-cryptenroll --recovery-key /dev/<partition>` makes a
new recovery key, and `cryptsetup luksChangeKey` changes the passphrase.

---

## What this does not do yet

* **No TPM enrolment is exercised by the installer's own tests.** The installer
  calls the helper and checks what came back; whether a TPM keyslot is safe is
  `apex-luks-enroll`'s decision and `docs/boot-v2.md`'s evidence.
* **Nothing has booted from an encrypted disk built this way.** The install is
  tested end to end on a loopback file — both keys open the volume, the
  bootloader entry carries the right UUIDs and keymap — but a VM boot that
  types a passphrase at the real prompt belongs to
  `files/scripts/boot-v2/run-scenarios`, and is requested there.
* **In-place encryption of an existing install is not offered**, and should not
  be: `cryptsetup reencrypt` on a live root is the single most dangerous thing
  this installer could learn to do.
