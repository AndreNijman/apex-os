# APEX-OS Plymouth Themes

Boot splash for APEX-OS, in two colorways: `apex-os-gold` and `apex-os-chartreuse`.

**Animation -- "Convergence":** 4 comets orbit with speed trails and accelerate,
spiral into the center (anticipation), flash, and morph into the spark
(back-out overshoot + rotation settle). The letterspaced "APEX OS" wordmark
rises in underneath; the spark then holds with a subtle breathing loop.
LUKS password prompts are handled (theme dims, prompt + bullets shown).
Static spark on shutdown.

Previews: `previews/preview-gold.gif`, `previews/preview-chartreuse.gif`
(regenerate with `./render-preview.sh <theme-dir> <out.gif>`).

## Install (on the target system / in the image build)

```sh
cp -r apex-os-gold /usr/share/plymouth/themes/
plymouth-set-default-theme -R apex-os-gold    # -R rebuilds initramfs
```

In the APEX-OS Containerfile the theme is copied into
`/usr/share/plymouth/themes/` and set as default; kernel args need
`quiet splash`.

## Test without rebooting

```sh
sudo plymouthd --debug --tty=/dev/tty1
sudo plymouth --show-splash
# ...wait, then:
sudo plymouth --quit
```
