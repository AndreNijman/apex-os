# APEX-OS Plymouth Themes

Boot splash for APEX-OS. **`apex-os-chartreuse` is the default** and the source
of every other theme: `Containerfile.apex` installs it plus the 24
`apex-os-accent-NN` themes (one per 15° of hue), and the initramfs starts the
splash in the one matching the owner's matugen accent
(`files/dracut/apex-plymouth-accent`). `apex-os-gold` is source art for the
other colourway and is installed by nothing; it dates from when Gaming was a
separate edition.

**Animation -- "Focus":** on a plain black screen the spark comes into focus
out of a soft glow of its own light (a wide blur, a closer blur, then the sharp
logo), a halo blooms behind it and settles, and the tracked-out `APEX OS`
wordmark fades in beneath. While the machine boots, the halo breathes on a
5.2 s period. Shutdown, reboot and update modes show the settled splash from
the first frame. At a LUKS prompt the splash dims, the wordmark gives way to
the prompt, each typed character adds an accent-coloured dot (the row
re-centres on a quick spring) and messages such as the keyboard layout appear
under the dots. Typed characters are never drawn.

Why it is built the way it is -- measured, not assumed, in a real `plymouthd`
(x11 renderer in a private Xvfb, an LD_PRELOAD frame probe): plymouth has no
vsync, truncates sprite positions to whole pixels, re-samples an image on
every `Image.Scale`/`Rotate`, and redraws a sprite only when its opacity moves
by more than 0.01. So every image is scaled once when the script starts,
nothing moves, and the refresh only changes opacities, on curves of real
elapsed time that start and end at rest. The header of `apex-os.script` has the
numbers for the "Convergence" animation this replaces.

## Files

| file | what |
|---|---|
| `apex-os-chartreuse/apex-os.script` | the splash; every other theme's script is this file with its own highlight line |
| `make-splash-art.sh chartreuse\|gold` | draws a colourway's images: the logo as 1x and `-hd` masters, the two blurred sparks, the halo, the bullet, the wordmark and the fallback prompt (ImageMagick 7, JetBrains Mono) |
| `make-accent-themes.sh` | hue-rotates the chartreuse images into the 24 accent themes and sets each script's highlight |
| `render-preview.py` | frame-exact offline render of the script on plymouth's arithmetic: 60 fps MP4, contact sheet, per-frame metrics |
| `render-preview.sh` | the old command: `./render-preview.sh <theme-dir> <out.gif>` |

After changing the script or the art, rerun `make-splash-art.sh chartreuse`,
`make-splash-art.sh gold` (it copies the script too), `make-accent-themes.sh`,
and the previews; `tests/test-apex-plymouth-accent.sh` fails if the accents
drift from the chartreuse script, if the preview's port of the curves goes
stale, or if a per-frame `Image.Scale`/`Rotate`, a frame counter, a gradient or
a sub-60 Hz refresh comes back.

Every file in a theme lands in the initramfs 25 times and PNG does not shrink
under zstd, so keep the image set small (43 KB per theme today).

## Previews

`previews/preview-chartreuse.gif`, `previews/preview-gold.gif` (30 fps). For the
full-rate version:

```sh
./render-preview.py apex-os-chartreuse /path/to/out --seconds 6              # boot
./render-preview.py apex-os-chartreuse /path/to/out --seconds 10 --password  # LUKS prompt
./render-preview.py apex-os-chartreuse /path/to/out --mode shutdown
```

Needs python3 with numpy and Pillow, and ffmpeg. Never test a splash with
`plymouthd` on the machine you are using: it takes over the VT and the DRM
device. Render it offline, or run `plymouthd` with its x11 renderer inside a
container with its own Xvfb and no access to `/dev/dri` or the host's display.

## Install (in the image build)

`Containerfile.apex` copies the themes into `/usr/share/plymouth/themes/`,
sets `apex-os-chartreuse` as the default and rebuilds the initramfs with
`--add "plymouth apex-plymouth-accent"`; kernel args need `quiet splash`.
Text (the prompt, messages) needs `plymouth-plugin-label`; the build asserts
`label-freetype.so` and `Plymouth.ttf` reach the initramfs, and if they ever do
not, the script shows a pre-rendered "Enter passphrase to unlock" instead of
an invisible prompt.
