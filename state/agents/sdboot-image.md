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
| `files/system/selinux/apex_sdboot.te` | one-rule policy module: `bootupd_t` may enter through `init_exec_t` |
| `files/system/selinux/verify-apex-sdboot.py` | reads the rule back out of the **binary** policy |
| `files/system/units/10-apex-bless-boot-esp.conf` | `SELinuxContext=-…:bootupd_t:s0` on `systemd-bless-boot.service` |
| `files/system/libexec/apex-boot-count` | renames the **staged** entry to `+3-0`, chosen by composefs digest |
| `files/system/units/apex-boot-count.service` | `ExecStop`, `After=bootc-finalize-staged.service`, conditioned on `entries.srel`, deliberately NO `SELinuxContext=` |
| `Containerfile.core` | `systemd-boot-unsigned`, `systemd-ukify`, `checkpolicy`, `python3-setools`; compiles + verifies the module |
| `Containerfile.base` | ships the drop-in, helper and unit; cross-tier `semodule -l` assertion |
| `files/system/libexec/apex-boot-health` | **bug fix**: `tail -c +5` cannot read efivarfs; `dd bs=1 skip=4` can |
| `tests/test-boot-v2.sh` | +32 checks, mutation-tested; 150 pass |
| `docs/boot-v2.md`, `AGENTS.md` | the pivot design and the contract |
| `.github/workflows/boot-v2.yml`, docs | every host-side `podman run … apex-bootlab` wrapped in `nvram-guard` |

The whole `Containerfile` stanza set was run in a real `podman build` before
being committed (`/var/lab-scratch/sdboot-image-agent/coretest/Containerfile`),
and the one refusal that could only ever pass — a `! grep` in mid-chain, which
errexit ignores — was caught by `tests/check-containerfile-assertions.sh` and
rewritten as an `if … exit 1`, then tested both ways in a build.

Lab artefacts, all under `/var/lab-scratch/sdboot-image-agent/` and
`/var/lab-scratch/sdboot-lab/`: `blesslab/` (the guest image sources),
`bless2.img` (43 GB, the installed guest), `bless2-b1.serial` /
`bless2-b2.serial` (the two boots), `install2.log`, `nvram/` (the guard's
snapshot pairs). Delete the .img when done; it is the biggest thing here.

## The defects that decide whether the pivot works at all

* **`systemd-bless-boot` could not rename a loader entry on a FAT ESP.** AVC:
  `init_t` → `dosfs_t:file rename` denied, enforcing, on the APEX image.
  Fedora has no domain for the worker. Unrepaired, every deployment rolls back
  on its fourth boot. **Repaired, and PROVEN IN A BOOT on 2026-09-21**:
  `Marked boot as 'good'`, suffix stripped, zero AVCs.
* **`bootc` writes no boot counter**, so the health gate the image asserts is
  inert on a machine installed exactly as bootc leaves it. `apex-boot-count`
  writes it, into the staged entry.
* **The symmetric fix for `apex-boot-count` is wrong**, and the guest showed it
  while appearing to pass. `type_transition init_t bin_t:process
  unconfined_service_t` exists and `init_exec_t` has none, so a `/usr/libexec`
  helper is already unconfined and can rename `dosfs_t`; the systemd binary is
  not. Forcing `bootupd_t` there confined it and logged
  `avc: denied … path="/proc/cmdline"` — the read the counter depends on.
  Reverted; the unit must carry no `SELinuxContext=`, and two assertions say so.
* **`bootupd_t` is a permissive domain in Fedora 43** (one of 41). The
  blessing's zero AVCs still mean something — a permissive domain logs what it
  would deny, and nothing was logged — but the drop-in alone would probably
  work today without the module. The module is what makes it correct rather
  than tolerated, and what survives that domain becoming enforcing.

## NEXT — for a stranger

Everything below is pushed; nothing is half-applied. Start here.

1. **Re-run the guest with the REVERTED counter** — the only loose end from
   gate 3, and the cheap half. Everything is still on disk, so this is one
   cached rebuild, one install and two boots, roughly 40 minutes:

   ```
   # the lab image, built from the predecessor's APEX + sd-boot image
   cd /var/lab-scratch/sdboot-image-agent/blesslab
   cp /var/tmp/apex-work/wt-sdboot-image/files/system/{selinux/apex_sdboot.te,units/apex-boot-count.service,libexec/apex-boot-count} .
   sudo podman build -t localhost/apex-sdboot:bless3 -f Containerfile.apexbless .

   cd /var/tmp/apex-work/wt-sdboot-image
   sudo tests/lab/bootc-install-lab --size 43G \
     localhost/apex-sdboot:bless3 /var/lab-scratch/sdboot-lab/bless3.img \
     --bootloader systemd \
     -- --composefs-backend --karg console=ttyS0,115200n8 \
        --karg systemd.journald.forward_to_console=1

   # two boots; lab-run.sh stages on the first and probes on the second
   sudo tests/lab/nvram-guard --label b1 -- podman run --rm --device /dev/kvm \
     -v /var/lab-scratch/sdboot-lab:/work localhost/apex-bootlab \
     -c '/work/boot-apex.sh /work/bless3.img /work/bless3-b1.serial 420'
   ```

   Read `LAB-COUNTER` as a pass only if `bootc_lab-43-1+3-0.conf` appears AND
   `LAB-avc2` is empty. A rename with denials under it is the failure that
   looked like a pass last time — `bootupd_t` is permissive, so it logs rather
   than refuses.

   Two things that cost time last round: the images live in the **root** podman
   store (`sudo podman images`), not the user one; and a `podman build` that
   the harness backgrounds can be SIGTERMed and still exit 0, so check the
   image actually exists afterwards.
2. **Take the Secure Boot decision** (docs/boot-v2.md, "Secure Boot: a
   decision, not a measurement"): APEX-signed sd-boot the user enrols, or a
   shim → sd-boot chain APEX authors. Everything in phase 2 waits on it.
3. **The kernel landed (merge `74b777ac`) and one of the three asks is now a
   RULE, not a request.** Signing only the inner `vmlinuz` once a UKI exists
   gives **an unbootable machine with every assertion green** — the kernel is
   signed, the kernel check passes, and the object the firmware loads is
   unsigned. `AGENTS.md`'s Secure Boot invariant now says the built artifact is
   whatever the firmware loads, and `docs/boot-v2.md` gate 2 requires the
   signature and the `sbverify` gate to move to the UKI in the same change that
   produces one. The remaining two asks stand: kernel and initramfs stay at
   `/usr/lib/modules/<kver>/`, and the MOK signer must handle an arbitrary PE
   and attach `.sbat`. Two facts to not re-derive: the kernel image is
   `FROM scratch`, 186.9 MiB, and no user downloads it (this pivot's
   update-cost accounting is unaffected); and it has **no shell**, so read
   files out of it rather than shelling in.
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
