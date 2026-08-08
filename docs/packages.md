# Installing software on APEX-OS

```bash
sudo apex install android-tools
sudo apex remove android-tools
apex search wireshark
apex pkg list
```

That is the whole interface. It works for ordinary Fedora packages — CLI tools,
libraries, development toolchains, GUI applications, fonts, services — and it
does **not** stop the OS from updating.

## Why this is not `rpm-ostree install`

APEX is a bootc image. Layering a package with `rpm-ostree` marks the deployment
as locally modified, and from that point on:

```text
error: Upgrading: Deployment contains local rpm-ostree modifications;
cannot upgrade via bootc.
```

One CLI tool and the machine silently stops receiving OS updates. Packages
applied with `--apply-live` were worse: they also disappeared on the next reboot,
so the user lost both the software and the update path.

`apex install` builds a **systemd system extension** instead: a squashfs image in
`/var/lib/extensions` that systemd overlays onto `/usr` at boot.

* the bootc deployment is never modified, so `bootc upgrade` keeps working
* programs land in the real `/usr/bin` — no wrappers, no PATH edits
* `.desktop` files, icons, man pages, shell completions, systemd units and udev
  rules work, because they sit exactly where the OS already looks
* removing a package is deleting a file; nothing rots in `/usr`
* `apex rollback` (the OS) and `apex pkg rollback` (packages) are independent

Fedora's signed repositories are the package source. There is no APEX package
registry to host, sign or keep online, and every RPM is checked against the
system keyring before a single file is extracted.

## What actually happens

1. `dnf5 download --resolve` resolves against the **installed image**, so only
   dependencies APEX does not already ship are downloaded.
2. Every RPM's signature is verified with `rpmkeys`.
3. The packages are extracted into a staging tree (`--noscripts`; the scriptlets
   that matter are emulated below).
4. Caches that are a single file describing a whole directory — GSettings
   schemas, the desktop database, the MIME database, GIO modules — are rebuilt
   from the **union** of the image and the new packages, so the extension can
   never hide the OS's own applications. Caches whose consumers re-scan safely
   (icon caches, fontconfig) are dropped instead.
5. The tree is labelled for SELinux with `setfiles`, so binaries are executable
   under enforcing.
6. It becomes one squashfs image, replaces the old one atomically, and systemd
   re-merges `/usr`.
7. `/etc` files ship to the real `/etc`; your edits are never overwritten (a new
   version lands beside yours as `*.apexnew`).

Everything the user requested lives in **one** extension, rebuilt from the
requested list on every change. Separate per-package images would fight over
shared dependencies and removal could delete files another package still needs.

## OS upgrades

An extension records the OS version it was built for, and systemd refuses to
merge a mismatched one. That refusal is the safety property that makes user
packages compatible with atomic updates — a Fedora 43 build is never overlaid
onto Fedora 44.

`apex-sysext-rebuild.service` completes the story: on the first boot after an OS
version change it rebuilds the extension against the new OS. It does nothing on a
normal boot, and if the machine is offline it says so and leaves the packages to
be rebuilt later rather than failing the boot.

`apex update` also re-resolves user packages, so they receive Fedora security
fixes instead of staying pinned at whatever was current on install day. If
nothing changed it stops early and does not re-merge `/usr`.

## Coming from a layered system

If a machine already has rpm-ostree layered packages, `apex update` will say so
and point at:

```bash
sudo apex pkg adopt
```

which rebuilds those same packages as an extension, then runs `rpm-ostree reset`
so the OS can update again. Reboot afterwards to drop the layered deployment.

## What it refuses, and why

| Refused | Reason |
|---|---|
| Kernels, `kmod-*`, `akmod-*` | need an initramfs and a real deployment — they belong in the image |
| `glibc`, `systemd`, `rpm`, `dnf`, `bootc`, `filesystem`, … | overlaying a second copy of the running userspace ABI is unrecoverable without a rollback |
| A **newer** version of something the image ships | that is an OS update, not a package install |
| Anything already in the image | already provided; nothing to do |

Packages with custom scriptlets install their files correctly, but APEX does not
execute arbitrary `%post` scripts against a live system. If a package needs one
to be useful, it belongs in the image — open an issue.

## Commands

| Command | Does |
|---|---|
| `apex install PKG…` | add packages (`--no-weak-deps`, `--enable-repo=REPO`) |
| `apex remove PKG…` | remove packages |
| `apex search TERM…` | search the repositories |
| `apex pkg list` | requested packages and dependency count |
| `apex pkg status` | extension state, what it was built for, whether merged |
| `apex pkg upgrade` | re-resolve everything against the repositories |
| `apex pkg rebuild [--if-needed]` | rebuild for the running OS version |
| `apex pkg rollback` | restore the previous extension |
| `apex pkg verify` | check the extension against its recorded checksum |
| `apex pkg adopt` | convert rpm-ostree layers into APEX packages |

Read-only verbs work as an ordinary user; anything that writes needs `sudo`.

## Notes

* Flatpak remains the right choice for sandboxed desktop applications, and
  Bazaar is still the graphical store. `apex install` is for the things Flatpak
  is a poor fit for: CLI tools, libraries, headers, drivers' userspace, anything
  that must exist in `/usr`.
* Set `APEX_PKG_FORMAT=tree` to build an uncompressed directory extension
  instead of squashfs. The engine falls back to this automatically if
  `mksquashfs` is unavailable.
* State lives in `/var/lib/apex/pkg` (`requested`, `state.json`, rollback copy).
  The extension itself is `/var/lib/extensions/apex-user.raw`.
