#!/usr/bin/env bash
# Generate the 24 accent themes of the APEX boot splash.
#
# Plymouth's script plugin cannot tint an image, and a theme has to be chosen
# BEFORE plymouthd starts for its first frame to be the right colour. So each of
# 24 hues (0°, 15°, … 345°) is a complete theme, apex-os-accent-NN, built from
# the shipped chartreuse one (#D9F99D, hue 80.9°): the three coloured images
# hue-rotated, flash.png (white) copied, and the script's highlight colour set
# to the chartreuse rotated to the same hue. A matugen primary is a light,
# saturated colour like the chartreuse, so hue is the part that differs.
#
# The result is committed; rerun only when the source theme changes.
set -euo pipefail
cd "$(dirname "$0")"
SRC=apex-os-chartreuse
SRC_HUE=80.9
for n in $(seq 0 23); do
    nn=$(printf '%02d' "$n"); name="apex-os-accent-$nn"
    rm -rf "$name"; mkdir -p "$name"
    h=$(awk -v n="$n" -v s="$SRC_HUE" 'BEGIN { printf "%.4f", 100 + (n*15 - s)*100/180 }')
    for img in comet glow spark; do
        magick "$SRC/$img.png" -modulate "100,100,$h" -strip \
            -define png:exclude-chunks=date,time "$name/$img.png"
    done
    cp "$SRC/flash.png" "$name/flash.png"
    hi=$(python3 -c "import colorsys
h,l,s=colorsys.rgb_to_hls(0xD9/255,0xF9/255,0x9D/255)
r,g,b=colorsys.hls_to_rgb(($n*15/360)%1,l,s)
print(f'HI_R = {r:.3f}; HI_G = {g:.3f}; HI_B = {b:.3f};   # hue {$n*15}° (apex-os-accent-$nn)')")
    sed -E "s|^HI_R = .*|${hi}|" "$SRC/apex-os.script" > "$name/apex-os.script"
    cat > "$name/$name.plymouth" <<THEME
[Plymouth Theme]
Name=APEX-OS accent $nn
Description=APEX-OS boot splash, hue $((n*15))° (follows the owner's matugen accent)
ModuleName=script

[script]
ImageDir=/usr/share/plymouth/themes/$name
ScriptFile=/usr/share/plymouth/themes/$name/apex-os.script
THEME
done
