#!/bin/bash
# Render a preview GIF of the APEX-OS "Convergence" plymouth animation.
# Usage: render-preview.sh <theme-dir> <out.gif>
set -e
DIR=$1; OUT=$2
TMP=$(mktemp -d)
W=960; H=600

for t in $(seq 1 110); do
  read -a V < <(awk -v t=$t -v base=156 'BEGIN{
    pi=3.14159265; r0=base*0.95;
    theta=0.06*t+0.0022*t*t;
    if (t<30) R=r0; else { p=(t-30)/22; if(p>1)p=1; R=r0*(1-p)*(1-p); }
    if (t<10) cop=t/10; else if (t>46) { cop=(52-t)/6; if(cop<0)cop=0; } else cop=1;
    if (t>54) cop=0;
    out="";
    for (i=0;i<4;i++){
      a=theta+i*pi/2; dx=R*cos(a); dy=R*sin(a); phi=a+pi/2; deg=phi*180/pi;
      out=out sprintf(" %.0f %.0f %.2f", dx, dy-36, deg);
    }
    if (t>=38 && t<50)      { gop=(t-38)/12*0.85; gs=base*(0.3+0.3*(t-38)/12); }
    else if (t>=50 && t<=62){ gop=1-(t-50)/12;    gs=base*(0.6+1.4*(t-50)/12); }
    else                    { gop=0; gs=2; }
    sop=0; ss=4; sdeg=0; fop=0;
    if (t>=50 && t<=70) {
      p=(t-50)/20; if(p>1)p=1; q=p-1;
      s=1+2.70158*q*q*q+1.70158*q*q; rot=-2.4*q*q*q;
      ss=base*s; if(ss<4)ss=4; sdeg=rot*180/pi;
      if (p<0.34) sop=p/0.34; else sop=1;
      fop=(1-p)*(1-p)*0.9;
    } else if (t>70) { ss=base; sop=0.93+0.07*sin((t-70)/9); sdeg=0; }
    wmop=0; wy=12;
    if (t>=62 && t<=84) { p=(t-62)/22; q=1-p; wmop=1-q*q; wy=q*q*12; }
    else if (t>84) { wmop=1; wy=0; }
    printf "%.3f%s %.3f %.0f %.3f %.0f %.2f %.3f %.3f %.2f\n",
           cop, out, gop, gs, sop, ss, sdeg, fop, wmop, wy;
  }')
  COP=${V[0]}; GOP=${V[13]}; GS=${V[14]}; SOP=${V[15]}; SS=${V[16]}; SDEG=${V[17]}; FOP=${V[18]}; WMOP=${V[19]}; WY=${V[20]}

  CMD="magick -size ${W}x${H} xc:'#0B0E14'"
  if [ "$GOP" != "0.000" ]; then
    CMD="$CMD \( '$DIR/glow.png' -resize ${GS}x${GS} -channel A -evaluate multiply $GOP +channel \) -gravity center -geometry +0-36 -composite"
  fi
  if [ "$COP" != "0.000" ]; then
    for c in 0 1 2 3; do
      i=$((1 + c*3))
      CMD="$CMD \( '$DIR/comet.png' -resize 59x59 -background none -rotate ${V[$((i+2))]} -channel A -evaluate multiply $COP +channel \) -gravity center -geometry +${V[$i]}+${V[$((i+1))]} -composite"
    done
  fi
  if [ "$SOP" != "0.000" ]; then
    CMD="$CMD \( '$DIR/spark.png' -resize ${SS}x${SS} -background none -rotate $SDEG -channel A -evaluate multiply $SOP +channel \) -gravity center -geometry +0-36 -composite"
  fi
  if [ "$FOP" != "0.000" ]; then
    CMD="$CMD \( '$DIR/flash.png' -resize ${SS}x${SS} -background none -rotate $SDEG -channel A -evaluate multiply $FOP +channel \) -gravity center -geometry +0-36 -composite"
  fi
  if [ "$WMOP" != "0.000" ]; then
    CMD="$CMD -gravity center -pointsize 26 -fill 'rgba(216,222,235,$WMOP)' -annotate +0+$(awk -v a=78 -v b=$WY 'BEGIN{printf "%.0f", a+b}') 'A P E X   O S'"
  fi
  CMD="$CMD '$(printf '%s/f-%03d.png' "$TMP" "$t")'"
  eval "$CMD"
done
magick -delay 3 -loop 0 "$TMP"/f-*.png -resize 720x450 -layers optimize "$OUT"
rm -rf "$TMP"
echo "wrote $OUT"
