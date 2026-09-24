#!/usr/bin/env bash
# Generate the 24 accent variants of the APEX boot splash.
#
# Plymouth's script plugin cannot tint an image at boot, so the colour has to be
# baked into the art. The shipped art is chartreuse (#D9F99D, hue 80.9°); each
# variant rotates its hue to one of 24 buckets (0°, 15°, … 345°), which keeps the
# glow's white core and the art's lightness — a matugen primary is a light,
# saturated colour like the chartreuse, so hue is the part that differs.
# flash.png is white and needs no variant.
#
# Output: apex-os-chartreuse/accent/NN/{comet,glow,spark}.png, NN = 00..23.
# The result is committed; rerun only when the source art changes.
set -euo pipefail
cd "$(dirname "$0")/apex-os-chartreuse"
SRC_HUE=80.9
for n in $(seq 0 23); do
    d=$(printf 'accent/%02d' "$n"); mkdir -p "$d"
    # ImageMagick -modulate hue: 100 is unchanged, and 200 is +180°.
    h=$(awk -v n="$n" -v s="$SRC_HUE" 'BEGIN { d = n*15 - s; printf "%.4f", 100 + d*100/180 }')
    for img in comet glow spark; do
        magick "$img.png" -modulate "100,100,$h" -strip -define png:exclude-chunks=date,time "$d/$img.png"
    done
done
