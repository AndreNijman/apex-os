# M0 results

Findings and evidence from the M0 spikes. Each spike proves one mechanic of the
APEX-OS build/boot chain before it is committed to in later milestones.

---

## Spike D — Secure Boot chain

**Goal:** prove the Secure Boot signing-chain mechanics with our own key — an
APEX-signed kernel boots with SB **enforcing** in a VM, and unsigned / foreign
kernels are **rejected** by the firmware. Scriptable and headless (no
interactive MokManager), so it can drop into CI.

**Verdict:**

| Step | Result |
|------|--------|
| Generate APEX signing keypair (test) | **PASS** |
| Enroll our cert as PK/KEK/db, SB on, headless | **PASS** |
| APEX-signed kernel boots under SB enforcing (SB detected + lockdown active + userspace) | **PASS** |
| Unsigned kernel rejected by firmware | **PASS** (`Access Denied`) |
| Foreign (rogue-key) signed kernel rejected | **PASS** (`Access Denied`) |
| Kernel-module signing (bonus) | **NOT DEMONSTRATED in-VM** — impractical here; exact procedure documented below |

Everything below was run on the Void host (qemu 11.0.2 + KVM). Large artifacts
live outside the repo in `~/apex-os-m0-work/spike-d/`; only the scripts
(`signing/spike-d/`) are committed. **No key material is committed.**

### Environment / tools installed

- `qemu-system-x86_64` 11.0.2, KVM (`/dev/kvm`, user in `kvm` group).
- OVMF: `edk2-ovmf-202605_1`, SB-enforcing build
  `/usr/share/edk2/x64/OVMF_CODE.secure.4m.fd`
  (sha256 `71359fc0…97d5`), template varstore `OVMF_VARS.4m.fd`
  (sha256 `5d2ac383…5d1e`).
- **Installed via xbps:** `sbsigntool-0.9.4_6` (provides `sbsign`, `sbverify`,
  `sbvarsign`).
- **Installed via pip (isolated venv, host untouched):** `virt-firmware 26.7.2`
  → `virt-fw-vars`. (System pip is PEP-668 "externally managed"; a venv at
  `~/apex-os-m0-work/spike-d/venv` avoids `--break-system-packages`.)
- Already present: `gcc`, `cpio`, `mtools` (`mcopy`/`mmd`/`mformat`),
  `mkfs.vfat`, `openssl`.

### Test subject

Rather than download a distro cloud image, the stock host kernel
`/boot/vmlinuz-7.1.4_1` was used as the test subject — it already carries what
we need: `CONFIG_EFI_STUB=y` (bootable PE with EFI handoff),
`CONFIG_SECURITY_LOCKDOWN_LSM=y` + `_EARLY=y`, `CONFIG_SERIAL_8250_CONSOLE=y`.
A ~330 KB initramfs (statically-linked C `init`, see
`~/apex-os-m0-work/spike-d/initramfs/init.c`) mounts `/proc` `/sys` `efivarfs`,
tags the kernel's SB/lockdown log lines, reads the `SecureBoot` EFI var, and
powers off — so the VM proves userspace with no root disk and self-terminates.

### Exact invocations

**1. Keypair (`keygen.sh`)** — RSA-2048, self-signed, SHA-256; the future
"APEX MOK"/db key (here a throwaway TEST key):

```sh
openssl req -new -x509 -newkey rsa:2048 -nodes \
  -keyout apex-mok.key -out apex-mok.crt -days 3650 -sha256 \
  -subj "/CN=APEX-OS TEST Secure Boot key (SPIKE-D, DO NOT TRUST)/"
openssl x509 -in apex-mok.crt -outform DER -out apex-mok.der
```

**2. Enroll into an SB-enforcing varstore (`enroll-vars.sh`)** — headless, our
key only, Microsoft keys deliberately omitted:

```sh
virt-fw-vars \
  --input  /usr/share/edk2/x64/OVMF_VARS.4m.fd \
  --set-pk  <GUID> apex-mok.der \
  --add-kek <GUID> apex-mok.der \
  --add-db  <GUID> apex-mok.der \
  --no-microsoft --secure-boot \
  --output apex-VARS.ours-only.4m.fd
```

Result (`virt-fw-vars --print`): `PK`, `KEK`, `db` each a 911-byte blob (our
cert), `dbx` seeded, `SecureBootEnable: ON`. PK present ⇒ firmware leaves Setup
Mode and enters User Mode = SB enforcing.

**3. Sign the kernel (`sign-kernel.sh` → `sbsign`)**:

```sh
sbsign --key apex-mok.key --cert apex-mok.crt \
       --output vmlinuz-apex-signed.efi /boot/vmlinuz-7.1.4_1
sbverify --cert apex-mok.crt vmlinuz-apex-signed.efi   # -> Signature verification OK
```

A rogue keypair (not enrolled) signed a second copy, and a third copy was left
unsigned, for the negative tests.

**4. Boot under SB enforcing (`boot-sb-vm.sh`)** — the key QEMU invocation
(SMM on, which the `OVMF_CODE.secure` build requires):

```sh
qemu-system-x86_64 \
  -machine q35,smm=on,accel=kvm -cpu host -m 2048 -smp 2 \
  -global driver=cfi.pflash01,property=secure,value=on \
  -global ICH9-LPC.disable_s3=1 \
  -drive if=pflash,unit=0,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.secure.4m.fd \
  -drive if=pflash,unit=1,format=raw,file=vars-<test>.fd \
  -drive if=virtio,format=raw,file=esp-<test>.img,media=disk \
  -serial file:serial-<test>.log -display none -no-reboot
```

The FAT ESP holds `\EFI\BOOT\BOOTX64.EFI` (an **APEX-signed** UEFI shell) plus a
`startup.nsh` that launches `vmlinuz.efi` with a cmdline + `initrd=`. The shell
being APEX-signed is itself a positive check (it loads); the shell's `LoadImage`
of the kernel is the signature gate under test. The cmdline included
`console=ttyS0,115200 ... lockdown=integrity` (see lockdown note below).

### Serial evidence

**Positive — APEX-signed kernel boots (SB enforcing):**

```
FS0:\> vmlinuz.efi initrd=\initramfs.cpio.gz console=ttyS0,115200 ... lockdown=integrity
[    0.016634] Secure boot enabled
[    0.925962] Lockdown: swapper/0: hibernation is restricted; see man kernel_lockdown.7
[    0.977893] Run /init as init process
APEX-SPIKE-D: >>> reached userspace: kernel executed under UEFI Secure Boot <<<
APEX-EVIDENCE: Kernel is locked down from command line; see man kernel_lockdown.7
APEX-EVIDENCE: Secure boot enabled
APEX-EVIDENCE: efivar SecureBoot = 1 (1 = enabled)
[    2.004344] reboot: Power down
```

⇒ firmware verified the kernel against db, kernel detected Secure Boot, lockdown
LSM is active and enforcing (hibernation restricted), and userspace ran.

**Negative — unsigned kernel AND rogue-signed kernel (same VARS):**

```
FS0:\> vmlinuz.efi initrd=\initramfs.cpio.gz console=ttyS0,115200 ...
Script Error Status: Access Denied (line number 5)
```

⇒ firmware `LoadImage` refused the image (`EFI_ACCESS_DENIED`); **no kernel
messages, no userspace** — the boot never started. Identical result whether the
image was unsigned or signed by a key absent from db, confirming it is the
signature-vs-db check doing the gating, not merely the presence of a signature.
(The OVMF `release` build emits nothing on debug port 0x402, so the shell's
`Access Denied` on serial is the authoritative rejection evidence.)

### Lockdown caveat (important, and it differs on real distro kernels)

The Void host kernel is built `CONFIG_LOCK_DOWN_KERNEL_FORCE_NONE=y` — it does
**not** auto-enter lockdown just because Secure Boot is on. So `lockdown=integrity`
was passed on the cmdline to activate the lockdown LSM, and the log reads
"locked down **from command line**". Fedora/RHEL/Ubuntu kernels are built with
the SB→lockdown coupling and would instead print "locked down **from EFI Secure
Boot mode**" automatically. **Implication for the APEX kernel:** build it with
`CONFIG_LOCK_DOWN_KERNEL_FORCE_INTEGRITY=y` (or the SB-coupling) so lockdown is
automatic under SB and not dependent on a cmdline argument a user could drop.

### Kernel-module signing (bonus) — why not shown in-VM, and the real procedure

Not demonstrated end-to-end here, for concrete reasons on this host:

- No kernel-devel/`build` tree and no `scripts/sign-file` for `7.1.4_1`, so an
  out-of-tree `.ko` can't be built/signed in place.
- More fundamentally: **module signatures are checked against the kernel
  keyring, not the UEFI db.** The stock Void kernel has
  `CONFIG_SECONDARY_TRUSTED_KEYRING` unset and no `.machine`/MOK keyring
  (`CONFIG_INTEGRITY_MACHINE_KEYRING`), so it only trusts its ephemeral built-in
  `certs/signing_key.pem`. Enrolling our key in **db** (which gates the *boot*
  chain) does nothing for module trust. Under `lockdown=integrity` unsigned
  modules are refused, but we cannot make this kernel *accept* an APEX-signed
  module without rebuilding it.

Exact procedure for the APEX image pipeline (captured in
`signing/spike-d/sign-module.sh`):

```sh
# Sign every shipped kmod with the APEX key (DER cert), sha512 to match
# the kernel's CONFIG_MODULE_SIG_HASH:
/lib/modules/<kver>/build/scripts/sign-file sha512 apex-mok.key apex-mok.der module.ko
```

For the kernel to trust those signatures, the APEX kernel must be built so the
APEX public key is in a trusted keyring — either bundled at build time
(`CONFIG_SYSTEM_TRUSTED_KEYS=/path/apex.pem`, with `CONFIG_MODULE_SIG=y`), or
loaded at runtime via the `.machine` keyring (`CONFIG_INTEGRITY_MACHINE_KEYRING=y`
+ shim/MOK). Pair with `CONFIG_MODULE_SIG_FORCE=y` (or `module.sig_enforce=1`)
to reject unsigned modules unconditionally.

### Implications for the CI signing pipeline (M1/M5)

- The four scripts in `signing/spike-d/` are the pipeline primitives:
  `sbsign` for boot components (kernel/UKI/shim/bootloader), `virt-fw-vars` for
  building test varstores, and `boot-sb-vm.sh` as an automated SB smoke test
  that CI can gate on (APEX-signed boots, unsigned/foreign `Access Denied`).
- Keep the private key out of the tree — inject from a CI secret / HSM at sign
  time. Only the public cert (DER) ships in images for enrollment. Repo
  `.gitignore` already blocks `*.key`/`*.pem`/`*.p12`.
- Sign **all** early-boot PE objects with the same key: kernel/UKI and, if shim
  is used, the shim's payload (grub/systemd-boot/UKI). Prefer a single UKI
  (kernel+initrd+cmdline in one signed PE) so the cmdline is inside the
  signature envelope and can't be tampered with — and so lockdown isn't left to
  a mutable cmdline arg.
- Match the kernel's module-sig hash (sha512 here) and bake the APEX key into a
  trusted keyring so kmod signing is enforceable (above).

### db-enroll (this VM) vs shim + MOK (real hardware) — state it plainly

This spike enrolled the APEX key directly into UEFI **db** (with self-owned
PK/KEK). That is legitimate and fully enforcing, but only feasible where we
control the firmware's key database — i.e. VMs, or physical machines where the
owner clears Setup Mode and enrolls a custom PK/db. It is **not** how most retail
hardware ships: those trust the **Microsoft UEFI CA** in db and cannot easily
have db rewritten.

On real hardware the APEX flow is therefore **shim + MOK**, not db:

1. Ship a `shim` signed by the Microsoft UEFI CA (already trusted in db).
2. shim carries/enrolls the **APEX cert as a MOK** (Machine Owner Key); MOK
   enrollment is confirmed once by the user in MokManager at first boot (or
   pre-seeded via `mokutil`).
3. shim verifies the APEX-signed kernel/UKI against the MOK — no db change
   needed, SB stays enforcing, and the MOK is linked into the kernel's
   `.machine` keyring so the **same key also validates signed modules**.

So: the *signing* commands proven here (`sbsign`, `sign-file`) are identical for
both paths — only *where the trust anchor lives* differs (db in the VM;
MOK-behind-shim on locked-down retail firmware). To offer both, ship the APEX
cert for db-enrollment on owner-controlled machines **and** a
Microsoft-CA-signed shim that enrolls the same cert as a MOK everywhere else.
```
