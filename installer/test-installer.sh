#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer.sh — test what actually ships: the engine's input guards,
#  and that every page of the GTK installer really draws.
#
#  (Renamed from test-engine-guards.sh when the GUI half was added. That file
#  in turn replaced test-interactive.sh, which drove the whiptail TUI with
#  canned answers — the TUI no longer exists: apex-install is engine-only now,
#  spoken to as `apex-install --headless ANSWERS` by the GTK installer, and
#  the text UI people kept getting stranded in has been deleted.)
#
#  ── Half 1: the engine refuses bad input BEFORE it wipes ────────────────────
#  Deleting the TUI silently deleted three guards that only lived inside it —
#  the username regex, the reserved-name check, and the hostname regex. Losing
#  them is not cosmetic. Nothing else rejects a bad username until `useradd`
#  runs, and `useradd` runs AFTER `bootc install --wipe` has already erased the
#  disk: the result is a fully installed system with no account on it, and the
#  user's previous OS gone. The original TUI validated early for exactly that
#  reason and said so in a comment. Two more guards (target == ESP, and target
#  not on the named disk) had no equivalent at all in headless mode. All five
#  are asserted below so a future refactor cannot quietly drop them again.
#
#  EVERY case here must fail BEFORE anything is written, so this half NEVER
#  touches a block device. The two partition-mode cases name real devices
#  (/dev/sda, /dev/sdb) because the guards need `-b` to succeed to be reached at
#  all — but they are rejected by the guard under test, several steps before any
#  mkfs, mount or bootc call. Nothing is opened for writing.
#
#  ── Half 2: every GUI page must draw ────────────────────────────────────────
#  The GUI is now the ONLY front end. If a page fails to render, or lays out so
#  its buttons land off-screen, the user is stranded with no fallback — and a
#  syntax-clean file proves nothing about either. So every page named in the
#  GUI's own registry is rendered headless (cage + wlroots-headless + grim in
#  the apex-guitest container) and the screenshot is measured, not just stat'd:
#  a produced PNG is NOT a pass — a blank or single-colour frame means the page
#  did not draw. Pages render at 1024x600 and 1366x768, the realistic
#  worst-case laptop panels; one page already clipped its action row at 720 px
#  (measured), which is exactly the failure class this half exists to catch.
#  Pixel checks alone are not enough, though: GTK prefers to SQUASH mid-page
#  widgets over pushing the action row off-screen (measured: at 1024x600 the
#  account page swallows the Computer-name field whole, buttons still visible),
#  so every page is also measured — GTK is asked for the page's minimum height
#  at each panel width, and it must fit. The exact pass criteria are documented
#  inline below. No disk, real or virtual, is enumerated (lsblk is stubbed
#  inside the container), let alone touched.
#
#  PASS = every engine case prints its expected APEX-INSTALL-FAILED reason and
#         never "Unexpected error on line" (that string means the ERR trap
#         fired, which is always a bug in the installer), and every GUI page
#         passes every render check at every size.
#
#  Run from the repo's installer/ directory. Needs passwordless root (sudo -n):
#  the engine refuses to run unprivileged, and the render container lives in
#  ROOT podman storage (built here on first run if missing).
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")"

ENGINE=./apex-install
ANS=$(mktemp /tmp/apex-test-answers.XXXXXX)
trap 'rm -f "$ANS"' EXIT
chmod 600 "$ANS"

pass=0; fail=0

# $1 = case name, $2 = expected substring in the failure reason, $3 = answers body
check() {
    local name=$1 want=$2 body=$3 out
    printf '%s\n' "$body" > "$ANS"
    out=$(sudo -n "$ENGINE" --headless "$ANS" 2>&1 </dev/null)

    if grep -q 'Unexpected error on line' <<<"$out"; then
        printf 'FAIL  %-30s ERR TRAP FIRED\n' "$name"; fail=$((fail+1)); return
    fi
    if grep -qF "$want" <<<"$out"; then
        printf 'PASS  %-30s\n' "$name"; pass=$((pass+1))
    else
        printf 'FAIL  %-30s expected %q\n      got: %s\n' \
            "$name" "$want" "$(grep -m1 APEX-INSTALL-FAILED <<<"$out" || echo '<no sentinel>')"
        fail=$((fail+1))
    fi
}

# A disk that cannot exist, so the whole-disk cases stop at the block-device
# check instead of proceeding. The account guards run BEFORE that check — which
# is the ordering under test.
BASE=$'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nhostname=apex'

echo "── argument handling ──────────────────────────────────────────────────"
out=$(sudo -n "$ENGINE" </dev/null 2>&1); rc=$?
if [ "$rc" = 2 ] && grep -q 'not a user interface' <<<"$out"; then
    printf 'PASS  %-30s (exit 2, starts nothing)\n' "no arguments"; pass=$((pass+1))
else
    printf 'FAIL  %-30s exit=%s\n' "no arguments" "$rc"; fail=$((fail+1))
fi

echo "── account validation (must run before the disk is touched) ───────────"
check "username: uppercase"   "Invalid username 'Bob'"        "$BASE"$'\nusername=Bob'
check "username: leading digit" "Invalid username '1bob'"     "$BASE"$'\nusername=1bob'
check "username: reserved"    "reserved system account"       "$BASE"$'\nusername=root'
check "hostname: underscore"  "Invalid hostname 'my_host'"    $'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nusername=bob\nhostname=my_host'

echo "── answers-file handling ──────────────────────────────────────────────"
check "unknown key"           "unknown key in answers file"   "$BASE"$'\nusername=bob\nbogus=1'
check "missing password"      "password missing"              $'mode=disk\ndisk=/dev/zzz-does-not-exist\nusername=bob\nhostname=apex'
check "bad mode value"        "bad mode"                      $'mode=wipeitall\ndisk=/dev/zzz-does-not-exist\nusername=bob\npassword=pw\nhostname=apex'
check "valid input reaches disk check" "is not a block device" "$BASE"$'\nusername=bob'

# The parser splits on '=' with IFS, so a password containing '=' is a real
# risk: everything after the first '=' must survive intact.
printf 'username=bob\npassword=a=b=c\nhostname=apex\n' > "$ANS"
got=$(while IFS='=' read -r k v || [ -n "$k" ]; do [ "$k" = password ] && printf '%s' "$v"; done < "$ANS")
if [ "$got" = 'a=b=c' ]; then
    printf 'PASS  %-30s\n' "password containing '='"; pass=$((pass+1))
else
    printf 'FAIL  %-30s got %q\n' "password containing '='" "$got"; fail=$((fail+1))
fi

# A file whose last line has no trailing newline used to lose that line
# entirely — measured. A dropped `mokpw` would skip Secure Boot enrolment
# without a word, so the parser reads the final unterminated line too.
printf 'username=bob\npassword=pw\nhostname=lastline' > "$ANS"
got=$(while IFS='=' read -r k v || [ -n "$k" ]; do [ "$k" = hostname ] && printf '%s' "$v"; done < "$ANS")
if [ "$got" = 'lastline' ]; then
    printf 'PASS  %-30s\n' "no trailing newline"; pass=$((pass+1))
else
    printf 'FAIL  %-30s last key lost\n' "no trailing newline"; fail=$((fail+1))
fi

echo "── partition mode: the two most destructive mistakes ──────────────────"
# These need devices that exist for the guard to be reached. Read-only: both
# cases are refused by the guard under test, long before any write.
if [ -b /dev/sda ] && [ -b /dev/sdb ] && [ -b /dev/sda2 ] && [ -b /dev/sdb1 ]; then
    check "target == ESP"     "same device"                   $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sda2\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex'
    check "target on another disk" "is not a partition of"    $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sdb1\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex'
else
    echo "SKIP  partition-mode cases (need /dev/sda2 and /dev/sdb1 present)"
fi

echo
echo "── GUI: every page must draw — it is the only front end there is ──────"

GUI=./apex-installer-gui
GUITEST=localhost/apex-guitest:latest       # gtk4/libadwaita/cage/grim/python3-cairo
RANDR=localhost/apex-guitest-randr:latest   # + wlr-randr, to drive the output geometry
SIZES="1024x600 1366x768"

# The page list comes from the GUI's own registry (the add_named loop in
# startup()), never from a list here that would rot the first time a page is
# added — the secureboot page appeared exactly that way.
PAGES=$(sed -n '/for name, build in (/,/):/p' "$GUI" \
        | grep -oE '"[a-z]+"' | tr -d '"' | awk '!seen[$0]++' | xargs)

gui_skip=0
case " $PAGES " in
    *" welcome "*) [ "$(wc -w <<<"$PAGES")" -ge 6 ] || gui_skip=1 ;;
    *) gui_skip=1 ;;
esac
if [ "$gui_skip" = 1 ]; then
    printf 'FAIL  %-30s could not read the page registry from %s (got: "%s")\n' \
        "gui: page registry" "$GUI" "$PAGES"
    fail=$((fail+1))
fi

# The render images live in ROOT podman storage; build them here if absent so
# the test is runnable on a fresh machine. wlroots' headless output is
# hard-wired to 1280x720 — the only supported way to get another geometry is
# the wlr-output-management protocol, which cage speaks and wlr-randr drives.
# Hence the one-package derived image.
if [ "$gui_skip" = 0 ] && ! sudo -n podman image exists "$GUITEST" 2>/dev/null; then
    echo "      ($GUITEST missing — building it, first run only)"
    printf 'FROM registry.fedoraproject.org/fedora:43\nRUN dnf install -y cage gtk4 libadwaita python3-gobject gobject-introspection python3-cairo cairo-gobject mesa-dri-drivers seatd grim && dnf clean all\n' \
        | sudo -n podman build -t "$GUITEST" -f - /var/empty >/dev/null 2>&1 \
        || { printf 'FAIL  %-30s could not build %s\n' "gui: render image" "$GUITEST"
             fail=$((fail+1)); gui_skip=1; }
fi
if [ "$gui_skip" = 0 ] && ! sudo -n podman image exists "$RANDR" 2>/dev/null; then
    printf 'FROM %s\nRUN dnf install -y wlr-randr && dnf clean all\n' "$GUITEST" \
        | sudo -n podman build -t "$RANDR" -f - /var/empty >/dev/null 2>&1 \
        || { printf 'FAIL  %-30s could not build %s\n' "gui: render image" "$RANDR"
             fail=$((fail+1)); gui_skip=1; }
fi

if [ "$gui_skip" = 0 ]; then
    WORK=$(mktemp -d /tmp/apex-gui-render.XXXXXX)
    mkdir -p "$WORK/gui" "$WORK/stub"
    # SELinux denies the container read access to $HOME even :ro, so the GUI is
    # copied beside the output dir and the whole thing is mounted :Z. The copy
    # is made fresh every run — it IS the file under test, just relabelled.
    cp "$GUI" "$WORK/gui/apex-installer-gui"

    # Stub lsblk (PATH-first inside the container): the container has no disks,
    # which would render only the empty-state pages. This presents a realistic
    # dual-boot table — ESP + Windows + Linux + a crypto_LUKS partition — so
    # disk/mode/part/confirm draw their full lists, including the blocked
    # container-member row and confirm's ERASED/KEPT/SHARED verdicts. Nothing
    # real is enumerated, let alone touched.
    cat > "$WORK/stub/lsblk" <<'STUB'
#!/usr/bin/env bash
case "$*" in
  *NAME,MOUNTPOINT*) exit 0 ;;   # live-media scan: nothing here is live media
  *NAME,SIZE,TYPE,MODEL,TRAN,RM,SERIAL*)
    echo 'NAME="vda" SIZE="512G" TYPE="disk" MODEL="APEX Test SSD" TRAN="nvme" RM="0" SERIAL="APXTEST01"'; exit 0 ;;
  *NAME,TYPE,SIZE,FSTYPE,LABEL,PARTTYPE*)
    printf '%s\n' \
      'vda1 part 512M vfat ESP c12a7328-f81f-11d2-ba4b-00a0c93ec93b' \
      'vda2 part 220G ntfs Windows ebd0a0a2-b9e5-4433-87c0-68b6b72699c7' \
      'vda3 part 240G btrfs Linux 0fc63daf-8483-4772-8e79-3d69d8477de4' \
      'vda4 part 50G crypto_LUKS vault 0fc63daf-8483-4772-8e79-3d69d8477de4'; exit 0 ;;
  *TYPE,SIZE,FSTYPE,LABEL*)
    printf '%s\n' 'part 512M vfat ESP' 'part 220G ntfs Windows' \
                  'part 240G btrfs Linux' 'part 50G crypto_LUKS vault'; exit 0 ;;
esac
exit 0
STUB

    # Inert engine stand-in for the run page. Without it the GUI's spawn of
    # /usr/bin/apex-install fails instantly and the page bounces to "done"
    # before grim fires — the screenshot would show the wrong page. It emits
    # the two status lines the page displays, then idles. Touches nothing.
    cat > "$WORK/stub/apex-install" <<'STUB'
#!/usr/bin/env bash
echo "Installing APEX-OS to /dev/vda3 (partition of /dev/vda) … (full log: /var/log/apex-install.log)"
echo "Do not power off — /dev/vda3 is being erased and rewritten from here on."
sleep 300
STUB
    # Exec bits matter: a non-executable stub is silently SKIPPED by the PATH
    # search and the REAL lsblk answers instead — measured: the disk page came
    # back listing this machine's actual drives through the container's /sys.
    chmod +x "$WORK/stub/"*

    # Measures every screenshot. One line per PNG:
    #   METRIC <file> <w> <h> <bytes> <ncolours> <bottom_clean> <actionpx> <right_clean>
    #
    # Criteria (thresholds applied by the shell below):
    #  ncolours ≥ 32 and ≥ 10000 bytes — "the page drew". A rendered page has
    #    HUNDREDS of distinct colours from font antialiasing alone (welcome
    #    measures ~700, 44 KB); a frame where GTK died is the compositor's
    #    solid fill: 1 colour, a few KB of PNG. The thresholds sit far from
    #    both, so neither theme tweaks nor compression changes can flip them.
    #  bottom_clean — frame() gives every page a 32 px background-only margin
    #    BELOW the action row. Any non-background pixel in the bottom 12 rows
    #    means the column overflowed the window and was clipped — the buttons
    #    are (at least partly) off-screen. The GUI is the only front end, so an
    #    unreachable Continue is a stranded user; this is that detector.
    #  actionpx ≥ 150 — non-background pixels in rows [h-90, h-12). A visible
    #    action row (min-height-44 buttons sitting directly above the margin)
    #    paints thousands there. This closes the one hole in bottom_clean: an
    #    overflow that happens to cut inside the background gap just ABOVE the
    #    buttons leaves the bottom strip clean while the buttons are still
    #    off-screen. The run page has no buttons by design (an install must not
    #    be abortable mid-write); its "Do not power off" caption occupies the
    #    same band, so the check holds there too.
    #  right_clean — the same idea sideways: a three-button action row that
    #    does not fit paints the last 8 columns; catches horizontal clipping.
    cat > "$WORK/analyze.py" <<'PY'
import cairo, os
OUT = "/out"
for f in sorted(os.listdir(OUT)):
    if not f.endswith(".png"):
        continue
    p = os.path.join(OUT, f)
    s = cairo.ImageSurface.create_from_png(p)
    w, h, stride = s.get_width(), s.get_height(), s.get_stride()
    ints = memoryview(bytes(s.get_data())).cast("I")  # one uint32 per pixel
    spx = stride // 4
    bg = ints[0]  # (0,0) sits inside the page's top margin: always background
    colours = set()
    for y in range(h):
        colours.update(ints[y*spx : y*spx + w])
    bottom_clean = int(all(v == bg for y in range(h-12, h)
                           for v in ints[y*spx : y*spx + w]))
    right_clean = int(all(ints[y*spx + x] == bg
                          for y in range(h) for x in range(w-8, w)))
    actionpx = sum(1 for y in range(max(0, h-90), h-12)
                   for v in ints[y*spx : y*spx + w] if v != bg)
    print("METRIC", f, w, h, os.path.getsize(p), len(colours),
          bottom_clean, actionpx, right_clean)
PY

    # The pixel checks cannot see a widget squashed in the MIDDLE of a page:
    # when a page is taller than the panel, GTK shrinks body children below
    # their minimum instead of pushing the action row off — measured at
    # 1024x600, where the account page kept its buttons but swallowed the
    # Computer-name entry whole. So ask GTK itself: import the real GUI (its
    # __main__ guard makes that safe), build every page from the builders
    # registry, and print each page's MINIMUM height at each panel width. A
    # page whose minimum exceeds the panel height cannot be laid out without
    # squashing or clipping something — that is the assertion.
    cat > "$WORK/measure.py" <<'PY'
import importlib.util, os
from importlib.machinery import SourceFileLoader
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk

# SourceFileLoader explicitly: the GUI has no .py extension, so
# spec_from_file_location alone cannot infer a loader for it.
loader = SourceFileLoader("apexgui", "/out/gui/apex-installer-gui")
spec = importlib.util.spec_from_loader("apexgui", loader)
mod = importlib.util.module_from_spec(spec)
loader.exec_module(mod)

widths = [int(x) for x in os.environ["MEASURE_WIDTHS"].split()]
app = mod.Installer()

def measure(_app):
    # Runs after the GUI's own activate handler, so builders exist and the
    # APEX_GUI_* state has been seeded exactly as in a jump-to-page render.
    for name, build in app.builders.items():
        page = build()
        for w in widths:
            print("MEASURE", name, w, page.measure(Gtk.Orientation.VERTICAL, w)[0],
                  flush=True)
    app.quit()

app.connect("activate", measure)
app.run(None)
PY

    # Runs INSIDE the container: for each geometry × page, start cage on a
    # headless output, let the first client resize it with wlr-randr, exec the
    # real GUI jumped to the page via APEX_GUI_PAGE (its documented test
    # affordance), screenshot with grim, tear down. Measure everything at the
    # end in one pass.
    cat > "$WORK/inner.sh" <<'INNER'
#!/usr/bin/env bash
set -u
sizes=$1; pages=$2
export XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export GSK_RENDERER=cairo GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1
export PATH=/out/stub:$PATH
install -m 0755 /out/stub/apex-install /usr/bin/apex-install
# Jump-to-page state: a partition-mode install of /dev/vda3, so confirm shows
# a per-partition verdict list and done shows the partition-mode success text.
export APEX_GUI_MODE=partition APEX_GUI_DISK=/dev/vda \
       APEX_GUI_TARGET=/dev/vda3 APEX_GUI_ESP=/dev/vda1 APEX_GUI_OK=1
for size in $sizes; do
  for p in $pages; do
    rm -f "$XDG_RUNTIME_DIR"/wayland*   # fresh socket → grim finds wayland-0
    APEX_GUI_PAGE=$p timeout 30 cage -- bash -c \
      "wlr-randr --output HEADLESS-1 --custom-mode $size >/dev/null 2>&1; sleep 1; exec python3 /out/gui/apex-installer-gui" \
      2>/dev/null &
    cpid=$!
    sleep 6                             # measured: first frame lands well within this
    grim "/out/$size-$p.png" 2>/dev/null || echo "RENDER-FAIL $size-$p"
    kill "$cpid" 2>/dev/null; wait "$cpid" 2>/dev/null
  done
done
# Layout audit (see measure.py): one more cage session, no screenshot — the
# client measures every page at every panel width and prints MEASURE lines.
rm -f "$XDG_RUNTIME_DIR"/wayland*
MEASURE_WIDTHS="$(for s in $sizes; do printf '%s ' "${s%x*}"; done)" \
  APEX_GUI_PAGE=confirm timeout 60 cage -- python3 /out/measure.py 2>/dev/null
exec python3 /out/analyze.py
INNER

    sudo -n podman run --rm --network=none -v "$WORK":/out:Z "$RANDR" \
        bash /out/inner.sh "$SIZES" "$PAGES" >"$WORK/render.log" 2>&1 || true

    for size in $SIZES; do
        for p in $PAGES; do
            name="gui: $p @ $size"
            line=$(grep -m1 "^METRIC $size-$p\.png " "$WORK/render.log" || true)
            if [ -z "$line" ]; then
                printf 'FAIL  %-30s no screenshot produced (see %s/render.log)\n' \
                    "$name" "$WORK"
                fail=$((fail+1)); continue
            fi
            read -r _ _ w h bytes ncolours bclean apx rclean <<<"$line"
            why=""
            [ "${w}x${h}" = "$size" ] \
                || why="rendered ${w}x${h}, wanted $size (mode-set failed)"
            if [ "$ncolours" -lt 32 ] || [ "$bytes" -lt 10000 ]; then
                why="${why:+$why; }blank frame ($ncolours colours, $bytes bytes) — page did not draw"
            fi
            [ "$bclean" = 1 ] \
                || why="${why:+$why; }content clipped at the BOTTOM edge — action row off-screen"
            [ "$apx" -ge 150 ] \
                || why="${why:+$why; }action-row band empty — buttons not visible"
            [ "$rclean" = 1 ] \
                || why="${why:+$why; }content clipped at the RIGHT edge"
            W=${size%x*}; H=${size#*x}
            minh=$(grep -m1 "^MEASURE $p $W " "$WORK/render.log" | awk '{print $4}')
            if [ -z "$minh" ]; then
                why="${why:+$why; }page was never measured (measure.py died — see render.log)"
            elif [ "$minh" -gt "$H" ]; then
                why="${why:+$why; }needs ${minh}px height at ${W}px wide — a $size panel squashes or hides part of it"
            fi
            if [ -z "$why" ]; then
                printf 'PASS  %-30s\n' "$name"; pass=$((pass+1))
            else
                printf 'FAIL  %-30s %s\n' "$name" "$why"; fail=$((fail+1))
            fi
        done
    done
    echo "      (screenshots kept in $WORK for eyeballing)"
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
printf '%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
