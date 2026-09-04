# APEX-OS Branding

## The mark: "spark"

APEX-OS uses a single logomark — the **spark** — rendered in edition-specific
colorways. The wordmark is the letterspaced "APEX OS".

## Color semantics

| Colorway | Hex (highlight) | Used for | Represents |
|----------|-----------------|----------|------------|
| **Chartreuse** | `#D9F99D` | APEX-OS | Everyday |
| **Gold** | `#FDE047` | legacy Gaming accent | Power |
| **Mono (black)** | — | neutral | Light backgrounds, print, single-color contexts |
| **Mono (white)** | — | neutral | Dark backgrounds, single-color contexts |

APEX publishes ONE image, so chartreuse is the colour of the product: the boot
splash and the greeter accent. There is no edition for gold to denote any more.

Gold is retained for one reason, and only that reason: a machine still booting a
pre-merge image reports `VARIANT_ID=gaming`, and apex-greet maps that to the gold
accent and `spark-gold.png` so it keeps its identity until it updates. New
artwork should not use gold to mean anything.

The mono variants exist for any context that needs a neutral, single-color mark.

## Asset inventory

All assets live under `files/branding/`.

### Logos — `files/branding/logos/<colorway>/`

Colorways: `gold/`, `chartreuse/`, `mono-black/`, `mono-white/`.

Each colorway provides:

- `apex-spark-<colorway>.svg` — vector source (512×512 viewBox)
- `apex-spark-<colorway>.png` — 512×512 base raster
- `apex-spark-<colorway>-{16,32,64,128,256,512,1024}.png` — icon sizes

(Mono files use the suffix `black` / `white`, e.g. `apex-spark-black-256.png`.)

### Plymouth boot themes — `files/branding/plymouth/`

**One theme ships.** `Containerfile.apex` copies `apex-os-chartreuse` and runs
`plymouth-set-default-theme apex-os-chartreuse`; nothing installs the gold
theme, because there is no second image to install it into.

- `apex-os-chartreuse/` — the boot splash, on every machine
- `apex-os-gold/` — source art only. NOT installed by any Containerfile. Kept
  because it is the same animation in the other colourway and deleting
  commissioned art to tidy a build is not a trade worth making; do not read its
  presence as evidence that a second image exists.

Each theme contains its `.plymouth` descriptor, the shared `apex-os.script`
animation, and the sprite images `spark.png`, `comet.png`, `glow.png`,
`flash.png`.

**Animation — "Convergence":** four comets with soft, speed-stretched light
trails swoop in from outside the frame, orbit and accelerate, spiral into the
center, flash, and morph into the spark; the wordmark rises in underneath and
the spark holds with a subtle breathing loop. The trails are tapered ribbons
sampled from each comet's own past path (stretching with angular velocity),
and the heads rotate with the true velocity vector, so the motion reads as a
continuous fluid sweep rather than orbiting dashes. LUKS password prompts are
handled (theme dims, prompt + bullets shown); shutdown shows a static spark.

Install (inside the image build):

```sh
cp -r apex-os-chartreuse /usr/share/plymouth/themes/
plymouth-set-default-theme -R apex-os-chartreuse   # -R rebuilds the initramfs
```

Kernel args need `quiet splash`. See
`files/branding/plymouth/README.md` for install/test details.

### Previews

`files/branding/plymouth/previews/preview-gold.gif` and
`preview-chartreuse.gif`. Regenerate with:

```sh
cd files/branding/plymouth
./render-preview.sh apex-os-gold previews/preview-gold.gif
./render-preview.sh apex-os-chartreuse previews/preview-chartreuse.gif
```

`render-preview.sh` reproduces the `.script` animation math in `awk` and
composites frames with ImageMagick (`magick`), so it requires ImageMagick 7.

### Wallpaper — `files/branding/wallpapers/`

`apex-wallpaper-default.jpg` — the default desktop wallpaper (3258×2160).

---

# Distro identity — de-branding Fedora / CachyOS

APEX-OS is built `FROM quay.io/fedora/fedora-bootc:43` and swaps in the CachyOS
performance kernel from the `bieszczaders` COPR. Both upstreams leave their name
in places the user can see. **No user-visible surface may say "Fedora" or
"CachyOS".** This section is the complete inventory: every surface, what it said,
what it says now, and where the fix lives.

Three surfaces genuinely cannot be changed. They are listed honestly at the
bottom rather than papered over.

## Where each surface is fixed

Fixes land in one of two places:

- **image** — `Containerfile.core` for OS-level branding (os-release,
  fedora-release, issue), `Containerfile.base` for anything COPY'd out of
  `files/`. Applies to every
  deployment created from a newly built image.
- **runtime** — `files/scripts/apex-debrand-runtime.sh`. Applies to a system that
  is **already installed**. Three of these surfaces live *outside* the ostree
  deployment (EFI NVRAM, `/boot/loader/entries`, a local `/etc` modification), so
  `bootc upgrade` will never replace them no matter how good the image gets.

## The inventory

| # | Surface | Was | Is now | Fixed in | Where |
|---|---------|-----|--------|----------|-------|
| 1 | GRUB boot menu entry (`/boot/loader/entries/*.conf` → `title`) | `Fedora Linux 43 (Forty Three) (ostree:0)` | `APEX-OS (ostree:0)` | image **+** runtime | ostree derives the title from the deployment's os-release `PRETTY_NAME`; `Containerfile.core` sets it. Existing entries: `apex-debrand-runtime.sh` |
| 2 | Firmware boot entry label (`efibootmgr`) | `Boot0002* Fedora` | `Boot####* APEX-OS` | image **+** runtime | bootupd's `get_product_name()` reads `/etc/system-release`; `Containerfile.core` rewrites `/usr/lib/fedora-release`. Existing NVRAM: `apex-debrand-runtime.sh` |
| 3 | `os-release` `NAME` / `PRETTY_NAME` | `Fedora Linux` / `Fedora Linux 43 (Forty Three)` | `APEX-OS` / `APEX-OS` | image | `Containerfile.base` os-release `sed` |
| 4 | `os-release` `VERSION` | `43 (Forty Three)` (Fedora codename) | `43` | image | same `sed` |
| 5 | `os-release` `LOGO` | `fedora-logo-icon` | `apex-os-logo` | image | same `sed`; the icon itself is installed into `hicolor` from `files/branding/logos/chartreuse/` |
| 6 | `os-release` `CPE_NAME` | `cpe:/o:fedoraproject:fedora:43` | `cpe:/o:apexos:apex_os:43` | image | same `sed`, plus `/usr/lib/system-release-cpe` |
| 7 | `os-release` `DOCUMENTATION_URL` / `SUPPORT_URL` | `docs.fedoraproject.org` / `ask.fedoraproject.org` | the apex-os GitHub repo | image | same `sed` |
| 8 | `os-release` `REDHAT_BUGZILLA_*` / `REDHAT_SUPPORT_*` | `"Fedora"` ×4 | deleted | image | `sed -e '/^REDHAT_/d'` |
| 9 | `os-release` `ANSI_COLOR`, `DEFAULT_HOSTNAME`, `HOME_URL`, `BUG_REPORT_URL` | Fedora blue / `fedora` / fedoraproject.org / bugzilla | APEX chartreuse / `apex` / apex-os repo | image | same `sed` (pre-existing) |
| 10 | `/etc/system-release`, `/etc/redhat-release`, `/etc/fedora-release` (all → `/usr/lib/fedora-release`) | `Fedora release 43 (Forty Three)` | `APEX-OS release 43` | image | `Containerfile.core`. **Content only — the file is NOT renamed**, the symlink chain and `[ -f /etc/fedora-release ]` probes must keep working, and this is the string bootupd turns into the firmware label. The image now **re-creates** those three links itself (plus `os-release`, the CPE and `issue{,.net}`) rather than inheriting them from the base layer, and asserts the branded string *through `/etc`*: writing only `/usr/lib` and trusting an inherited hardlinked symlink is what failed every weekly `core` build from 2026-08-17 on |
| 11 | VT login banner `/etc/issue`, `/etc/issue.net` (→ `/usr/lib/issue*`) | `\S` + `Kernel \r on \m (\l)` → printed `7.1.3-cachyos1.fc43.x86_64` | `\S` only → prints `APEX-OS` | image | `Containerfile.base`. `\S` is expanded by agetty from `PRETTY_NAME`. Writing `/usr/lib/issue` (not `/etc/issue`) keeps the symlink intact |
| 12 | `fastfetch` kernel line | `Linux 7.1.3-cachyos1.fc43.x86_64` | `7.1.3` | image | `files/system/fastfetch/config.jsonc` — the `kernel` module replaced by a `command` module running `uname -r \| cut -d- -f1` |
| 13 | `fastfetch` OS line + ASCII logo | Fedora `F` logo (auto-detected from `ID`) | APEX spark + `APEX-OS 43` | image (pre-existing) | `config.jsonc` pins `logo.source` to `/etc/fastfetch/apex-logo.txt`; bare `fastfetch` picks up `/etc/fastfetch/config.jsonc`, verified |
| 14 | Kernel-version stamp file | `/usr/lib/apex-cachyos-kver` | `/usr/lib/apex-kver` | image | `Containerfile.base` / `.daily` / `.gaming` |
| 15 | Installer completion screen | *"look for \"Fedora\" / \"APEX\""* | *"pick it from the one-time boot menu (F12 on ThinkPads)"* | image | `installer/apex-install` |
| 16 | Live-ISO GRUB menu | already `Install APEX-OS` | unchanged | image (pre-existing) | `installer/build-live-iso.sh` |
| 17 | Plymouth boot splash | `apex-os-chartreuse` on every machine (gold is source art only), wordmark `A P E X   O S` | unchanged | image (pre-existing) | `files/branding/plymouth/` |
| 18 | Greeter (`apex-greet`) | no distro string at all | unchanged | n/a | verified clean |
| 18a | `hostnamectl` "Operating System" | `Fedora Linux 43 (Forty Three)` | `APEX-OS` | image | reads os-release `PRETTY_NAME`; covered by surface 3. Its "Kernel:" line still shows the CachyOS release string — see the unfixable table |
| 18b | `neofetch` / `screenfetch` / `lsb_release` | — | — | n/a | **not installed** in the image (verified). `fastfetch` is the only fetch tool, and it is handled by surfaces 12–13. If one is ever layered in, it will auto-detect the Fedora logo from `ID` exactly as bare `fastfetch` would, and needs the same `logo.source` pin |
| 19 | Local `/etc/os-release` override on installed systems | a hand-written file with `VERSION="43 (Forty Three)"`, `LOGO=fedora-logo-icon`, `REDHAT_*`, fedora URLs | branded, and removed entirely once the image's own os-release is branded | **runtime** | `apex-debrand-runtime.sh`. See the warning below |

### The `/etc/os-release` trap (surface 19)

On the running install `/etc/os-release` is a **regular file**, not the image's
symlink into `/usr/lib`. On ostree, that is a local `/etc` modification, and
ostree **3-way-merges `/etc` forward into every future deployment**. So the
override outlives the image fix: after upgrading to a branded image it keeps
shadowing `/usr/lib/os-release` — pinning the stale `VERSION="43 (Forty Three)"`,
the Fedora `LOGO`/URLs, and worst, pinning `VERSION_ID=43` across a future rebase
to an F44 base (which would break `$releasever`).

`apex-debrand-runtime.sh` handles both states:

- image os-release already branded → **removes** the override and restores the
  symlink `../usr/lib/os-release`;
- image os-release not yet branded → **refreshes** the override with a fully
  branded copy and stamps it `TRANSITIONAL`, with a comment telling you to delete
  it after the next `bootc upgrade`.

## Why `ID=fedora` stays

`ID` and `VERSION_ID` are the only os-release keys left un-branded, and that is
deliberate. They are **machine-facing** — nothing that renders on screen reads
them (the boot menu, Plymouth, the greeter, fastfetch's `{name}`, and agetty's
`\S` all read `PRETTY_NAME`).

The textbook derivative pattern is `ID=apex` + `ID_LIKE="fedora"`. It was tested
and it **breaks the build**. `dnf copr` derives its chroot name from
`ID`-`VERSION_ID`-`arch`, and `ID_LIKE` does not help:

```
$ sed -i -e 's|^ID=.*|ID=apex|' -e '/^ID=apex/a ID_LIKE="fedora"' /usr/lib/os-release
$ dnf5 -y copr enable bieszczaders/kernel-cachyos
Chroot not found in the given Copr project (apex-43-x86_64).
You can choose one of the available chroots explicitly:
 …
 fedora-43-x86_64
```

That is three COPRs in the build path (`bieszczaders/kernel-cachyos`,
`bieszczaders/kernel-cachyos-addons`, `xxmitsu/mesa-git`) plus any COPR a user
enables later. `$releasever` was checked separately and is safe — it comes from
`VERSION_ID`, not `ID` (`dnf5 --dump-variables` → `releasever = 43` with
`ID=apex`), but that only narrows the blast radius, it does not remove it.

Verdict: **`ID=fedora` and `VERSION_ID=43` stay.** The user-visible mandate is
fully met without them. If this is ever revisited, every `dnf copr enable` call
site must be given an explicit `fedora-43-x86_64` chroot argument first, and the
whole build re-run.

`Containerfile.base` asserts this invariant at build time — the branding step
fails the build if any line other than `ID=fedora` still matches `fedora`:

```sh
test "$(grep -ci fedora /usr/lib/os-release)" = 1
grep -qx 'ID=fedora' /usr/lib/os-release
```

## What CANNOT be fixed

| Surface | Shows | Why it cannot change |
|---------|-------|----------------------|
| `uname -r`, `/usr/lib/modules/<kver>/`, `/usr/src/kernels/<kver>/`, `/usr/share/licenses/kernel-cachyos-core/`, `/proc/version` | `7.1.3-cachyos1.fc43.x86_64` | The release string is compiled into the kernel package (`CONFIG_LOCALVERSION` + the RPM dist tag). Renaming it means building the kernel from source, which is out of scope. **Mitigated**: it is hidden from the boot menu (BLS `title` never contains the kernel version), from the VT login banner (surface 11) and from fastfetch (surface 12). It remains visible to anyone who runs `uname -r`. |
| The ESP directory `EFI/fedora/` | `\EFI\fedora\shimx64.efi`, `/boot/efi/EFI/fedora/` | The path is **hardcoded inside Fedora's signed `grubx64.efi`** (its build-time prefix) and in `shim`'s fallback CSV. Renaming the directory breaks the Secure Boot chain — grub would not find its config and the machine would not boot. Only the **NVRAM label** can change, and it does (surface 2). Also note `installer/apex-install` deliberately keeps its "a Fedora-family install already uses `\EFI\fedora`" warning: that text is about a genuine neighbouring Fedora install and a genuine path, and making it say APEX would be a lie. |
| `os-release` `ID` / `VERSION_ID` | `fedora` / `43` | See "Why `ID=fedora` stays" above — verified build breakage. |
| `/etc/yum.repos.d/fedora*.repo`, `rpmfusion-*.repo`, `_copr:…kernel-cachyos*.repo` (left behind, `enabled=0`, by `dnf copr disable`), `rpm -E %fedora`, `%dist_vendor` | `Fedora`, `cachyos` | Package-manager plumbing pointing at real Fedora / COPR repositories. Renaming would break package resolution, and none of it is user-visible. |
| Engineering comments in `Containerfile.*`, `installer/build-live-iso.sh`, `apex-greet/README.md`, `mpv.conf` | `Fedora`, `CachyOS` | Load-bearing rationale ("Fedora ships crippled ffmpeg", "Fedora's signed grub has prefix /EFI/fedora"). These are developer-facing and accurate; scrubbing them would destroy the reasoning. |

## Fixing an already-installed system

Do the boot-menu change and the NVRAM change as **separate steps**, in this
order. The BLS rewrite has been tested against a copy of the real entries; the
NVRAM write has not (there is no way to dry-run firmware). Keeping them apart
means that if the firmware misbehaves on the relabel, you are already booted
through a verified-good boot menu instead of debugging two changes at once.

```sh
# 1. See what would change — writes nothing.
sudo /usr/libexec/apex-debrand-runtime            # or ./files/scripts/apex-debrand-runtime.sh

# 2. Boot menu titles + the /etc/os-release override. Reboot and confirm the
#    GRUB menu now reads "APEX-OS (ostree:N)".
sudo /usr/libexec/apex-debrand-runtime --apply --skip-efi

# 3. A second install on another partition has its own /boot but SHARES the ESP
#    (and therefore the single firmware boot entry) — do its BLS titles too.
sudo /usr/libexec/apex-debrand-runtime --apply --boot-dir /mnt/other/boot

# 4. Only now, the firmware boot entry label ("Fedora" -> "APEX-OS"). Run this
#    ONCE for the whole disk, not once per install.
sudo /usr/libexec/apex-debrand-runtime --apply --skip-bls --skip-os-release

# 5. AFTER the first `bootc upgrade` onto a branded image, run it once more.
#    /usr/lib/os-release is branded by then, so the script takes the other
#    branch: it deletes the transitional /etc/os-release override and restores
#    the image symlink. THIS STEP IS NOT OPTIONAL — skipping it leaves the
#    override merging forward forever, pinning VERSION_ID=43 across a future
#    rebase to an F44 base.
sudo /usr/libexec/apex-debrand-runtime --apply --skip-bls --skip-efi
```

Safety properties (all exercised against a copy of the real
`/boot/loader/entries`, see the script header):

- dry-run by default; `--apply` is required to write anything;
- an entry is validated before it is touched — exactly one `title` line, a
  `linux` line, and the referenced vmlinuz/initramfs must exist. A broken entry
  is skipped loudly and left untouched, and the exit code is non-zero;
- **only the `title` line is rewritten**; the rewrite is rejected unless every
  other line is byte-for-byte identical. `options`, `linux`, `initrd` and
  `version` are never reformatted;
- written with `cat >` (not `mv`), so inode, mode, owner and SELinux label are
  preserved;
- backups go to `/var/lib/apex-debrand/<timestamp>/`, deliberately **not** next
  to the entries — grub's `blscfg` globs `*.conf` and an in-place backup would
  show up as a phantom boot menu entry;
- `/boot` is remounted rw only if it is mounted ro, and restored on exit
  including on failure;
- `grubenv` is checked for a `saved_entry` that names a title (none on the
  current installs; the script prints the exact `grub2-editenv` fix if one
  appears);
- the EFI step creates and **verifies** the new entry (same ESP PARTUUID, same
  loader) *before* deleting the old one, and restores `BootOrder` with the new
  entry in the old entry's position. If any step fails, the original entry is
  left alone and the machine still boots;
- idempotent — a second run reports "already branded" and writes nothing.
