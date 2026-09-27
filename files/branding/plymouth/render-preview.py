#!/usr/bin/env python3
"""Render the APEX boot splash offline, as plymouth would draw it.

    render-preview.py THEME_DIR OUT_DIR [--res 1920x1200] [--seconds 6]
                      [--mode boot|shutdown] [--password] [--refresh-log LOG]
                      [--gif PATH] [--gif-width 600]

A frame-exact simulation of apex-os.script on plymouth's script plugin, so the
splash can be judged without booting and without touching a real display:

  - the script's logic is ported line for line below (keep them in step: the
    `class Splash` methods carry the script's function names);
  - images are loaded, premultiplied, scaled and alpha-blended with the same
    8-bit integer arithmetic as libply-splash-core's ply-pixel-buffer.c
    (bilinear corner-aligned resize, truncated opacity byte, rounded blend);
  - sprite positions are truncated to whole pixels, sprites under 0.011
    opacity are skipped, and a sprite only triggers a redraw when it moved,
    changed image, or its opacity moved by more than 0.01 since it was last
    drawn -- plymouth's own rules, which is what makes stepping visible here
    if a design has it;
  - the refresh runs at the script's RATE and the display is sampled at 60 Hz.
    With --refresh-log (a probe log from a real plymouthd) the refreshes
    happen at the measured times instead, and boot-progress callbacks arrive
    at 30 Hz of real time, as plymouth sends them -- so the script's clock is
    exercised exactly as in the daemon.

Writes OUT_DIR/preview.mp4 (60 fps), OUT_DIR/contact.png, OUT_DIR/metrics.csv,
OUT_DIR/metrics.png (if matplotlib is present) and, with --gif, a GIF.
Needs python3 with numpy and Pillow, and ffmpeg.
"""
import argparse, csv, math, os, re, subprocess, sys
import numpy as np
from PIL import Image as PILImage, ImageDraw, ImageFont

# ── plymouth's pixel arithmetic ──────────────────────────────────────────────

def load_png(path):
    """ply-image.c: RGBA8 -> premultiplied, each channel truncated."""
    im = PILImage.open(path).convert("RGBA")
    a = np.asarray(im).astype(np.float64)
    al = a[..., 3:4]
    rgb = np.floor(np.clip((a[..., :3] / 255.0) * (al / 255.0) * 255.0, 0, 255))
    return np.concatenate([rgb, al], axis=2).astype(np.uint8)

def resize(img, nw, nh):
    """ply_pixel_buffer_resize: bilinear, corner-aligned, no filtering."""
    h, w = img.shape[:2]
    sx = (w - 1) / max(nw - 1, 1)
    sy = (h - 1) / max(nh - 1, 1)
    ox = np.arange(nw) * sx
    oy = np.arange(nh) * sy
    ix = ox.astype(np.int64); iy = oy.astype(np.int64)
    fx = (ox - ix)[None, :, None]; fy = (oy - iy)[:, None, None]
    ix1 = np.minimum(ix + 1, w - 1); iy1 = np.minimum(iy + 1, h - 1)
    ix = np.minimum(ix, w - 1); iy = np.minimum(iy, h - 1)
    f = img.astype(np.float64)
    p00 = f[iy][:, ix]; p01 = f[iy][:, ix1]; p10 = f[iy1][:, ix]; p11 = f[iy1][:, ix1]
    v = p00 * (1 - fx) * (1 - fy) + p01 * fx * (1 - fy) + p10 * (1 - fx) * fy + p11 * fx * fy
    return np.floor(v + 1e-9).astype(np.uint8)

def blend_into(fb, src, x0, y0, opacity, clip):
    """ply_pixel_buffer_fill_with_argb32_data_at_opacity_with_clip over an
    opaque canvas: make_pixel_value_translucent + blend_two_pixel_values."""
    cx0, cy0, cx1, cy1 = clip
    h, w = src.shape[:2]
    X0 = max(x0, cx0); Y0 = max(y0, cy0)
    X1 = min(x0 + w, cx1); Y1 = min(y0 + h, cy1)
    if X0 >= X1 or Y0 >= Y1:
        return
    s = src[Y0 - y0:Y1 - y0, X0 - x0:X1 - x0].astype(np.int32)
    ob = int(opacity * 255.0) & 0xFF
    if ob != 255:
        t = s * ob
        s = ((t + (t >> 8) + 0x80) >> 8) & 0xFF
    a = s[..., 3:4]
    d = fb[Y0:Y1, X0:X1].astype(np.int32)
    t = s[..., :3] * 255 + d * (255 - a)
    fb[Y0:Y1, X0:X1] = ((t + (t >> 8) + 0x80) >> 8).astype(np.uint8)

class Sprite:
    def __init__(self, image=None):
        self.image = image
        self.x = self.y = self.z = 0.0
        self.opacity = 1.0
        self.old = None            # (x, y, z, w, h, opacity) as last drawn
        self.refresh_me = image is not None
    def SetImage(self, img):
        self.image = img; self.refresh_me = True
    def SetPosition(self, x, y, z):
        self.x, self.y, self.z = float(x), float(y), float(z)
    def SetOpacity(self, o):
        self.opacity = float(o)

class Screen:
    """script-lib-sprite.c: dirty-region redraw over a solid background."""
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.fb = np.zeros((h, w, 3), np.uint8)
        self.sprites = []
    def sprite(self, image=None):
        s = Sprite(image); self.sprites.append(s); return s
    def refresh(self):
        rects = []
        for s in self.sprites:
            if s.image is None:
                continue
            ih, iw = s.image.shape[:2]
            o = s.old
            if (o is None or s.x != o[0] or s.y != o[1] or s.z != o[2]
                    or abs(o[5] - s.opacity) > 0.01 or s.refresh_me):
                rects.append((int(s.x), int(s.y), int(s.x) + iw, int(s.y) + ih))
                if o is not None:
                    rects.append((int(o[0]), int(o[1]), int(o[0]) + o[3], int(o[1]) + o[4]))
                s.old = (s.x, s.y, s.z, iw, ih, s.opacity)
                s.refresh_me = False
        order = sorted(self.sprites, key=lambda s: s.z)
        for r in rects:
            clip = (max(r[0], 0), max(r[1], 0), min(r[2], self.w), min(r[3], self.h))
            if clip[0] >= clip[2] or clip[1] >= clip[3]:
                continue
            self.fb[clip[1]:clip[3], clip[0]:clip[2]] = 0      # Window background: black
            for s in order:
                if s.image is None or s.opacity < 0.011:
                    continue
                blend_into(self.fb, s.image, int(s.x), int(s.y), s.opacity, clip)
        return len(rects)

# ── the script, ported ──────────────────────────────────────────────────────
# Every method below mirrors the function of the same name in apex-os.script.

def odd(v): return 2 * math.floor(v / 2) + 1
def clamp01(x): return 0.0 if x < 0 else 1.0 if x > 1 else x
def smooth(x):
    u = clamp01(x); return u * u * u * (u * (u * 6 - 15) + 10)
def expn(x):
    if x <= 0: return 1.0
    if x > 40: return 0.0
    y = 1 - x / 1024
    for _ in range(10): y = y * y
    return y
def spring(t, wn):
    if t <= 0: return 0.0
    x = wn * t; return 1 - (1 + x) * expn(x)
def bump(t, tp):
    if t <= 0: return 0.0
    u = t / tp; return 7.3890561 * u * u * expn(2 * u)

# The lines of apex-os.script this port reproduces. If any is missing, the
# script was retuned without the port and a preview would show the old curves:
# refuse rather than render something the splash does not do.
PORTED = [
    "S = odd(short * 0.18);", "B = odd(S * 1.25);", "HALO = odd(S * 2.3);",
    "spark_img = spark_src.Scale(S, S);",
    "cy = Math.Int(h * 0.46);", "T0 = 0.12;",
    "local.a_blur = 0.85 * smooth(local.t / 0.5) * (1 - smooth((local.t - 0.35) / 0.6));",
    "local.a_soft = 0.9 * smooth((local.t - 0.15) / 0.45) * (1 - smooth((local.t - 0.75) / 0.5));",
    "local.a_spark = smooth((local.t - 0.35) / 0.7);",
    "local.a_halo = 0.34 * spring(local.t, 3.2) + 0.3 * bump(local.t, 0.8);",
    "local.a_halo = local.a_halo + 0.07 * Math.Sin(2 * Math.Pi * (local.t - 2.0) / 5.2) * smooth((local.t - 2.0) / 1.6);",
    "local.a_wm = 0.92 * smooth((local.t - 0.95) / 0.75);",
    "global.pw_p = Math.Min(global.pw_p + local.fdt / 0.4, 1);",
    "global.pw_p = Math.Max(global.pw_p - local.fdt / 0.4, 0);",
    "global.msg_p = Math.Min(global.msg_p + local.fdt / 0.3, 1);",
    "global.msg_p = Math.Max(global.msg_p - local.fdt / 0.3, 0);",
    "local.g = 1 - 0.3 * local.pw;",
    "halo.SetOpacity(local.a_halo * (1 - 0.5 * local.pw));",
    "wordmark.SetOpacity(local.a_wm * (1 - smooth(global.pw_p / 0.5)));",
    "prompt_sprite.SetOpacity(smooth((global.pw_p - 0.4) / 0.6));",
    "message_sprite.SetOpacity(smooth(global.msg_p));",
    "bullet_spr[local.i].SetOpacity(bullet_a[local.i] * pw);",
    "wm_y = star_bottom + Math.Int(h * 0.036);",
    "bullets_y = wm_y + Math.Int(h * 0.045);", "msg_y = bullets_y + Math.Int(h * 0.05);",
    "SP = Math.Int(BS * 1.9);", "MAXB = 40;",
    "(1 - expn(fdt / 0.07));", "local.kb = 1 - expn(fdt / 0.06);",
    "local.err = (global.prog_new - global.sync_off) - global.clock + local.dt / 2;",
]

def script_constants(path):
    src = open(path).read()
    missing = [l for l in PORTED if l not in src]
    if missing:
        sys.exit("render-preview: apex-os.script no longer matches this port; update both:\n  "
                 + "\n  ".join(missing))
    m = re.search(r"^RATE = (\d+);", src, re.M)
    return {"RATE": int(m.group(1)), "src": src}

class Splash:
    def __init__(self, theme, w, h, mode, rate, text_font):
        self.scr = Screen(w, h)
        self.w, self.h = w, h
        self.RATE = rate
        self.intro = 1 if mode == "boot" else 0
        self.text_font = text_font
        img = lambda n: load_png(os.path.join(theme, n))
        short = min(w, h)
        self.hd = 1 if h > 1500 else 0
        self.cx = math.floor(w / 2)
        self.cy = math.floor(h * 0.46)
        S = odd(short * 0.18)
        if S < 61: S = 61
        self.S = S
        self.B = odd(S * 1.25)
        self.HALO = odd(S * 2.3)
        spark_src = img("spark-hd.png" if S > 300 else "spark.png")
        spark_img = resize(spark_src, S, S)
        blur_img = resize(img("spark-blur.png"), self.B, self.B)
        soft_img = resize(img("spark-soft.png"), self.B, self.B)
        halo_img = resize(img("halo.png"), self.HALO, self.HALO)
        def fit(im, ref):
            k = h / ref
            nw = math.floor(im.shape[1] * k + 0.5); nh = math.floor(im.shape[0] * k + 0.5)
            if nw == im.shape[1] and nh == im.shape[0]: return im
            return resize(im, max(nw, 1), max(nh, 1))
        if self.hd:
            self.wm_img = fit(img("wordmark-hd.png"), 2400)
            self.bullet_img = fit(img("bullet-hd.png"), 2400 * 0.8)
            self.prompt_fallback = fit(img("prompt-hd.png"), 2400)
        else:
            self.wm_img = fit(img("wordmark.png"), 1200)
            self.bullet_img = fit(img("bullet.png"), 1200 * 1.33)
            self.prompt_fallback = fit(img("prompt.png"), 1200)
        cx, cy, B, HALO = self.cx, self.cy, self.B, self.HALO
        sc = self.scr
        self.halo = sc.sprite(halo_img); self.halo.SetPosition(cx - (HALO - 1) // 2, cy - (HALO - 1) // 2, 1); self.halo.SetOpacity(0)
        self.blur = sc.sprite(blur_img); self.blur.SetPosition(cx - (B - 1) // 2, cy - (B - 1) // 2, 2); self.blur.SetOpacity(0)
        self.soft = sc.sprite(soft_img); self.soft.SetPosition(cx - (B - 1) // 2, cy - (B - 1) // 2, 3); self.soft.SetOpacity(0)
        self.spark = sc.sprite(spark_img); self.spark.SetPosition(cx - (S - 1) // 2, cy - (S - 1) // 2, 4); self.spark.SetOpacity(0)
        star_bottom = cy - (S - 1) // 2 + math.floor(S * 0.84)
        self.wm_y = star_bottom + math.floor(h * 0.036)
        self.wordmark = sc.sprite(self.wm_img)
        self.wordmark.SetPosition(cx - math.floor(self.wm_img.shape[1] / 2), self.wm_y, 5); self.wordmark.SetOpacity(0)
        self.prompt_sprite = sc.sprite(); self.prompt_sprite.SetOpacity(0)
        self.prompt_text = ""; self.prompt_ready = 0
        self.BS = self.bullet_img.shape[1]
        self.SP = math.floor(self.BS * 1.9)
        self.MAXB = 40
        self.bullets_y = self.wm_y + math.floor(h * 0.045)
        self.msg_y = self.bullets_y + math.floor(h * 0.05)
        self.bullets_n = 0; self.bullets_made = 0
        self.bullet_spr = []; self.bullet_a = []
        self.row_off = 0.0
        self.message_sprite = sc.sprite(); self.message_sprite.SetOpacity(0); self.message_text = ""
        self.pw_target = 0; self.pw_p = 0.0
        self.msg_target = 0; self.msg_p = 0.0
        self.clock = 0.0; self.last_clock = 0.0
        self.prog_new = -1.0; self.prog_last = -1.0; self.sync_off = -1.0
        self.T0 = 0.12
        self.state = {}

    def text(self, s, r, g, b):
        """Image.Text with label-freetype: 12 pt at 96 dpi of Plymouth.ttf."""
        if not s:
            return np.zeros((0, 0, 4), np.uint8)
        f = self.text_font
        l, t, rr, bb = f.getbbox(s)
        im = PILImage.new("RGBA", (max(rr, 1), max(bb, 1)), (0, 0, 0, 0))
        ImageDraw.Draw(im).text((0, 0), s, font=f, fill=(int(r * 255), int(g * 255), int(b * 255), 255))
        a = np.asarray(im).astype(np.float64)
        al = a[..., 3:4]
        rgb = np.floor(a[..., :3] * al / 255.0)
        return np.concatenate([rgb, al], axis=2).astype(np.uint8)

    def on_progress(self, duration):
        if duration > self.prog_last:
            self.prog_last = duration
            self.prog_new = duration

    def advance_clock(self):
        dt = 1 / self.RATE
        self.clock += dt
        if self.prog_new >= 0:
            if self.sync_off < 0: self.sync_off = self.prog_new - self.clock
            err = (self.prog_new - self.sync_off) - self.clock + dt / 2
            if err < -0.25: self.sync_off = self.prog_new - self.clock
            elif err > 0.25: self.clock += err
            elif err > 0: self.clock += err * 0.5
            else: self.clock += max(err * 0.1, -dt / 2)
            self.prog_new = -1.0

    def refresh(self):
        self.advance_clock()
        fdt = self.clock - self.last_clock
        self.last_clock = self.clock
        t = self.clock - self.T0 if self.intro else self.clock + 10
        a_blur = 0.85 * smooth(t / 0.5) * (1 - smooth((t - 0.35) / 0.6))
        a_soft = 0.9 * smooth((t - 0.15) / 0.45) * (1 - smooth((t - 0.75) / 0.5))
        a_spark = smooth((t - 0.35) / 0.7)
        a_halo = 0.34 * spring(t, 3.2) + 0.3 * bump(t, 0.8)
        a_halo += 0.07 * math.sin(2 * math.pi * (t - 2.0) / 5.2) * smooth((t - 2.0) / 1.6)
        a_wm = 0.92 * smooth((t - 0.95) / 0.75)
        if self.pw_target: self.pw_p = min(self.pw_p + fdt / 0.4, 1)
        else: self.pw_p = max(self.pw_p - fdt / 0.4, 0)
        if self.msg_target: self.msg_p = min(self.msg_p + fdt / 0.3, 1)
        else: self.msg_p = max(self.msg_p - fdt / 0.3, 0)
        pw = smooth(self.pw_p)
        g = 1 - 0.3 * pw
        self.halo.SetOpacity(a_halo * (1 - 0.5 * pw))
        self.blur.SetOpacity(a_blur * g)
        self.soft.SetOpacity(a_soft * g)
        self.spark.SetOpacity(a_spark * g)
        self.wordmark.SetOpacity(a_wm * (1 - smooth(self.pw_p / 0.5)))
        self.prompt_sprite.SetOpacity(smooth((self.pw_p - 0.4) / 0.6))
        self.message_sprite.SetOpacity(smooth(self.msg_p))
        if self.pw_p > 0 or self.bullets_made > 0:
            self.draw_bullets(fdt, pw)
        self.state = dict(t=t, halo=self.halo.opacity, blur=self.blur.opacity, soft=self.soft.opacity,
                          spark=self.spark.opacity, spark_size=self.spark.image.shape[1],
                          spark_x=self.spark.x, spark_y=self.spark.y, wordmark=self.wordmark.opacity,
                          pw=pw, bullets=self.bullets_n)

    def draw_bullets(self, fdt, pw):
        n = min(self.bullets_n, self.MAXB)
        while self.bullets_made < n:
            s = self.scr.sprite(self.bullet_img); s.SetOpacity(0)
            self.bullet_spr.append(s); self.bullet_a.append(0.0)
            self.bullets_made += 1
        target = -(n - 1) * self.SP / 2
        if n < 1: target = self.row_off
        self.row_off += (target - self.row_off) * (1 - expn(fdt / 0.07))
        kb = 1 - expn(fdt / 0.06)
        for i in range(self.bullets_made):
            want = 1 if i < n else 0
            self.bullet_a[i] += (want - self.bullet_a[i]) * kb
            x = self.cx + self.row_off + i * self.SP - (self.BS - 1) / 2
            self.bullet_spr[i].SetPosition(math.floor(x), self.bullets_y, 21)
            self.bullet_spr[i].SetOpacity(self.bullet_a[i] * pw)

    def display_password(self, prompt, bullets):
        self.pw_target = 1
        if not self.prompt_ready or prompt != self.prompt_text:
            self.prompt_ready = 1
            self.prompt_text = prompt
            pimg = self.text(prompt, 0.824, 0.839, 0.871)
            if pimg.shape[1] < 1: pimg = self.prompt_fallback
            self.prompt_sprite.SetImage(pimg)
            self.prompt_sprite.SetPosition(self.cx - math.floor(pimg.shape[1] / 2), self.wm_y, 20)
        self.bullets_n = bullets

    def display_normal(self):
        self.pw_target = 0

    def message(self, text):
        if text == "":
            self.msg_target = 0; return
        self.message_text = text
        m = self.text(text, 0.62, 0.66, 0.72)
        self.message_sprite.SetImage(m)
        self.message_sprite.SetPosition(self.cx - math.floor(m.shape[1] / 2), self.msg_y, 22)
        self.msg_target = 1

# ── driving it ──────────────────────────────────────────────────────────────

def password_events(at):
    """The same sequence the lab runner types into a real plymouthd."""
    p = "Please enter passphrase for disk SAMSUNG MZVL21T0 (luks-3f2a)"
    ev = [(at, "pw", p, 0), (at + 0.8, "msg", "Keyboard layout: us")]
    t = at + 1.4
    for n in range(1, 14):
        ev.append((t, "pw", p, n)); t += 0.18
    t += 1.0; ev.append((t, "pw", p, 12)); t += 0.3; ev.append((t, "pw", p, 11))
    t += 0.8; ev.append((t, "normal")); t += 0.2; ev.append((t, "msg", ""))
    return ev

def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("theme"); ap.add_argument("out")
    ap.add_argument("--res", default="1920x1200")
    ap.add_argument("--seconds", type=float, default=6.0)
    ap.add_argument("--mode", default="boot")
    ap.add_argument("--password", action="store_true", help="simulate a LUKS prompt")
    ap.add_argument("--password-at", type=float, default=3.0)
    ap.add_argument("--refresh-log", help="probe log of real refresh times (frame t0 t1 t2)")
    ap.add_argument("--gif"); ap.add_argument("--gif-width", type=int, default=600)
    ap.add_argument("--no-mp4", action="store_true")
    a = ap.parse_args()
    W, H = map(int, a.res.split("x"))
    os.makedirs(a.out, exist_ok=True)
    consts = script_constants(os.path.join(a.theme, "apex-os.script"))
    fontpath = subprocess.run(["fc-match", "-f", "%{file}"], capture_output=True, text=True).stdout.strip()
    font = ImageFont.truetype(fontpath, 16)
    sp = Splash(a.theme, W, H, a.mode, consts["RATE"], font)

    # refresh instants (seconds after the splash starts)
    if a.refresh_log:
        t0s = [float(l.split()[1]) for l in open(a.refresh_log) if l.startswith("frame")]
        refresh_times = [t - t0s[0] for t in t0s if t - t0s[0] <= a.seconds]
    else:
        n = int(a.seconds * consts["RATE"])
        refresh_times = [i / consts["RATE"] for i in range(n)]
    events = password_events(a.password_at) if a.password else []
    prog_period = 1 / 30.0
    prog_next = 0.0
    nframes = int(a.seconds * 60)
    ff = None
    if not a.no_mp4:
        ff = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
                               "-s", f"{W}x{H}", "-r", "60", "-i", "-", "-c:v", "libx264", "-preset", "slow",
                               "-crf", "10", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
                               os.path.join(a.out, "preview.mp4")], stdin=subprocess.PIPE)
    gif = None
    if a.gif:                            # 30 fps, the middle two thirds of the screen, one palette
        gw, gh = W * 2 // 3, H * 2 // 3
        gx, gy = sp.cx - gw // 2, min(max(sp.cy + H // 20 - gh // 2, 0), H - gh)
        gif = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
                                "-s", f"{gw}x{gh}", "-r", "60", "-i", "-", "-vf",
                                f"fps=30,scale={a.gif_width}:-1:flags=lanczos,split[a][b];"
                                "[a]palettegen=max_colors=64:stats_mode=full[p];"
                                "[b][p]paletteuse=dither=bayer:bayer_scale=4", a.gif], stdin=subprocess.PIPE)
    rows = []
    prev = None
    ri = 0
    ei = 0
    sheet_every = 12
    sheet = []
    for f in range(nframes):
        ts = f / 60.0                    # scanout instant
        while ri < len(refresh_times) and refresh_times[ri] <= ts + 1e-9:
            rt = refresh_times[ri]
            while prog_next <= rt + 1e-9:
                sp.on_progress(0.3 + prog_next)       # plymouth reports time since plymouthd start
                prog_next += prog_period
            while ei < len(events) and events[ei][0] <= rt + 1e-9:
                e = events[ei]
                if e[1] == "pw": sp.display_password(e[2], e[3])
                elif e[1] == "normal": sp.display_normal()
                elif e[1] == "msg": sp.message(e[2])
                ei += 1
            sp.refresh()
            nr = sp.scr.refresh()
            ri += 1
        fb = sp.scr.fb
        if ff: ff.stdin.write(fb.tobytes())
        lum = fb[..., 1].astype(np.int32)
        d = 0 if prev is None else int(np.abs(lum - prev).sum())
        mx = 0 if prev is None else int(np.abs(lum - prev).max())
        prev = lum
        st = dict(sp.state); st.update(frame=f, scan_t=round(ts, 4), clock=round(sp.clock, 4),
                                       centre_g=int(fb[sp.cy, sp.cx, 1]), frame_delta=d, max_px_delta=mx)
        rows.append(st)
        if f % sheet_every == 0 and f <= 11 * sheet_every * 2:
            sheet.append((ts, fb.copy()))
        if gif:
            gif.stdin.write(np.ascontiguousarray(fb[gy:gy + gh, gx:gx + gw]).tobytes())
    if ff:
        ff.stdin.close(); ff.wait()

    keys = ["frame", "scan_t", "clock", "t", "halo", "blur", "soft", "spark", "spark_size", "spark_x", "spark_y",
            "wordmark", "pw", "bullets", "centre_g", "frame_delta", "max_px_delta"]
    with open(os.path.join(a.out, "metrics.csv"), "w", newline="") as fh:
        wr = csv.DictWriter(fh, fieldnames=keys, extrasaction="ignore"); wr.writeheader(); wr.writerows(rows)

    # contact sheet: a crop around the spark, every 12th frame (0.2 s)
    cw, chh = min(W, int(H * 0.8)), int(H * 0.62)
    x0 = sp.cx - cw // 2; y0 = max(0, sp.cy - int(chh * 0.45))
    thumbs = []
    for ts, fbc in sheet:
        im = PILImage.fromarray(fbc[y0:y0 + chh, x0:x0 + cw]).resize((cw // 3, chh // 3), PILImage.LANCZOS)
        ImageDraw.Draw(im).text((6, 4), f"{ts:.2f}s", fill=(120, 120, 120))
        thumbs.append(im)
    cols = 6
    rws = (len(thumbs) + cols - 1) // cols
    tw, th = thumbs[0].size
    cs = PILImage.new("RGB", (cols * (tw + 4) + 4, rws * (th + 4) + 4), (40, 40, 40))
    for i, im in enumerate(thumbs):
        cs.paste(im, (4 + (i % cols) * (tw + 4), 4 + (i // cols) * (th + 4)))
    cs.save(os.path.join(a.out, "contact.png"))

    if gif:
        gif.stdin.close(); gif.wait()

    try:
        import matplotlib; matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        t = [r["scan_t"] for r in rows]
        fig, ax = plt.subplots(4, 1, figsize=(11, 11), sharex=True)
        for k in ("halo", "blur", "soft", "spark", "wordmark", "pw"):
            ax[0].plot(t, [r[k] for r in rows], label=k)
        ax[0].set_ylabel("opacity"); ax[0].legend(ncol=6, fontsize=8)
        ax[1].plot(t, [r["centre_g"] for r in rows]); ax[1].set_ylabel("centre pixel (G)")
        ax[2].plot(t, [r["frame_delta"] / 1000 for r in rows]); ax[2].set_ylabel("frame change\n(sum |dG| / 1000)")
        ax[3].plot(t, [r["spark_size"] for r in rows], label="spark size px")
        ax[3].plot(t, [r["spark_x"] + (r["spark_size"] - 1) / 2 - sp.cx for r in rows], label="spark centre x offset px")
        ax[3].legend(fontsize=8); ax[3].set_xlabel("seconds (60 Hz scanouts)")
        fig.tight_layout(); fig.savefig(os.path.join(a.out, "metrics.png"), dpi=90)
    except ImportError:
        pass
    print(f"wrote {a.out}: {nframes} frames")

if __name__ == "__main__":
    main()
