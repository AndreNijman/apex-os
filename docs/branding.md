# APEX-OS Branding

## The mark: "spark"

APEX-OS uses a single logomark — the **spark** — rendered in edition-specific
colorways. The wordmark is the letterspaced "APEX OS".

## Color semantics

| Colorway | Hex (highlight) | Edition | Represents |
|----------|-----------------|---------|------------|
| **Gold** | `#FDE047` | APEX-OS **Gaming** | Power |
| **Chartreuse** | `#D9F99D` | APEX-OS **Daily** | Everyday |
| **Mono (black)** | — | neutral | Light backgrounds, print, single-color contexts |
| **Mono (white)** | — | neutral | Dark backgrounds, single-color contexts |

Gold always denotes the Gaming edition; chartreuse always denotes the Daily
edition. The mono variants exist for any context that needs a neutral,
single-color mark and must not imply an edition.

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

Two themes, one per edition:

- `apex-os-gold/` — Gaming edition boot splash
- `apex-os-chartreuse/` — Daily edition boot splash

Each theme contains its `.plymouth` descriptor, the shared `apex-os.script`
animation, and the sprite images `spark.png`, `comet.png`, `glow.png`,
`flash.png`.

**Animation — "Convergence":** four comets orbit and accelerate, spiral into
the center, flash, and morph into the spark; the wordmark rises in underneath
and the spark holds with a subtle breathing loop. LUKS password prompts are
handled (theme dims, prompt + bullets shown); shutdown shows a static spark.

Install (inside the image build):

```sh
cp -r apex-os-gold /usr/share/plymouth/themes/
plymouth-set-default-theme -R apex-os-gold   # -R rebuilds the initramfs
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
