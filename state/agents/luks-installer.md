# luks-installer — L-002, "Enable LUKS2 by default"

Branch `task/luks-installer` in **apex-os**, worktree
`/var/tmp/apex-work/wt-luks-installer`. Merged `origin/roadmap/v2.2` @
`b5f696cb` cleanly on 2026-09-20. Not landed. Do not commit to `roadmap/v2.2`.

## READ THIS FIRST — the guard in tests/lab does not prevent what it says

`tests/lab/bootc-install-lab` inserts `--tmpfs /sys/firmware/efi/efivars` and
calls that the primary guard. **It is not one.** Measured here on 2026-09-20 at
22:12: a loopback `bootc install to-filesystem` ran with that mask applied and
logged, and still executed

```
efibootmgr -b 0000 -B
efibootmgr --create --disk /dev/loop1 --part 1 --loader \EFI\fedora\shimx64.efi
```

against this laptop — the 2026-09-20 boot breakage, reproduced four hours after
its guard landed. `bootc` needs `--pid=host` and **re-enters the host's mount
namespace** for the bootloader step, so the container's mounts are irrelevant.
Checked, not deduced, twice: an unmasked privileged container here shows ZERO
entries under `/sys/firmware/efi/efivars`, and `strings /usr/bin/bootc` carries
`nsenter`, `/proc/1/ns/mnt` and `/proc/1/root`.

The prevention is `bootc install --generic-image` ("Changes to the system
firmware will be skipped"). `apex-install` passes it on all four `bootc
install` call sites when the target is loop-backed. **`bootc-install-lab` still
does not**, and its argv is built for the caller, so every lab loopback install
still depends on `nvram-guard` catching the write after it happens. That is
`efivars-guard`'s to fix and is the single most important thing in this card.

The host was repaired the same session from the guard's own before-snapshot
(`efibootmgr -b 0000 -B`, then `--create --disk /dev/nvme0n1 --part 1 --loader
'\EFI\fedora\shimx64.efi' --label 'APEX-OS'`). `efibootmgr -v` and the efivarfs
sha256 of Boot0000/Boot0004/BootOrder are byte-identical to the pre-run
snapshot. The machine was never rebooted broken.

## What this unit changed

| file | what |
|------|------|
| `installer/apex-install` | `encrypt=yes` builds ESP + plain ext4 `/boot` + LUKS2→btrfs, installs with `bootc install to-filesystem`, calls `/usr/libexec/apex-luks-enroll`, proves BOTH keys open the volume before installing, surfaces the recovery key, writes `/etc/crypttab`; **NEW:** `--generic-image` + efivars tmpfs for loop-backed targets; an UNLOCK keymap resolved separately from the console keymap; `loader/credentials/vconsole.keymap.cred` written to the ESP |
| `files/dracut/apex-unlock-hint/apex-vconsole-credential{,.service}` | **NEW.** Applies a `vconsole.keymap` system credential to the initramfs's own `/etc/vconsole.conf` before `systemd-vconsole-setup`, and stands down if the kernel command line already decided. This is the UKI-era keymap channel |
| `installer/test-installer-keymap-boot.sh` + `keymap-boot-drive.py` | **NEW, not CI.** Five real guests, shipped initramfs, real LUKS2 volume, passphrase typed on an emulated keyboard through QMP. 12 passed / 0 failed |
| `installer/test-installer-luks.sh` | 57 passed / 0 failed on the L16, and `keymap-checks.sh` re-run inside `quay.io/fedora/fedora:43` — the route CI takes — is 32 / 0 with the identical 547/15/11 sweep. NVRAM section (5 assertions + 2 mutants) and, through `keymap-checks.sh`, 14 typeability assertions + 2 mutants |
| `installer/test-installer-luks-live.sh` | now runs the engine inside `tests/lab/nvram-guard` and asserts its verdict; asserts the ESP credential and that bootupd never ran `efibootmgr` |
| `Containerfile.apex` | asserts both shim halves are in the shipped initramfs, naming `sysinit.target.wants` |
| `docs/disk-encryption.md` | the UKI seam is measured now, not predicted |

## The three things a stranger most needs to know

1. **The keymap property is measured as a user experiences it.**
   `installer/test-installer-keymap-boot.sh` boots the *shipped* initramfs and
   types the physical key positions `a p e x y e d 1` through QMP `send-key`.
   On `de` the `y` key produces `z`, the passphrase `apexzed1` is right, and
   the volume unlocks; on `us` the identical keystrokes are refused. Five
   boots, and runs 3 and 5 are what make run 4 mean anything.
2. **A systemd credential on the ESP is INERT on its own.**
   `systemd-vconsole-setup(8)`: vconsole.conf and the kernel command line take
   precedence over the credential, and dracut bakes `/etc/vconsole.conf`
   (`KEYMAP=us`) into the initramfs. Boot 3 shows the credential arriving in
   `/run/credentials/@system` and losing. `apex-vconsole-credential` is what
   makes it count, and it must be pulled in by **`sysinit.target.wants`** —
   `initrd.target.wants`, where this module's other unit lives, runs far too
   late.
3. **15 of the 562 shipped keymaps cannot type an ASCII passphrase.**
   `hr-unicode` and four relatives have no `q w x y` anywhere; `vn`,
   `kz-latin`, `cm-azerty` have no `1`; `it-geo`, `ge-ergonomic`, `fa` are
   missing much of the alphabet. The engine resolves an UNLOCK keymap, falls
   back to `us` for that one prompt, and names the characters that forced it.
   `ru` is NOT in that list — the first measurement said it was, because the
   regex was anchored to column 0 and never saw the indented `keycode` lines.

## NEXT

1. **Finish the live install.** `installer/test-installer-luks-live.sh` run 6
   was started at 2026-09-20 ~23:05 with the `--generic-image` fix in place;
   read `/var/lab-scratch/luks-installer-msgs/r2/live-run6.log` first and look
   for exactly three things: nvram-guard's verdict must be `verified`;
   `grep 'Executing: "efibootmgr"'` over that log and the engine log must find
   nothing; and `sudo efibootmgr -v` must still name PARTUUID `1c417de2-…`,
   `0x12c000`. If those three hold, the `--generic-image` prevention is proven
   whatever else the run reports. Run 5
   died at the NVRAM guard, so **no encrypted install has completed end to end
   since the `/proc` fix for `useradd`**. Needs `sudo -n`, root podman,
   `localhost/apex-os:daily`, ~25 GB on `/var/lab-scratch` — never `/tmp`.
   **Check `sudo efibootmgr -v` names PARTUUID `1c417de2-…` before and after.**
2. **Boot a disk this installer produced.** Nothing has. The keymap boot test
   uses a bare volume and a direct kernel boot: no firmware, no bootloader, no
   ostree pivot. The recipe is one OVMF boot in the `apex-bootlab` container
   against the live suite's `target.img` (`OVMF_CODE_4M.secboot.qcow2`, QMP
   `send-key` the passphrase, watch serial for the pivot to `/sysroot`).
3. **Ask `luks-enroll` for the hop this unit could not measure.** Boots 3 and 4
   deliver the credential over **SMBIOS type 11**, which proves
   systemd-vconsole-setup honours a *system credential*. It does NOT prove
   sd-boot reads `\loader\credentials\*.cred` off the ESP and passes it
   through sd-stub. Their `run-scenarios` has the whole signed-UKI chain: put
   the `.cred` on the lab ESP, no karg, type `y`, expect `z` to unlock.
4. **Tell `sdboot-image`** the installer's requirement is exactly one string,
   `vconsole.keymap=<name>`, reaching the initrd per machine without
   re-signing, and that the implementation assumed here is the ESP credential
   plus `apex-vconsole-credential`. If they choose UKI `.cmdline` addons
   instead, the installer side is one function and the shim becomes dead weight
   rather than wrong.
5. **The unlock-keymap fallback is announced too late to be useful.** The
   engine decides it during validation and `note`s it, which the GUI shows on
   the PROGRESS page, after the confirm step. A Croatian owner whose
   passphrase contains an `x` should be told on the **encrypt page**, while
   they can still choose a different one. That needs the conversion and the
   typeability check on the GUI side, or a one-shot `--check-passphrase` mode
   in the engine the page can call.
6. **`localectl set-keymap` after install updates neither the karg nor the
   `.cred`.** `/etc/vconsole.conf`'s comment tells the user to change both; no
   tool does it. That is a real second stranding path and nothing covers it.
7. **`tests/lab/bootc-install-lab` needs `--generic-image`** — see the top of
   this card. Not this unit's file.

## Traps this unit paid for

* `/tmp` is a 15 GB **tmpfs**. Use `/var/lab-scratch`.
* `grep` with a pattern containing `[` or `]` is a character class. Three
  keymap-boot runs that had plainly succeeded were reported as failures until
  `serial_has` grew `-F`.
* An apostrophe inside a single-quoted awk program ends the program. The X
  keysym table had one.
* A keycode pattern anchored to `^` misses the xkb maps that write one plane
  per line with the modifiers in front (`shift keycode 2 = …`).
* Dockerfile comment lines inside a `RUN … \` continuation are stripped by the
  parser, so they must NOT end with a backslash.
* `bash`'s `printf '%b' '\x61'` did not produce `a` here; it dropped the `\x`.
  The characters are built inside awk with `printf "%c"` instead.
