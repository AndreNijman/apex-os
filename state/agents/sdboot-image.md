# sdboot-image — pivot APEX to systemd-boot on every machine

- **Repo / branch**: apex-os, `task/sdboot-image` (pushed, merged with
  `origin/roadmap/v2.2` @ `b5f696cb`).
- **Worktree**: `/var/tmp/apex-work/wt-sdboot-image`.
- **Evidence**:
  - `ROADMAP/evidence/sdboot-image-20260920-lab.md` — install / boot / upgrade /
    rollback transcripts (predecessor).
  - `ROADMAP/evidence/sdboot-image-20260921-decision.md` — the SELinux blessing
    defect, the per-machine decision, the BIOS correction (this round).
- **The design lives in the repo**, not in this card: `docs/boot-v2.md`,
  section "The pivot to systemd-boot". `AGENTS.md` boot-path rule 5 is the
  contract version.
- **Andre's decision**: fully pivot APEX to systemd-boot on all machines. Not
  ours to relitigate. The reasons GRUB was kept are the constraints the design
  must satisfy.

## What is settled

1. **The pivot is a storage-backend change.** `bootc install --bootloader
   systemd` on the ostree backend is refused outright; only
   `--composefs-backend` works, and there is **no in-place converter**. An
   existing machine cannot be migrated — it is a reinstall.
2. **Per-machine configuration is systemd credentials on the ESP.** The cmdline
   stays image-static inside the UKI. Addons are ruled out (signature-gated
   against db/Shim MOK; APEX's key is a CI secret). LUKS is discovered by GPT
   partition type GUID, not `rd.luks.uuid=`. Keymap is `vconsole.keymap` in
   `<ESP>/loader/credentials/`. Secrets must be `systemd-creds encrypt
   --with-key=tpm2`. **`luks-installer` builds against this.**
3. **Phase 1 = unsealed Type #1 entries** (what is measured and what
   `apex-boot-count` handles). **Phase 2 = sealed UKI** (credentials become the
   only mechanism; boot counting must be redone for `.efi` filenames).
4. **Legacy BIOS costs almost nothing, because it never worked.** The L16 has
   no BIOS boot partition at all (partition-mode install); whole-disk installs
   create one and never write to it. The live ISO does boot BIOS.
5. **Secure Boot is an undecided chain, not a pending measurement** — bootc's
   ESP has no shim and `systemd-boot-unsigned` is unsigned.

## What was built this round

| file | what |
| --- | --- |
| `files/system/selinux/apex_sdboot.te` | two-rule policy module: `bootupd_t` may enter through `init_exec_t` (the blessing) and `bin_t` (apex-boot-count) |
| `files/system/selinux/verify-apex-sdboot.py` | reads the rule back out of the **binary** policy |
| `files/system/units/10-apex-bless-boot-esp.conf` | `SELinuxContext=-…:bootupd_t:s0` on `systemd-bless-boot.service` |
| `files/system/libexec/apex-boot-count` | renames the **staged** entry to `+3-0`, chosen by composefs digest |
| `files/system/units/apex-boot-count.service` | `ExecStop`, `After=bootc-finalize-staged.service`, conditioned on `entries.srel`, runs in `bootupd_t` |
| `Containerfile.core` | `systemd-boot-unsigned`, `systemd-ukify`, `checkpolicy`, `python3-setools`; compiles + verifies the module |
| `Containerfile.base` | ships the drop-in, helper and unit; cross-tier `semodule -l` assertion |
| `files/system/libexec/apex-boot-health` | **bug fix**: `tail -c +5` cannot read efivarfs; `dd bs=1 skip=4` can |
| `tests/test-boot-v2.sh` | +26 checks, mutation-tested; 148 pass |
| `docs/boot-v2.md`, `AGENTS.md` | the pivot design and the contract |
| `.github/workflows/boot-v2.yml`, docs | every host-side `podman run … apex-bootlab` wrapped in `nvram-guard` |

The whole `Containerfile` stanza set was run in a real `podman build` before
being committed (`/var/lab-scratch/sdboot-image-agent/coretest/Containerfile`).

## The two defects that decide whether the pivot works at all

* **`systemd-bless-boot` cannot rename a loader entry on a FAT ESP.** AVC:
  `init_t` → `dosfs_t:file rename` denied, enforcing, on the APEX image. Fedora
  has no domain for the worker. Unrepaired, every deployment rolls back on its
  fourth boot. Repaired at the policy level; **not yet proven in a boot.**
* **`bootc` writes no boot counter**, so the health gate the image asserts is
  inert on a machine installed exactly as bootc leaves it.
* **`apex-boot-count` would have hit the identical SELinux wall** — it renames
  a `.conf` on the same FAT ESP from a `bin_t` helper, so PID 1 leaves it in
  `init_t`. Found by reading the change, not by a boot. Same repair. The
  failure modes differ and it matters: the blessing failing rolls a deployment
  back; the counter failing just means no counter, which is the status quo.

## NEXT — for a stranger

Everything below is pushed; nothing is half-applied. Start here.

1. **Close gate 3: prove the blessing in a booted guest.** This is the single
   highest-value thing left and it is a few hours.
   - Build `fedora-bootc:43` + `systemd-boot-unsigned` + `apex_sdboot.pp` +
     the drop-in + a trivial `RequiredBy=boot-complete.target` oneshot (stock
     fedora-bootc never reaches that target, which is why the predecessor's
     guests never blessed).
   - Install with **`tests/lab/bootc-install-lab IMAGE TARGET.img --bootloader
     systemd -- --composefs-backend`** — never a hand-written `podman run`.
     AGENTS.md rule 6. Lab dir `/var/lab-scratch/sdboot-lab`, **never `/tmp`**.
   - Boot once, rename the entry to `+3-0`, boot again, and check the suffix is
     gone and no AVC appeared. `/var/lab-scratch/sdboot-lab/boot-apex.sh` is the
     qemu wrapper the predecessor used.
   - **Watch for a second AVC.** `bootupd_t` was probed for the other
     permissions `systemd-bless-boot` needs and all were present *except* block
     device access: `bootupd_t → fixed_disk_device_t:blk_file` is `getattr`
     only, and systemd's ESP verification can open the block device with
     libblkid. If that denial appears, add exactly what the AVC names to
     `apex_sdboot.te` — do not pre-emptively widen it.
   - On a booted bootc machine **`semodule -l` may list nothing** (the module
     store under `/var/lib/selinux` is not part of the deployment;
     `/etc/selinux/targeted/policy/policy.NN` is). The runtime proof is setools
     against the binary policy, or the absence of the AVC — not `semodule -l`.
2. **Take the Secure Boot decision** (docs/boot-v2.md, "Secure Boot: a
   decision, not a measurement"): APEX-signed sd-boot the user enrols, or a
   shim → sd-boot chain APEX authors. Everything in phase 2 waits on it.
3. **Tell `kernel-build` the three asks** in that document: kernel and
   initramfs stay at `/usr/lib/modules/<kver>/`; the MOK signer must handle an
   arbitrary PE and attach `.sbat`; the `sbverify` gate moves to the UKI.
4. **Phase 2 work, in order**: `bootc container ukify` against the APEX image;
   a credential changing the keymap at a real LUKS prompt in a guest (this is
   what `luks-installer` will want to see); then `apex-boot-count`'s second
   branch for `.efi` filenames.
5. **An ESP sizing decision.** 374 MiB per deployment on the L16, ~1.1 GiB
   needed for two plus a staged third. bootc's composefs default is 1 GiB,
   which is under the peak. The installer must ask for more.
6. **The Windows-entry assertion** (gate 4). Build a lab disk carrying a
   `Windows Boot Manager` entry, install into it, `cmp` `efibootmgr -v` either
   side. Katana's APEX `Boot0000` lives on the **Windows** ESP.

Not this unit's files, flagged rather than edited:

- `tests/lab/bootc-install-lab:46` still says "GRUB stays the default for
  published images, AGENTS.md rule 5". Rule 5 now says the opposite.
  `efivars-guard` owns that file.
- `installer/apex-install` passes neither `--bootloader` nor
  `--composefs-backend`, so both its modes still install ostree + GRUB. That is
  `luks-installer`'s to change, against the contract in `docs/boot-v2.md`.

Bounds that still hold: **do not touch the L16**, **do not migrate katana**,
large artefacts in `/var/lab-scratch`, address katana's disks by serial or
PARTUUID, never `pkill -f`.
