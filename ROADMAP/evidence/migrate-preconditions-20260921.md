# migrate-preconditions — measured 2026-09-21, L16

Everything below was run, not reasoned about. Commands are read-only unless
said otherwise; nothing was written to this machine's disks.

## 1. bootc does NOT write the ESP the caller mounts (composefs + systemd-boot)

This contradicts `docs/apex-owns-its-esp.md`, which states "`bootc install
to-filesystem` uses the ESP the CALLER mounted … bootc does not go hunting; it
writes where it is pointed."

Version pinned before reading, because a source read against the wrong revision
is worth nothing:

```
$ bootc --version
bootc 1.16.10
$ git -C /var/lab-scratch/sdboot-xbootldr/bootc-src describe --tags
v1.16.10                       # 3e76c16 "Release 1.16.10" — the exact version above
$ git -C /var/lab-scratch/sdboot-xbootldr/bootc-main describe --tags
v1.16.11
```

`crates/lib/src/bootc_composefs/boot.rs` reaches the ESP at **four** call sites.
All four are `find_first_colocated_esp()`:

| site | 1.16.10 | 1.16.11 | arm |
|---|---|---|---|
| `setup_composefs_bls_boot` | 691 | 708 | `BootSetupType::Setup` (install) |
| `setup_composefs_bls_boot` | 729 | 746 | `BootSetupType::Upgrade` |
| `setup_composefs_uki_boot` | 1256 | 1285 | `BootSetupType::Setup` (install) |
| `setup_composefs_uki_boot` | 1273 | 1302 | `BootSetupType::Upgrade` |

```rust
// Locate ESP partition device by walking up to the root disk(s)
let esp_part = root_setup.device_info.find_first_colocated_esp()?;
...
let esp_mount = mount_esp_writable(&esp_device).context("Mounting ESP")?;
```

`crates/blockdev/src/blockdev.rs:214`:

```rust
pub fn find_first_colocated_esp(&self) -> Result<Device> {
    self.find_colocated_esps()?
        .and_then(|mut v| Some(v.remove(0)))
```

`boot_mount_spec()` appears in that function exactly once, and only to build a
`systemd.mount-extra=<src>:/boot:<fstype>:<opts>` kernel argument. **It does not
steer the loader write.**

### Three consequences

1. **Mounting APEX's ESP does not make bootc write it.** Any design that mounts
   a chosen ESP and expects the UKI to land there is not what 1.16.10 or
   1.16.11 does. → `windows-installer-3`.
2. **must-measure #3 is answered, from source, without a lab.** Both `Upgrade`
   arms call `find_first_colocated_esp()` afresh, so **every later `bootc
   upgrade` re-walks the GPT**. There is no runtime store pointer to pin or to
   check. If Windows' ESP is first in partition order it is written forever,
   not once at install.
3. **ESP preference on this path can only be expressed as GPT partition
   ORDER.** No flag overrides the selection on either subcommand:

```
$ bootc install to-existing-root --help | grep -iE 'boot-mount|esp'
      --bootupd-skip-boot-uuid
      --bootloader <BOOTLOADER>
```

`to-existing-root` takes only `[ROOT_PATH]`. `--boot-mount-spec` exists solely
on `to-filesystem`, and per the source above it does not move the loader write.

## 2. The decision table on the real L16

`sudo apex-boot-migrate precheck --explain`, read-only, ESP mounted `ro`:

```
OK      uefi                         booted through UEFI
OK      on-ostree                    booted store is ostreeContainer
OK      no-staged-update             no ostree deployment is staged
OK      tools                        mkfs.vfat, rsync and podman all present
OK      bootc-new-enough             to-existing-root has --composefs-backend
REFUSE  secure-boot-unsigned-loader  Secure Boot is enabled and systemd-boot is unsigned
OK      root-space                   300 GiB free, 37 GiB needed
OK      esp-choice                   bootc will write PARTUUID 1c417de2-… of 1 ESP(s)
REFUSE  esp-too-small                590 MiB free, needs 1170 MiB, short by 580 MiB
rc=10
```

**Plain `precheck` shows only the first of those two refusals** — it exits at
Secure Boot having never mounted the ESP. That is the whole case for
`--explain`: this machine has two independent blockers and one of them was
invisible.

### Leak check, the same run

The ESP was **not** mounted beforehand (`findmnt -S /dev/nvme0n1p1` empty), so
this run exercised the `ro` mount and the new `trap release_esp EXIT` against
the production ESP. Afterwards: `findmnt -S /dev/nvme0n1p1` empty, and
`/run/apex-boot-migrate/esp` empty. No leak, no dirty bit — the shipped code
mounted it `rw` and leaked it on `refuse`.

## 3. The number `initramfs-slim` needs

Measured on 7.2.3-cachyos2.fc43.x86_64: `vmlinuz` 16,898,120 B + `initramfs.img`
375,558,646 B = **374 MiB per deployment**, and `need = per*3 + 48 MiB = 1170
MiB` against **590 MiB free** on a 600 MiB ESP.

The refusal now computes and prints the budget rather than the shortfall alone:

```
  * a kernel + initramfs of 180 MiB or less would fit this ESP
    as it is.
```

**180 MiB is the per-deployment ceiling for the L16 as it stands.** The figure
is derived from the live partition at run time — `(avail + staged - 48 MiB)/3` —
not hardcoded, so it moves with the machine. `initramfs-slim`'s varB (~100 MB
initramfs) plus this vmlinuz is ~117 MiB, which clears 180 MiB with room; that
is their number to confirm, not mine.

## 4. The Windows-ESP refusal, mutation-tested

`tests/test-boot-migrate.sh`, six behavioural assertions running the engine
against two fixture ESPs that differ by one file:

| engine | result |
|---|---|
| unmutated | 83 passed, 0 failed |
| `if windows_loader_on_esp` → `if false` | 82 passed, 1 failed — "an ESP carrying Windows' loader was allowed" |
| `if windows_loader_on_esp` → `if true` | 82 passed, 1 failed — "esp-is-windows fires on an ESP with no Windows loader — it is unconditional, so it proves nothing" |

A grep for the refusal's name would have passed the second mutation.

No `esp-is-windows` verdict appears on the L16, correctly: it has no NTFS and
no Windows on any disk.
