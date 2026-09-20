# luks-installer — L-002, "Enable LUKS2 by default"

Branch `task/luks-installer` in **apex-os**, worktree
`/var/tmp/apex-work/wt-luks-installer`, from `roadmap/v2.2` @ `303221d5`.
Not landed. Do not commit to `roadmap/v2.2`.

## What this unit changed

The installer could not create an encrypted disk at all. Every `crypto_LUKS`
branch in `installer/apex-install` was a refusal to overwrite an existing
header; there was no `luksFormat` anywhere in the tree. There is now a real
path, and it has done a real `bootc install` onto a real LUKS2 volume.

| file | what |
|------|------|
| `installer/apex-install` | `encrypt=yes` builds ESP + plain ext4 `/boot` + LUKS2→btrfs and installs with `bootc install to-filesystem`; calls `/usr/libexec/apex-luks-enroll`; proves BOTH keys open the volume before the install; surfaces the recovery key; writes `/etc/crypttab`; converts XKB layout → console keymap and puts it on the kernel command line |
| `installer/apex-installer-gui` | new **encrypt** page (box ticked by default, show-characters passphrase field, layout named); recovery key captured off stdout and shown on the last page behind an acknowledgement checkbox that gates Reboot |
| `Containerfile.core` | `cryptsetup` + `kbd` pinned explicitly with file-level assertions; the `99apex-unlock-hint` dracut module copied in |
| `Containerfile.apex` | the final dracut run now asserts systemd-cryptsetup, the crypt module, the tpm2 token plugin, non-`us` keymaps and the hint module are in the shipped initramfs |
| `files/branding/plymouth/*/apex-os.script` | `message()` was `{ }` in both themes; it now draws |
| `files/dracut/apex-unlock-hint/` | new dracut module: posts the console keymap and "use your recovery key" just before the passphrase prompt |
| `installer/test-installer-luks.sh` | **CI**, 36 assertions, all refusals + keymap conversion + character-level keyboard checks, 6 mutants |
| `installer/test-installer-luks-live.sh` | **not CI**: a real `bootc install` onto a loopback LUKS2 volume |
| `tests/check-suites-run-in-ci.sh` | discovered `tests/test-*.sh` only; now `installer/test-*.sh` too |
| `docs/disk-encryption.md` | new |

## The two things a stranger most needs to know

1. **`encrypt=` is now MANDATORY in the answers file.** The engine refuses a
   file without it rather than defaulting either way. Existing suites were
   given `encrypt=no`. The GUI always writes it, ticked.
2. **The keymap fix is a kernel argument, not an initramfs rebuild.** The
   initramfs is baked at image build, `--no-hostonly`, so it is identical on
   every machine; `systemd-vconsole-setup` inside it parses `vconsole.keymap=`
   from `/proc/cmdline`, which is the one per-machine channel that exists
   today. **This breaks when APEX moves to sd-boot + UKIs** (the cmdline is
   inside the signed PE). See `docs/disk-encryption.md` § "The seam" — the
   replacement is a systemd credential on the ESP or a UKI addon, and it is
   one function in the engine.

## NEXT

1. **Re-run `installer/test-installer-luks-live.sh`** (needs `sudo -n`, podman,
   `localhost/apex-os:daily` in ROOT storage, ~25 GB on `/var/lab-scratch` —
   never `/tmp`, which is a tmpfs and filled the machine once already). Two
   runs got as far as *"Installation complete!"* — partitioning, LUKS2,
   enrolment, both-key verification, btrfs, and the whole `bootc install` — and
   then failed at `useradd --root`: *"failure while writing changes to
   /etc/passwd"*. The write probe added after run 1 shows the filesystem is
   **not** read-only, so the read-only-remount theory is dead.
   A control install with `encrypt=no` on the same host (the long-standing
   `bootc install to-disk` path, which this unit did not touch) was running
   when this card was written: **read `/var/lab-scratch/control1.log` first.**
   If the control fails the same way, the cause is this host — it runs SELinux
   **enforcing** and the live ISO boots `selinux=0`; the journal shows
   `mac_admin` denials against `chcon` from inside bootc's container at exactly
   the install times. If the control SUCCEEDS, the fault is in the encrypted
   path's post-install and is this unit's to fix.
2. **Run `installer/test-installer.sh`** — its GUI half renders every page in
   the registry at 1024x600 and 1366x768, and the new `encrypt` page has not
   been through it. Same for `installer/test-installer-a11y.sh` (AT-SPI).
3. **Ask `luks-enroll` for two things** (see the report): a `run-scenarios`
   case that boots an encrypted volume with `vconsole.keymap=de` and types a
   passphrase containing a `z` through QMP `sendkey`; and confirmation that
   their token is one `tpm2-device=auto` discovers.
4. **Tell `sdboot-image` what the installer needs**: one string
   (`vconsole.keymap=<name>`) delivered to the initrd per machine, without
   re-signing. Recommended: `systemd-vconsole-setup` reads a `vconsole.keymap`
   **system credential**, and both sd-boot and sd-stub pass credentials from
   the ESP.
5. `ROADMAP/roadmap.yaml` L-002 evidence was updated by this unit; L-002 stays
   **blocked**. The remaining blockers are named there.

## Traps this unit paid for

* `/tmp` is a 15 GB **tmpfs**. A 30 GB loopback image there broke the Bash tool
  for twenty minutes. Use `/var/lab-scratch`.
* `printf … | grep -q` under `pipefail` returns 141 on a match. The passphrase
  ASCII check uses `grep -qv` on a **here-string** instead, and says why.
* A `sed '/text/,+Nd'` mutation over a multi-line `die` string eats the next
  statement, and a mutant that cannot parse looks exactly like a mutation that
  worked. All mutants here are one-line inversions.
* `find /usr/lib/kbd/keymaps -name us.map.gz -print -quit` can return the
  **Atari** keymap, which uses different keycodes entirely — "the US keymap
  types nothing" was a wrong answer dressed as a failure.
