#!/bin/bash
# Render a preview GIF of the APEX-OS "Convergence" plymouth animation.
# Reproduces the .script animation math (comet trails, easing, spark morph)
# in awk + ImageMagick. Usage: render-preview.sh <theme-dir> <out.gif>
set -e
DIR=$1; OUT=$2
TMP=$(mktemp -d)
W=960; H=600; BASE=156

for t in $(seq 1 110); do
  awk -v t=$t -v base=$BASE 'BEGIN{
    pi=3.14159265; r0=base*0.95; cw=base*0.38; ntrail=9;
    # ---- glow / flash (z: under comets) ----
    if (t>=38 && t<50)      { gop=(t-38)/12*0.85; gs=base*(0.3+0.3*(t-38)/12); }
    else if (t>=50 && t<=62){ gop=1-(t-50)/12;    gs=base*(0.6+1.4*(t-50)/12); }
    else                    { gop=0; gs=2; }
    printf "G %.3f %.0f\n", gop, gs;
    # ---- motion primitives ----
    # angular position: quadratic ease-in acceleration
    # theta(t) = 0.06 t + 0.0022 t^2
    # radius: constant orbit, then quartic-ish accelerating contraction
    # entrance: swoop from 1.5x radius easing down during first 10 frames
    # trail: soft ribbon sampled from past positions, stretching with speed
    om = 0.06 + 0.0044*t; st = om/0.15; if (st<0.8) st=0.8; if (st>2.0) st=2.0;
    if (t<10) cop=t/10; else if (t>46) { cop=(52-t)/6; if(cop<0)cop=0; } else cop=1;
    if (t>54) cop=0;
    for (i=0;i<4;i++){
      for (k=0;k<=ntrail;k++){
        tt = t - k*0.22*st; if (tt<0) tt=0;
        th = 0.06*tt + 0.0022*tt*tt + i*pi/2;
        if (tt<30) R=r0; else { p=(tt-30)/22; if(p>1)p=1; R=r0*(1-p)*(1-p); }
        if (tt<10) R=R*(1 + 0.5*(1-tt/10)*(1-tt/10));   # swoop-in entrance
        x=R*cos(th); y=R*sin(th)-36;
        # velocity direction (screen coords, y down) sampled half a frame apart
        t1=tt+0.5; th1=0.06*t1+0.0022*t1*t1+i*pi/2;
        if (t1<30) R1=r0; else { p=(t1-30)/22; if(p>1)p=1; R1=r0*(1-p)*(1-p); }
        if (t1<10) R1=R1*(1+0.5*(1-t1/10)*(1-t1/10));
        t2=tt-0.5; if(t2<0)t2=0; th2=0.06*t2+0.0022*t2*t2+i*pi/2;
        if (t2<30) R2=r0; else { p=(t2-30)/22; if(p>1)p=1; R2=r0*(1-p)*(1-p); }
        if (t2<10) R2=R2*(1+0.5*(1-t2/10)*(1-t2/10));
        vx=R1*cos(th1)-R2*cos(th2); vy=R1*sin(th1)-R2*sin(th2);
        deg=atan2(vy,vx)*180/pi;
        f=(1-k/(ntrail+1));
        if (k==0) {
          printf "T %.0f %.0f %.2f %.0f %.3f\n", x, y, deg, cw, cop;   # comet head
        } else {
          op=cop*0.55*f*f;                      # ribbon fades quadratically
          sz=cw*0.30*(0.25+0.75*f);             # ribbon tapers to a point
          printf "R %.0f %.0f %.0f %.3f\n", x, y, sz, op;
        }
      }
    }
    # ---- spark morph: back-out overshoot + rotation settle ----
    sop=0; ss=4; sdeg=0; fop=0;
    if (t>=50 && t<=70) {
      p=(t-50)/20; if(p>1)p=1; q=p-1;
      s=1+2.70158*q*q*q+1.70158*q*q; rot=-2.4*q*q*q;
      ss=base*s; if(ss<4)ss=4; sdeg=rot*180/pi;
      if (p<0.34) sop=p/0.34; else sop=1;
      fop=(1-p)*(1-p)*0.9;
    } else if (t>70) { ss=base*(1+0.02*sin((t-70)/9)); sop=0.93+0.07*sin((t-70)/9); sdeg=0; }
    printf "S %.3f %.0f %.2f %.3f\n", sop, ss, sdeg, fop;
    # ---- wordmark ----
    wmop=0; wy=12;
    if (t>=62 && t<=84) { p=(t-62)/22; q=1-p; wmop=1-q*q; wy=q*q*12; }
    else if (t>84) { wmop=1; wy=0; }
    printf "W %.3f %.2f\n", wmop, wy;
  }' > "$TMP/v.txt"

  CMD="magick -size ${W}x${H} xc:'#0B0E14'"
  GZ=""; RZ=""; TZ=""; SZ=""; WZ=""
  while read -r kind a b c d e; do
    case $kind in
      T) [ "$e" != "0.000" ] && TZ="$TZ \( '$DIR/comet.png' -resize ${d}x${d} -background none -rotate $c -channel A -evaluate multiply $e +channel \) -gravity center -geometry +${a}+${b} -composite";;
      R) [ "$d" != "0.000" ] && RZ="$RZ \( '$DIR/glow.png' -resize ${c}x${c} -channel A -evaluate multiply $d +channel \) -gravity center -geometry +${a}+${b} -composite";;
      G) [ "$a" != "0.000" ] && GZ="$GZ \( '$DIR/glow.png' -resize ${b}x${b} -channel A -evaluate multiply $a +channel \) -gravity center -geometry +0-36 -composite";;
      S) [ "$a" != "0.000" ] && SZ="$SZ \( '$DIR/spark.png' -resize ${b}x${b} -background none -rotate $c -channel A -evaluate multiply $a +channel \) -gravity center -geometry +0-36 -composite"
         [ "$d" != "0.000" ] && SZ="$SZ \( '$DIR/flash.png' -resize ${b}x${b} -background none -rotate $c -channel A -evaluate multiply $d +channel \) -gravity center -geometry +0-36 -composite";;
      W) [ "$a" != "0.000" ] && WZ="$WZ -gravity center -pointsize 26 -fill 'rgba(216,222,235,$a)' -annotate +0+$(awk -v aa=78 -v bb=$b 'BEGIN{printf "%.0f", aa+bb}') 'A P E X   O S'";;
    esac
  done < "$TMP/v.txt"
  CMD="$CMD$GZ$RZ$TZ$SZ$WZ '$(printf '%s/f-%03d.png' "$TMP" "$t")'"
  eval "$CMD"
done
magick -delay 3 -loop 0 "$TMP"/f-*.png -resize 720x450 -layers optimize "$OUT"
rm -rf "$TMP"
echo "wrote $OUT"
