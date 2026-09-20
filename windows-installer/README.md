# Portable Windows installer: design and read-only image laboratory

Status: **not an installer**. This is a reviewable first slice: a dependency-free
Rust content validator, strict GPT image enumerator, and interactive console
selection by partition GUID. It has no device-opening API, disk-write API,
firmware API, deployment command, elevation, or Install button. Do not use it to
certify a physical partition as safe. No real disks were used for development.

## Toolchain decision

Rust, edition 2024; intended release target `x86_64-pc-windows-msvc`, static CRT.
This produces a native portable executable without MSI, installer, .NET runtime,
WebView runtime, or a system service. Rust makes bounded byte parsing and owned
handles practical; future Windows bindings should isolate the small unsafe API
boundary. C# NativeAOT is plausible but brings an additional interop/AOT surface;
C++ makes this parser and handle ownership harder to audit. The present crate
uses only std, so its content checks run offline on Linux and Windows. A native
Windows front end can be added after the safety backend is proven.

The executable is a single file; a future install still needs several GB of
verified OS/staging payload and temporary workspace. “Portable” does not mean
that Linux bootc runs natively under Windows or that the OS payload fits in RAM.

Build/test on Linux:

```sh
cd windows-installer
cargo build --offline --locked
cargo test --offline --locked
cargo clippy --offline --locked --all-targets -- -D warnings
python3 tests/image_lab.py
cargo run --offline --locked -- lab /absolute/path/to/test-disk.img
```

The command lists partitions and requests an exact GUID on stdin. It reports
size, GPT name (escaped), type, attributes, extent and GPT disk identity. It
explicitly reports model/serial unavailable and filesystem label not probed;
a GPT name is **not** a filesystem label. Unknown selections fail. No default
selection is made. All-zero content is success for this diagnostic only; refusal
returns exit code 1. It never offers to erase signatures to make a target pass.

Intended Windows build, in a Windows x64 MSVC developer environment with Rust:

```powershell
$env:RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --release --locked --offline --target x86_64-pc-windows-msvc
# target/x86_64-pc-windows-msvc/release/apex-windows-installer.exe
```

This command is a build recipe, **not a verified Windows artifact**. This session
has Linux Rust 1.98; Windows target/linker and rustfmt are unavailable. Windows
compilation, executable imports, startup on a clean Windows VM, and reparse-point
guards still need validation. The integration runner currently targets the Linux
binary. No release artifact or Authenticode signing is provided.

## What “empty” means here

1. Open a regular `.img` file read-only. Reject device/network/verbatim paths on
   Windows, symlinks, reparse points and nonregular files. This lab guard is not
   hardened against a hostile process replacing path components during open.
2. Accept only conventional 512-byte-sector GPT: protective MBR (no hybrid),
   revision 1, 92-byte headers, 128 entries of 128 bytes, both headers' CRC32,
   both identical tables and table CRC32, matching disk identity and usable
   bounds. Reject missing/duplicate partition identities, overlaps, invalid
   bounds and residual bytes in unused entries. Unsupported geometry fails.
3. Select one exact GUID. The lab permits only Linux filesystem GPT type with
   zero attributes. Windows basic-data, ESP, recovery, MSR, RAID, LVM and unknown
   types fail even if all their bytes are zero. This is deliberately more
   restrictive than Windows Disk Management's normal basic-data partition.
4. Read **every byte of the selected partition**, in 1 MiB chunks, stopping on
   the first nonzero byte. Report actual bytes read and the first nonzero byte's
   partition-relative and image-absolute offsets. Short reads, overflow, empty
   ranges and I/O failures are errors, never evidence of emptiness.
5. Re-read the GPT and image length after scanning; changes fail. This detects
   layout changes but is NOT a content lock or atomic snapshot.

The check is not a signature allowlist. NTFS/FAT headers, ext superblocks,
btrfs backup superblocks, LUKS headers, RAID metadata, nested MBR/GPT, arbitrary
old data, and backup signatures at the last byte all fail if any nonzero byte
survives **inside the extent**. A quick format with no user files fails. Reading
zeroes through a sparse-file hole succeeds; it says nothing about the storage
medium's remanence or TRIM behavior. No secure-erasure claim is made.

The outer GPT is expected to exist: it defines the partition the user already
created. A zeroed Linux-type extent behind a valid but historically stale active
GPT entry will pass the lab content check. Bytes cannot establish why an entry
exists or who owns it. A stale *unused* entry with residual fields fails; a
protected type fails; a mismatched primary/backup table fails. Other partitions
and unallocated gaps are not scanned for content. Full Windows eligibility must
also prove ownership, non-use, stable identity and exclusive access. Until then,
**all installation is disabled**, even following an all-zero report.

## Reference behavior and required adaptation

Read `installer/apex-install` before this design. It validates the selected disk,
parent/partition relationships, minimum capacity (16 decimal GB), mounted state,
container membership, ESP type/parent, accounts and filesystem tooling. Partition
mode formats only the target (btrfs default), mounts the root and existing ESP,
checks roughly 40 MiB ESP free space, and calls privileged Linux
`bootc install to-filesystem`. Post-install creates the account in the ostree
deployment, sets hostname/locale/keymap, carries network settings, relabels with
the target SELinux policy, and handles Secure Boot/MOK enrollment.

Do not invoke that engine from Windows or copy its bootloader side effects:
its `EFI/fedora` updates and possible `EFI/BOOT/BOOTX64.EFI` replacement conflict
with this task's additive-only rule. Its whole-disk mounted guard also cannot
simply apply to a Windows system disk. `installer/**` remains untouched; ongoing
LUKS changes must be reconciled before implementing deployment. Existing LUKS
headers are never overwritten. This prototype detects them as nonzero content.

## Proposed deployment architecture — not implemented

Use an isolated Linux appliance to create a target-sized filesystem image with
bootc and a **private synthetic ESP**. The appliance gets only scratch images,
never a physical disk, host ESP, or host firmware variables. Use userspace VM
emulation if an optional Windows hypervisor is unavailable; do not silently
install WSL/Hyper-V/drivers. Appliance packaging/licensing, performance, image
resizing and free-space budgets need a proof of concept before choosing a VM
runtime. Alternatively a signed build-produced root image may reduce local work,
but exact geometry and per-machine configuration still need proof.

Resolve `ghcr.io/andrenijman/apex-os` for the chosen Daily/Gaming flavor to one
immutable digest; validate the repository's cosign identity, source SHA,
architecture and kernel/module signatures before any target writes. Per-SHA tags
are traceability inputs, not substitutes for digest verification. Floating tags
move only on main builds; never resolve them twice during a transaction. Preserve
the correct flavor's update origin so bootc upgrade/rollback stays image-based.
No parallel Windows updater for image-owned components.

Configure the staged deployment using the target's tools and SELinux policy:
account/hostname, locale/keymap, signed kernel and expected MOK flow. Do not import
Windows credentials or assume Linux NetworkManager profiles exist. Confirm bootc
and ostree upgrades/rollback after transplanting the root image. Prove UUIDs,
BLS/root arguments, initramfs and bootloader references address the selected
partition without rewriting GPT. Encryption support must follow the Linux
contract and get separate VM tests; no encryption implementation exists here.

A future elevated writer accepts an immutable plan, not a drive number. It must:

- Enumerate via Windows storage APIs; correlate volume extents and system,
  boot, recovery, pagefile, crashdump, BitLocker, Storage Spaces/dynamic-disk and
  mounted/in-use ownership. Unknown state refuses. Do not offline the whole
  Windows disk or force-unlock anything. Prove exclusive target access for the
  entire validation/write interval, including RAW partitions without a volume.
- Bind selection to GPT disk GUID + unique partition GUID + offset/length +
  physical sector geometry + device model/serial/storage ID. Refuse missing or
  ambiguous identity, cloned GUIDs and duplicate IDs. Re-resolve after reboot or
  enumeration changes; never persist PhysicalDrive indices as authority.
- Show a plain-language review identifying size, filesystem label (verified
  absent for all-zero content), GPT name, model, serial, GUID and extent. Include
  an independently identified shared Windows ESP and an exact list of new files
  and boot variables. Example: “Write the verified APEX Daily filesystem to the
  selected 100 GB partition, filesystem label: none, on MODEL / SERIAL. Add the
  listed APEX boot files to this shared Windows ESP. Windows remains the default.”
  The real values, byte counts, digest and paths must replace every placeholder.
- Require explicit final confirmation tied to the plan and fresh all-zero scan
  under the held handle/lock. A changed identity, layout or precondition
  invalidates consent. No API accepts a reusable `is_empty=true` boolean.
- Confine writes to that one verified root extent; validate every offset/length,
  never rewrite GPT/MBR, flush and read back the deployed image. Partial failure
  is reported as incomplete; retries cannot bypass emptiness by assuming old
  writes are ours. Recovery needs a separately reviewed transaction protocol.

### Shared Windows ESP: separate, narrow exception

The root emptiness rule never makes an ESP eligible as a root target. The only
proposed exception to “do not touch other partitions” is the explicitly reviewed,
additive boot-file transaction on the shared Windows ESP required by this task.
No ESP formatting, shrinking, cleanup, fallback replacement, or Microsoft writes.

Stage bootc output privately, then add only verified files in a fresh unique
`EFI/APEX-<transaction-id>/` namespace using create-new semantics. The shim/GRUB
chain must first be proven to work there under Secure Boot; copying or renaming
`EFI/fedora` is **not** assumed sufficient. If it cannot, implementation stops.
Do not write `EFI/Microsoft`, `EFI/fedora`, `EFI/BOOT`, Windows BCD, or existing
files even if their hashes match. Check FAT space and preserve hashes of every
preexisting file. ESP absence/ambiguity, naming collisions or concurrent changes
refuse. Never assume the ESP is on the root disk.

Before mutation persist a durable transaction journal in explicitly approved
application storage, with exact ESP identity, new path/hash pairs, firmware
variable names/attributes/bytes and preconditions. Journal must survive power
loss; design crash recovery before enabling writes. The default is no NVRAM
change until a separately reviewed boot-entry step. That step may create one
unused Boot#### and append its ID at the end of BootOrder, preserving every
existing ID's position and the Windows default. No BootNext change, no reordering,
no replacing a prior APEX entry. Read back and verify all firmware state. If
firmware unexpectedly reorders entries, do not claim success; recovery must be
proven in VM firmware before this code can ship.

Rollback deletes only journal-owned files/variables whose identities and hashes
still match and removes only the appended ID when the expected BootOrder matches.
Never restore an old entire ESP snapshot over later Windows changes. Preserve
Windows bootability at every interrupted stage; the root partition may remain
incomplete. Firmware writes are not atomic with FAT writes—this is a release
blocker requiring fault-injection tests, not something an “undo” button solves.

## Review gates before hardware support

- [ ] Windows x86_64 static-CRT build and clean-VM execution; Authenticode and
      supply-chain review; no installer, MSI, drivers or runtime installation.
- [ ] Audited Windows enumeration, ownership, hardware identity and exclusivity;
      stable-ID tests with controller order swaps and duplicate/missing serials.
- [ ] VHDX/VM tests for Windows/system/recovery/ESP refusal, BitLocker, hibernation,
      Storage Spaces, RAW volumes, 4Kn, short reads, hot unplug and lock failure.
- [ ] Fuzz/corpus review of GPT parsing, huge/overflowing ranges and concurrent
      modification; supported layouts expanded only with tests.
- [ ] Verified all-byte scan and explicit consent immediately before bounded
      writes; no disk-index authority or automatic “make empty” operation.
- [ ] Signed immutable payload, geometry, capacity and SELinux/account setup;
      bootc first boot, upgrade and rollback in a VM, all three flavors.
- [ ] Secure Boot/MOK and isolated bootloader namespace proven; all preexisting
      ESP bytes and Windows entry/order preserved through success and failures.
- [ ] Durable journal, additive-only rollback, cancellation/power-loss recovery
      and full Windows boot tests at every write boundary.
- [ ] Independent human review before any future hardware deployment. All
      development/testing remains VHDX, VM or regular image files only.

## Primary API references

- [bootc installation requires a Linux host kernel](https://bootc.dev/bootc/bootc-install.html)
- [Externally prepared filesystem installation](https://bootc.dev/bootc/man/bootc-install-to-filesystem.8.html)
- [Windows partition layout query](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_disk_get_drive_layout_ex)
- [Volume locking](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_lock_volume)
- [Firmware variable writes and required privilege](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setfirmwareenvironmentvariableexw)

These establish available APIs, not proof that the proposed transaction is safe.
