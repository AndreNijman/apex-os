# M0 results

Findings and evidence from the M0 spikes. Each spike proves one mechanic of the
APEX-OS build/boot chain before it is committed to in later milestones.

---

## Spike E — dual-boot `to-filesystem` rehearsal (shared ESP)

**Date:** 2026-07-21   **Host:** Void Linux, qemu 11.0.2 + KVM, OVMF (edk2 x64,
4 MB split), podman 5.8.3   **Image:** `quay.io/fedora/fedora-bootc:43`
(bootc **1.16.3**)   **Scripts:** [`files/scripts/spike-e/`](../files/scripts/spike-e/)
(reusable install template)   **Artifacts:** `~/apex-os-m0-work/spike-e/` (VM
disks + `out/` captures, not in repo).

### Question

Does `bootc install to-filesystem`, run against a **shared ESP** that already
carries an incumbent OS's bootloader files, **clobber, reorder, or preserve**
that OS's EFI boot entries, `BootOrder`, and ESP files? This de-risks the real
installs on the **ThinkPad L16** (Void, EFISTUB entries + kernels *on* the ESP)
and **MSI Katana** (Windows).

### Method (rehearsal harness)

A 30 GB GPT VM disk was built with a layout mirroring the L16:

| Part | Size | FS | Role |
|------|------|----|------|
| `vda1` | 1 GiB | FAT32 | **shared ESP** |
| `vda2` | 12 GiB | ext4 | incumbent root (Alpine) |
| `vda3` | ~17 GiB | btrfs (empty) | APEX-OS install target |

The **incumbent is Alpine 3.24.1** booting **classic EFISTUB** exactly like
Void on the L16: kernel + initramfs copied *onto* the shared ESP at
`\EFI\alpine\` and an `efibootmgr` entry whose `LoadOptions` carry the cmdline
(`initrd=\EFI\alpine\initramfs-lts root=UUID=… …`). Using a **non-Fedora
incumbent in its own ESP vendor dir** (`/EFI/alpine`, not `/EFI/fedora`) is
deliberate — it is the only way to tell "bootc wiped the whole ESP" apart from
"bootc overwrote its own vendor dir".

The install was run **from inside the VM** (podman in the incumbent) — the only
safe place to let bootc's `efibootmgr` touch NVRAM, since that NVRAM belongs to
the VM's OVMF (`OVMF_VARS` pflash), never the host. The host's ESP / efibootmgr
/ bootloader were never touched. NVRAM persists across VM reboots, so
`BootOrder` survival is directly observable.

Choreography (host-driven state machine over a serial log + a shared payload
disk carrying the image tar): **setup** (create the incumbent's NVRAM entry) →
**before** (capture) → **install** → **after** (capture) → boot-test the bootc
OS → boot-test the incumbent. See `99-run-all.sh`.

### The `bootc install` invocation that worked

```sh
# target prepared first: vda3 (empty btrfs) -> /target ; vda1 (ESP) -> /target/boot/efi
podman run --rm --privileged --pid=host --network=host \
    -v /dev:/dev \
    -v /run/udev:/run/udev \
    -v /var/lib/containers:/var/lib/containers \
    -v /target:/target \
    quay.io/fedora/fedora-bootc:43 \
    bootc install to-filesystem \
      --karg=root=UUID=<apex-root-uuid> --karg=rw \
      --karg=console=tty0 --karg=console=ttyS0,115200 \
      --skip-fetch-check \
      /target
```

`--replace` was **not** passed (target root is empty). The default mode is what
preserves the foreign ESP content — see gotchas. Result:

```
Installing image: docker://quay.io/fedora/fedora-bootc:43
Initializing ostree layout
Deploying container image...done (9 seconds)
Bootloader: grub
Installing bootloader via bootupd
Executing: "efibootmgr" "--create" "--disk" "/dev/vda" "--part" "1" \
           "--loader" "\EFI\fedora\shimx64.efi" "--label" "Fedora"
Installation complete!
```

The apex root received a correct bootc/ostree deployment
(`.bootc-aleph.json`, `ostree/deploy/default/deploy`, `/boot` with
`grub2/`, `loader/`, `bootupd-state.json`).

### `efibootmgr` before / after

**Before** (`BootCurrent: 0002`):
```
BootOrder: 0002,0000,0001,0003,0004,0005,0006,0007
Boot0002* Alpine (incumbent)  HD(1,GPT,…)/\EFI\alpine\vmlinuz-lts  [+cmdline in LoadOptions]
```

**After** (bootc's own ordering, before any orchestration):
```
BootOrder: 0008,0002,0000,0001,0003,0004,0005,0006,0007
Boot0002* Alpine (incumbent)  HD(1,GPT,…)/\EFI\alpine\vmlinuz-lts   <- PRESERVED, byte-identical
Boot0008* Fedora              HD(1,GPT,…)/\EFI\fedora\shimx64.efi   <- NEW (added by bootc)
```

**Diff:**
- `Boot0002` (incumbent): **untouched** — same device path, same LoadOptions/cmdline.
- All other pre-existing entries (`0000,0001,0003–0007`): **untouched**.
- `Boot0008 "Fedora"`: **added** via `efibootmgr --create` (label from
  `/EFI/fedora/BOOTX64.CSV`).
- `BootOrder`: **reordered** — bootc's `--create` **prepends**, so `0008`
  (Fedora) becomes the default and the incumbent `0002` is demoted from 1st to
  2nd. **Nothing was deleted or renumbered.**

### ESP contents + space delta

| | Before | After |
|---|---|---|
| `/EFI/alpine/vmlinuz-lts` | 14,468,096 B | 14,468,096 B (identical) |
| `/EFI/alpine/initramfs-lts` | 20,876,653 B | 20,876,653 B (identical) |
| `/EFI/BOOT/` (BOOTX64.EFI + fbx64.efi) | — | 1,037,240 B (added) |
| `/EFI/fedora/` (shim, grub, mm, CSV, grub.cfg, bootuuid.cfg) | — | 6,794,292 B (added) |
| **ESP total used** | **35,344,749 B (~33.7 MiB)** | **43,176,281 B (~41.2 MiB)** |

**bootc ESP delta ≈ 7.47 MiB (7,831,532 B)** — the `/EFI/BOOT` + `/EFI/fedora`
trees. The incumbent's `/EFI/alpine` kernels were **not touched**. (The L16 ESP
already carries Void kernels/initramfs; budget ~8–10 MiB headroom for bootc.)

### Pass / fail per sub-goal

| Sub-goal | Result |
|---|---|
| Incumbent boots (before) | **PASS** — OVMF loaded `Boot0002` → `\EFI\alpine\vmlinuz-lts` → Alpine. |
| `to-filesystem` completes | **PASS** — rc=0, primary attempt (no `--disable-selinux`). |
| bootc OS boots after | **PASS** — shim→grub→ostree kernel→dracut→**switch-root** into the ostree root; second systemd reached `basic`/`network` targets. |
| Incumbent boots after | **PASS** — normal boot loaded `Boot0002` → Alpine (kernel + os-release + cmdline confirmed from inside). |
| Existing entries/BootOrder preserved | **PASS (with caveat)** — entries & ESP files preserved byte-for-byte; `BootOrder` **reordered** (bootc prepended itself as default). |
| ESP delta measured | **PASS** — +7.47 MiB. |

### Gotchas / findings

1. **bootupd only manages `/EFI/BOOT` + its own vendor dir (`/EFI/fedora`).** It
   did **not** wipe the ESP. The upstream warning that "bootc install is always
   destructive with respect to `/boot` and the ESP" (PR #1752 / docs) did **not**
   materialise for `to-filesystem` in **default (empty-root)** mode on
   bootc 1.16.3 — foreign vendor dirs survive. This is the central good news.
2. **Never use `--replace=alongside` on a shared ESP with a foreign OS.** Its
   own help says it wipes bootloader state ("the bootloader state will have its
   contents wiped and replaced"). We used the default empty-root mode instead.
3. **bootc prepends its NVRAM entry and becomes the default boot.** The incumbent
   is preserved but demoted. The real install must **re-assert `BootOrder`**
   afterwards to the desired default (`efibootmgr -o …`) and record it.
4. **`/EFI/BOOT/BOOTX64.EFI` (removable fallback) is written by bootc.** It was
   absent before and bootc created it. If an incumbent already uses the
   removable-media fallback path, bootc **would overwrite** it. Void on the L16
   boots via a dedicated efibootmgr EFISTUB entry (not the fallback), so this is
   expected-safe — but **pre-flight check the L16's `\EFI\BOOT\` before install.**
5. **SELinux was a non-issue.** The primary attempt (no `--disable-selinux`)
   succeeded even though the install host (Alpine) has no SELinux — bootc labels
   the target from the container's policy. The script keeps a `--disable-selinux`
   fallback but it was not needed. (Real install from a Fedora live USB is still
   the recommended, most-faithful environment.)
6. **`bootc install` needs `/run/udev` on the host.** From a bare Alpine
   minirootfs it aborts with *"Comparing filesystems at /run/udev …: No such file
   or directory"*. Fixed by running `eudev` (`udevadm trigger`/`settle`) and
   bind-mounting `-v /run/udev:/run/udev`. A normal Fedora live USB already runs
   systemd-udevd, so this is a harness detail, not a real-install step.
7. **podman needs cgroup v2 mounted.** Alpine's minirootfs mounts no cgroups;
   crun failed with *"invalid file system type on /sys/fs/cgroup"* until
   `mount -t cgroup2 none /sys/fs/cgroup`. Again a harness detail (Fedora live
   has it).
8. **First-boot bootstrap of the incumbent:** OVMF did **not** reliably fall
   through to its internal UEFI Shell / `startup.nsh` (it exhausted disk options
   then hung on PXE). The harness boots the incumbent once via qemu `-kernel`
   (still under OVMF, so efivarfs works) purely to *create* its persistent
   EFISTUB entry; every subsequent boot uses the real NVRAM entry. Not relevant
   to the real machines (which already have their entries).
9. **LUKS was deferred.** The host lacks `cryptsetup`, so this ran plain btrfs.
   The core ESP/BootOrder result is independent of LUKS. For the L16 LUKS2 path
   (plan §5b): create LUKS2 + btrfs by hand, mount the decrypted root at
   `/target`, add a **separate unencrypted `/boot`** partition mounted at
   `/target/boot`, and pass `--boot-mount-spec UUID=<boot>` so kernels/BLS land
   on the plain `/boot` while `/` stays encrypted. Needs a dedicated LUKS rehearsal.

### Verdict — is this safe enough for the L16?

**Yes, with defined pre-flight guards.** `bootc install to-filesystem` in
default empty-root mode is **non-destructive to a foreign OS's ESP files and
NVRAM entries**: it adds `/EFI/BOOT` + `/EFI/fedora` (~7.5 MiB) and prepends one
NVRAM entry. Void's EFISTUB kernels on the shared ESP and Void's efibootmgr
entry will survive; only the default boot target changes.

**Pre-flight checks the real L16 install needs (beyond the plan's current list):**

- **Snapshot boot state first:** `efibootmgr -v` + a recursive listing of the
  real ESP, saved off-box, so the post-install diff is auditable and BootOrder
  can be restored.
- **Inspect `\EFI\BOOT\` on the L16 before install** — if Void relies on the
  removable fallback there, bootc will overwrite it; plan to restore/relayer.
- **Re-assert `BootOrder` after install** (`efibootmgr -o …`) — bootc makes
  itself the default; decide and set the intended default explicitly, and
  confirm Void's entry is still bootable.
- **Verify ESP free space** (≥ ~10 MiB headroom) — trivially satisfied on a
  ≥256 MiB ESP but worth asserting since the L16 ESP already holds Void kernels.
- **Target root must be empty** and passed *without* `--replace` — and
  **`--replace=alongside` is forbidden** on the shared ESP.
- **Pin the bootc/image version and re-run this spike on every bump** — the
  "preserve vs wipe" behaviour is a bootupd implementation detail, not a
  guarantee; this result is specific to bootc 1.16.3 / fedora-bootc:43.
- **Prefer a Fedora live USB as the install host** (SELinux + udev + cgroup2
  present) to avoid the harness workarounds above; capture the Windows/Void
  baseline and (Katana) BitLocker recovery key first per the existing plan.
- **Still to rehearse separately:** the **LUKS2 + separate `/boot` +
  `--boot-mount-spec`** variant (§5b), and Secure Boot interaction (§5a).
