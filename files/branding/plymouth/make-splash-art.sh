#!/usr/bin/env bash
# Render the boot splash's image masters for one colourway.
#
#   make-splash-art.sh chartreuse     -> rime-os-chartreuse/*.png
#   make-splash-art.sh gold           -> rime-os-gold/*.png
#
# Then run make-accent-themes.sh, which hue-rotates the chartreuse set into the
# 24 accent themes. Both outputs are committed; rerun only when the art changes.
#
# Every image here is drawn ONCE at splash start (rime-os.script scales each to
# the screen a single time and never again), so the per-frame work of the
# splash is opacity changes on fixed images. That is the point of the whole
# design: plymouth has no vsync, truncates sprite positions to whole pixels, and
# re-samples an image on every Image.Scale()/Rotate(), so anything that moves or
# resizes per frame steps and jitters. Light that fades does not.
#
# Sizes are chosen so the script's one-time scale stays within ~0.7x-1.4x of a
# master on 1080p-4K screens (plymouth's resize is bilinear with no filtering:
# a large downscale aliases, a large upscale blurs), which is why the spark,
# wordmark, bullet and fallback prompt come in a 1x and an -hd master.
#
# Every file in a theme directory lands in the initramfs 25 times (24 accents +
# chartreuse), and PNG does not shrink under zstd: keep the set small.
#
# Needs ImageMagick 7 (magick) and the JetBrains Mono fonts (the shell's face).
set -euo pipefail
cd "$(dirname "$0")"

WAY=${1:?usage: make-splash-art.sh chartreuse|gold}
case "$WAY" in
    chartreuse) LIGHT='#D9F99D'; DEEP='#65A30D' ;;   # the logo's gradient stops
    gold)       LIGHT='#FDE047'; DEEP='#F59E0B' ;;
    *) echo "unknown colourway $WAY" >&2; exit 2 ;;
esac
LOGOS=../logos/$WAY
OUT=rime-os-$WAY
FONT=$(fc-match -f '%{file}' 'JetBrains Mono:medium')
case "$FONT" in *JetBrainsMono-Medium*) ;; *) echo "JetBrains Mono Medium not found (got $FONT)" >&2; exit 1 ;; esac

TEXT='#D9DEEB'      # wordmark, the same neutral the shell and installer use
PROMPT='#D2D6DE'    # fallback prompt, matches the script's Image.Text colour
PNG=(-strip -define 'png:exclude-chunks=date,time')
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$OUT"
rm -f "$OUT"/comet.png "$OUT"/glow.png "$OUT"/flash.png

# ── the spark: the brand logo, unchanged, as 8-bit masters ───────────────────
magick "$LOGOS/rime-spark-$WAY-256.png" -depth 8 "${PNG[@]}" "$OUT/spark.png"
magick "$LOGOS/rime-spark-$WAY-512.png" -depth 8 "${PNG[@]}" "$OUT/spark-hd.png"

# ── the spark out of focus, at two depths ────────────────────────────────────
# The logo's alpha, blurred, filled with the logo's own vertical gradient (its
# stops span the star's height, y 82..430 of 512). A smooth colour field keeps
# these small and matches the sharp spark's colours through the crossfade. The
# canvas is 1.25x the spark's (640 for 512) so the blur is not clipped; the
# script draws these at 1.25x the spark's size.
magick -size 640x348 "gradient:$LIGHT-$DEEP" \( -size 640x146 "xc:$LIGHT" \) +swap \
    \( -size 640x146 "xc:$DEEP" \) -append "$TMP/field.png"
blur() {   # blur <sigma at 512> <output size> <out>
    magick "$LOGOS/rime-spark-$WAY-512.png" -background none -gravity center \
        -extent 640x640 -alpha extract -blur "0x$1" "$TMP/a.png"
    magick "$TMP/field.png" "$TMP/a.png" -alpha off -compose CopyOpacity -composite \
        -resize "$2x$2" -depth 8 "${PNG[@]}" "$3"
}
blur 26 128 "$OUT/spark-blur.png"
blur 8  256 "$OUT/spark-soft.png"

# ── the halo: a soft gaussian of the spark's light colour ────────────────────
# alpha = 0.5 * exp(-r^2 / 0.18) * (1 - r^2) of the radius r (0 centre, 1 edge):
# a gaussian that reaches exactly zero at the edge, so the square never shows.
# Half-strength on purpose: plymouth only redraws a sprite when its opacity
# moves by more than 0.01, so the halo's slow breathing lands in steps of 0.01
# of its brightest pixel; at half strength a step is about one grey level.
# Smooth enough to upscale ~3x with plymouth's bilinear resize.
magick -size 192x192 xc: -fx \
    'dd=hypot(i-95.5,j-95.5)/96; dd<1 ? 0.5*exp(-dd*dd/0.18)*(1-dd*dd) : 0' "$TMP/halo-a.png"
magick -size 192x192 "xc:$LIGHT" "$TMP/halo-a.png" -alpha off \
    -compose CopyOpacity -composite -depth 8 "${PNG[@]}" "$OUT/halo.png"

# ── password bullets: a dot in the spark's light colour ──────────────────────
for s in 16:bullet 32:bullet-hd; do
    n=${s%%:*}; name=${s#*:}
    magick -size 256x256 xc:none -fill "$LIGHT" -draw 'circle 127.5,127.5 127.5,8' \
        -resize "${n}x${n}" -depth 8 "${PNG[@]}" "$OUT/$name.png"
done

# ── the wordmark: Rime OS, tracked out, in the shell's typeface ──────────────
word() {   # word <pointsize> <tracking> <gap> <out>
    magick -background none -fill "$TEXT" -font "$FONT" -pointsize "$1" \
        -kerning "$2" label:Rime -trim +repage "$TMP/w1.png"
    magick -background none -fill "$TEXT" -font "$FONT" -pointsize "$1" \
        -kerning "$2" label:OS -trim +repage "$TMP/w2.png"
    magick "$TMP/w1.png" \( -size "$3x1" xc:none \) "$TMP/w2.png" \
        -background none -gravity south +append -depth 8 "${PNG[@]}" "$4"
}
word 21 9 26 "$OUT/wordmark.png"
word 42 18 52 "$OUT/wordmark-hd.png"

# ── the prompt, for an initramfs whose Image.Text draws nothing ──────────────
# Image.Text() needs a label plugin and returns a 0x0 image without one; an
# encrypted machine whose prompt is invisible is the stranding case this repo
# has already shipped once. The script draws this instead when that happens.
for s in 17:prompt 34:prompt-hd; do
    n=${s%%:*}; name=${s#*:}
    magick -background none -fill "$PROMPT" -font "$FONT" -pointsize "$n" \
        label:'Enter passphrase to unlock' -trim +repage -depth 8 "${PNG[@]}" "$OUT/$name.png"
done

# The script is written once, in the chartreuse theme; the gold one is the
# same file with the gold highlight colour.
if [ "$WAY" = gold ]; then
    sed -E 's|^HI_R = .*|HI_R = 0.992; HI_G = 0.878; HI_B = 0.278;   # #FDE047 (gold highlight)|' \
        rime-os-chartreuse/rime-os.script > "$OUT/rime-os.script"
fi

du -cb "$OUT"/*.png | tail -1 | awk -v o="$OUT" '{ printf "%s: %d bytes of images\n", o, $1 }'
