# shellcheck shell=bash
# ─────────────────────────────────────────────────────────────────────────────
#  tests/lib/atspi.sh — a PRIVATE accessibility bus, for asking an APEX surface
#  what a screen reader actually receives.
#
#  Source it; do not execute it.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#
#  Every accessibility assertion in this repository until now read the QML
#  object tree: `Accessible.name` off a live QQuickItem, `Accessible.role` off
#  the attached object. That is a real measurement and it is not the criterion.
#  The criterion is "screen reader validated", and a screen reader does not read
#  QQuickItems — it speaks AT-SPI over D-Bus and reads whatever the toolkit
#  bridge chose to publish. Those two trees are NOT the same tree: the bridge
#  drops items, collapses wrappers, suppresses names (see the passwordEdit note
#  in test-apex-greet-a11y.sh) and applies its own role mapping. An assertion on
#  the QML side cannot see any of that.
#
#  So this file stands up the real thing: a D-Bus session bus, the real
#  at-spi-bus-launcher, the real at-spi2-registryd, and the accessibility flag
#  set the way a screen reader sets it. What the app publishes onto that bus is
#  then readable with tests/atspi-walk.py, exactly as Orca would read it.
#
#  ── The predecessor's blocker, and what it actually was ─────────────────────
#
#  state/agents/p2-b.md recorded this as not measurable here:
#
#      at-spi2-registryd refuses to activate in a `dbus-run-session`:
#      Activated service 'org.a11y.atspi.Registry' failed: … Permission denied
#
#  That is a true observation with the wrong conclusion drawn from it. The
#  failure is not a sandbox refusing accessibility; it is D-Bus *activation*
#  doing what its service file says. /usr/share/dbus-1/services/org.a11y.Bus.service
#  carries `SystemdService=at-spi-dbus-bus.service`, so a dbus-daemon that is
#  asked to activate org.a11y.Bus hands the job to the calling user's systemd
#  manager — the real one, outside the test — which will not start a unit into a
#  throwaway bus it knows nothing about. Permission denied is that handoff
#  failing, not at-spi being unavailable.
#
#  Nothing here is activated. at-spi-bus-launcher and at-spi2-registryd are
#  execed directly against a bus this file owns, and both come up clean.
#
#  ── Private, and checked rather than intended ───────────────────────────────
#
#  The one thing this harness must never do is reach the a11y bus of the person
#  sitting at the machine — /run/user/<uid>/at-spi/bus. That bus belongs to a
#  live desktop session; a test that walked it would be reading, and with
#  DoAction pressing, the buttons of real windows. So:
#
#    * XDG_RUNTIME_DIR is replaced with a fresh 0700 directory before anything
#      starts, and the a11y bus lands inside it.
#    * DBUS_SESSION_BUS_ADDRESS, AT_SPI_BUS_ADDRESS, DISPLAY and WAYLAND_DISPLAY
#      are all removed from the environment first, so nothing can be inherited.
#    * atspi_start REFUSES to continue unless the address it ends up with is a
#      path inside that directory. An ambient address is an abort, not a
#      fallback — the same rule tests/lib/headless.sh applies to compositors.
#
#  ── Usage ───────────────────────────────────────────────────────────────────
#
#      . "$(dirname "$0")/lib/atspi.sh"
#
#      atspi_require            || exit 0     # SKIP 0 if the stack is missing
#      atspi_start              || exit 0     # SKIP 0 if it will not come up
#      trap atspi_cleanup EXIT INT TERM
#      # AT_SPI_BUS_ADDRESS and DBUS_SESSION_BUS_ADDRESS now name private buses.
#
#  Exported for the caller: ATSPI_W (scratch dir), ATSPI_BUS, ATSPI_RUNTIME,
#  DBUS_SESSION_BUS_ADDRESS, AT_SPI_BUS_ADDRESS, XDG_RUNTIME_DIR.
# ─────────────────────────────────────────────────────────────────────────────

# Captured before anything is changed, so a leak can be recognised rather than
# merely avoided.
ATSPI_AMBIENT_RUNTIME="${XDG_RUNTIME_DIR:-}"
ATSPI_AMBIENT_A11Y="${AT_SPI_BUS_ADDRESS:-}"
ATSPI_AMBIENT_SESSION="${DBUS_SESSION_BUS_ADDRESS:-}"

ATSPI_W=""
ATSPI_BUS=""
ATSPI_RUNTIME=""
ATSPI_LAUNCHER_PID=""
ATSPI_REGISTRY_PID=""
ATSPI_DBUS_PID=""
ATSPI_STATUS=""

# The pieces, spelled the way each distribution spells them. A missing piece is
# a SKIP with a name, never a silent degradation to "no tree found = no problem".
ATSPI_BUS_LAUNCHER=""
ATSPI_REGISTRYD=""

atspi_require() {
    local missing=()

    for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher \
             /usr/libexec/at-spi2-core/at-spi-bus-launcher; do
        [ -x "$c" ] && { ATSPI_BUS_LAUNCHER="$c"; break; }
    done
    [ -n "$ATSPI_BUS_LAUNCHER" ] || missing+=("at-spi-bus-launcher (at-spi2-core)")

    for c in /usr/libexec/at-spi2-registryd /usr/lib/at-spi2-core/at-spi2-registryd \
             /usr/libexec/at-spi2-core/at-spi2-registryd; do
        [ -x "$c" ] && { ATSPI_REGISTRYD="$c"; break; }
    done
    [ -n "$ATSPI_REGISTRYD" ] || missing+=("at-spi2-registryd (at-spi2-core)")

    command -v dbus-daemon >/dev/null 2>&1 || missing+=("dbus-daemon")
    command -v gdbus       >/dev/null 2>&1 || missing+=("gdbus (glib2)")
    python3 -c 'import gi; gi.require_version("Gio","2.0"); from gi.repository import Gio' \
        >/dev/null 2>&1 || missing+=("python3-gi (Gio typelib)")

    if [ ${#missing[@]} -gt 0 ]; then
        printf 'SKIP: the accessibility stack is not installed here: %s\n' "${missing[*]}"
        printf '      This is a COULD-NOT-RUN, not a pass. Nothing below was measured.\n'
        return 1
    fi
    return 0
}

atspi_start() {
    ATSPI_W="$(mktemp -d "${TMPDIR:-/tmp}/apex-atspi.XXXXXX")" || return 1
    chmod 700 "$ATSPI_W"
    ATSPI_RUNTIME="$ATSPI_W/run"
    mkdir -p "$ATSPI_RUNTIME" && chmod 700 "$ATSPI_RUNTIME" || return 1

    # Nothing is inherited. A stale AT_SPI_BUS_ADDRESS in the environment is the
    # one value that could silently point every assertion below at a live
    # desktop, so it goes first.
    unset AT_SPI_BUS_ADDRESS DBUS_SESSION_BUS_ADDRESS
    export XDG_RUNTIME_DIR="$ATSPI_RUNTIME"

    # Portals are turned off, belt and braces. Not tidiness: the first run of
    # this file activated xdg-desktop-portal-gtk on the private session bus,
    # which (a) registered ITSELF with the accessibility registry, so the walk
    # saw an application the test never started, and (b) fuse-mounted a `doc`
    # directory inside the private runtime dir, which then refused to be removed
    # on cleanup. An assertion that counts registered applications is wrong the
    # moment a portal can appear in the count.
    #
    # The environment variables below are only the polite half, and on their own
    # they did NOT stop it -- the portal came back on the next run. The half that
    # works is the bus config written in atspi_start: the private session bus is
    # given an EMPTY service directory, so it can activate nothing at all. Every
    # process this harness wants is execed by hand, so there is nothing for
    # activation to do and no way for a stray client to conjure a daemon onto a
    # bus the test is about to make assertions over.
    export GTK_USE_PORTAL=0 GIO_USE_PORTALS=0 QT_NO_XDG_DESKTOP_PORTAL=1

    # ── the private session bus ──────────────────────────────────────────────
    # Started by hand rather than with dbus-run-session, because the address has
    # to be readable here to prove it is the private one.
    # --print-pid as well as --print-address: cleanup must kill exactly the
    # daemon this function started. An earlier draft matched it with `pkill -f`,
    # which would have reaped any other harness's private bus on the machine --
    # and this repository runs several suites at once.
    mkdir -p "$ATSPI_W/no-services" || return 1
    cat >"$ATSPI_W/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$ATSPI_W</listen>
  <!-- Deliberately an EMPTY directory. Activation is what let a desktop portal
       appear inside a test's accessibility tree; with nowhere to look for
       .service files the bus cannot start anything that this harness did not
       exec itself. -->
  <servicedir>$ATSPI_W/no-services</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
    dbus-daemon --config-file="$ATSPI_W/session.conf" --print-address=3 --print-pid=4 --fork \
        3>"$ATSPI_W/session-address" 4>"$ATSPI_W/session-pid" \
        2>"$ATSPI_W/dbus.err" || {
        printf 'SKIP: could not start a private session bus\n'
        sed 's/^/      dbus: /' "$ATSPI_W/dbus.err" 2>/dev/null | head -3
        return 1; }
    ATSPI_DBUS_PID="$(cat "$ATSPI_W/session-pid" 2>/dev/null)"
    DBUS_SESSION_BUS_ADDRESS="$(cat "$ATSPI_W/session-address" 2>/dev/null)"
    export DBUS_SESSION_BUS_ADDRESS
    [ -n "$DBUS_SESSION_BUS_ADDRESS" ] || {
        printf 'SKIP: the private session bus printed no address\n'; return 1; }

    # ── the private a11y bus ────────────────────────────────────────────────
    # Directly, NOT by activation. See the header.
    "$ATSPI_BUS_LAUNCHER" >"$ATSPI_W/launcher.out" 2>"$ATSPI_W/launcher.err" &
    ATSPI_LAUNCHER_PID=$!

    local i raw=""
    for i in $(seq 1 60); do
        raw="$(gdbus call --session -d org.a11y.Bus -o /org/a11y/bus \
                    -m org.a11y.Bus.GetAddress 2>/dev/null)" && [ -n "$raw" ] && break
        kill -0 "$ATSPI_LAUNCHER_PID" 2>/dev/null || break
        sleep 0.25
    done
    ATSPI_BUS="$(printf '%s' "$raw" | sed -e "s/^('//" -e "s/',)$//")"
    if [ -z "$ATSPI_BUS" ]; then
        printf 'SKIP: at-spi-bus-launcher did not publish an a11y bus address\n'
        sed 's/^/      launcher: /' "$ATSPI_W/launcher.err" 2>/dev/null | head -5
        return 1
    fi

    # ── the refusal ─────────────────────────────────────────────────────────
    # The whole point of the private runtime dir is that the bus lands inside
    # it. If it did not, something reached an ambient session and every
    # assertion after this point would be about somebody's real desktop.
    case "$ATSPI_BUS" in
        *"unix:path=$ATSPI_RUNTIME/"*) : ;;
        *)
            printf 'FATAL: the a11y bus is not inside the private runtime dir.\n' >&2
            printf '       want a path under: %s\n' "$ATSPI_RUNTIME" >&2
            printf '       got:               %s\n' "$ATSPI_BUS" >&2
            printf '       Refusing to walk a bus that may belong to a live session.\n' >&2
            return 1
            ;;
    esac
    if [ -n "$ATSPI_AMBIENT_A11Y" ] && [ "$ATSPI_BUS" = "$ATSPI_AMBIENT_A11Y" ]; then
        printf 'FATAL: the a11y bus is the ambient one this shell started with.\n' >&2
        return 1
    fi
    # ── the flag a screen reader sets ───────────────────────────────────────
    # Set because a screen reader sets it: Orca's first act on connecting is to
    # put org.a11y.Status.ScreenReaderEnabled true, and toolkits watch that
    # property to decide whether to pay for an accessibility tree at all. This
    # harness IS the assistive technology, so it does what one does.
    #
    # ── A correction, and the machine that produced the wrong answer ────────
    #
    # This block used to say the opposite, and said it with measurements:
    #
    #     org.a11y.Status.IsEnabled cannot be pushed false here at all. Setting
    #     it to <false> and reading it straight back returns <true> ... Qt 6.10.3
    #     publishes whenever it can REACH an a11y bus, with IsEnabled true or
    #     false. All four combinations register.
    #
    # Both observations were real and the conclusion was an artefact of ONE
    # laptop. On a developer machine with a live desktop session both properties
    # are ALREADY true before this harness starts, so "set false, read true" was
    # measuring a value that had never been false, and "Qt does not consult it"
    # was never tested against a flag that was genuinely off.
    #
    # Measured on a second machine -- a bare fedora:43 container, which is what
    # CI runs -- the same code gives the opposite result:
    #
    #     BEFORE: {'IsEnabled': <false>, 'ScreenReaderEnabled': <false>}
    #     the greeter registers NOTHING with the registry
    #     ... set both true, and the tree appears exactly as on the laptop.
    #
    # So Qt DOES gate on the status flags, they CAN be pushed in both
    # directions on a machine where nothing has already turned them on, and a
    # suite that relied on the old paragraph would have reported the greeter as
    # publishing no accessibility tree the moment it ran anywhere but here. That
    # is the second time in this unit that a second machine has corrected a
    # finding a laptop was certain of.
    #
    # Reachability is still the OTHER gate, and still the shipped greeter's
    # problem: an application with no D-Bus session bus cannot resolve
    # org.a11y.Bus and publishes nothing whatever these flags say. See
    # tests/test-apex-greet-session-bus.sh.
    #
    # QT_LINUX_ACCESSIBILITY_ALWAYS_ON is still deliberately NOT set. It forces
    # the bridge past every check including reachability, which would make this
    # harness pass on an image where accessibility genuinely cannot work. These
    # two properties are a different thing: they are the switch a screen reader
    # itself throws, on the bus, over the same interface any reader uses.
    #
    # Order matters and is load-bearing: this block sits BEFORE the
    # AT_SPI_BUS_ADDRESS export because tests/mutate-greet-atspi.sh's A7 splices
    # a broken session bus in at that export. With the block after it, A7 would
    # break this harness's own status read and the suite would go red one
    # assertion early, on a line about the test rather than about the greeter.
    for _p in ScreenReaderEnabled IsEnabled; do
        gdbus call --session -d org.a11y.Bus -o /org/a11y/bus \
              -m org.freedesktop.DBus.Properties.Set \
              org.a11y.Status "$_p" "<true>" >/dev/null 2>&1
    done
    ATSPI_STATUS="$(gdbus call --session -d org.a11y.Bus -o /org/a11y/bus \
        -m org.freedesktop.DBus.Properties.GetAll org.a11y.Status 2>/dev/null)"
    export ATSPI_STATUS
    case "$ATSPI_STATUS" in
        *"'ScreenReaderEnabled': <true>"*) : ;;
        *)  # Not fatal here -- the suite that cares asserts it and says so --
            # but it must never be silent, because the consequence is an empty
            # tree that reads exactly like a greeter with no markup.
            printf 'NOTE: org.a11y.Status.ScreenReaderEnabled did not read back true: %s\n' \
                   "${ATSPI_STATUS:-<no answer>}" ;;
    esac

    export AT_SPI_BUS_ADDRESS="$ATSPI_BUS"

    # ── the registry ────────────────────────────────────────────────────────
    "$ATSPI_REGISTRYD" >"$ATSPI_W/registry.out" 2>"$ATSPI_W/registry.err" &
    ATSPI_REGISTRY_PID=$!
    for i in $(seq 1 60); do
        gdbus call --address "$ATSPI_BUS" -d org.freedesktop.DBus \
              -o /org/freedesktop/DBus -m org.freedesktop.DBus.ListNames 2>/dev/null \
            | grep -q 'org.a11y.atspi.Registry' && break
        sleep 0.25
    done
    if ! gdbus call --address "$ATSPI_BUS" -d org.freedesktop.DBus \
              -o /org/freedesktop/DBus -m org.freedesktop.DBus.ListNames 2>/dev/null \
            | grep -q 'org.a11y.atspi.Registry'; then
        printf 'SKIP: at-spi2-registryd did not take its name on the private bus\n'
        sed 's/^/      registryd: /' "$ATSPI_W/registry.err" 2>/dev/null | head -5
        return 1
    fi
    return 0
}

# Launch an application the way a real one starts: with a session bus, and
# WITHOUT AT_SPI_BUS_ADDRESS.
#
# That variable is a shortcut. An application that has it connects straight to
# the named bus; an application without it has to do what every real desktop
# application does -- resolve org.a11y.Bus on the session bus and ask it for the
# address. Leaving the shortcut in the environment made this harness unable to
# tell the two paths apart: the mutant that breaks the session bus SURVIVED,
# because the application under test still had a direct address to fall back on.
#
# The walker keeps the variable, because the walker is the assistive technology
# in this picture and a screen reader is told where the bus is. The application
# must find it.
atspi_run_app() {
    # `exec`, and it is load-bearing. Backgrounding a shell FUNCTION runs it in a
    # subshell, so `$!` is the subshell and the application is its child --
    # `kill "$!"` then reaps the subshell while the application keeps running,
    # still registered with the registry. A survey written before this line had
    # six installer processes alive at once and reported their merged trees as
    # one page's, which made every per-page count in it wrong.
    # exec replaces the subshell, so `$!` really is the application.
    exec env -u AT_SPI_BUS_ADDRESS "$@"
}

# How many applications are registered with the private registry right now.
# Prints a number; prints 0 if the registry cannot be reached at all, so callers
# must treat "cannot reach" separately from "nothing registered" where it
# matters.
atspi_app_count() {
    python3 "$(dirname "${BASH_SOURCE[0]}")/../atspi-walk.py" --count 2>/dev/null || echo 0
}

atspi_cleanup() {
    local p
    for p in "$ATSPI_REGISTRY_PID" "$ATSPI_LAUNCHER_PID"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null
    done
    # The launcher owns the a11y dbus-daemon it spawned and takes it down when
    # it exits. The session bus is killed BY PID -- never by pattern; see the
    # --print-pid note in atspi_start.
    [ -n "$ATSPI_DBUS_PID" ] && kill "$ATSPI_DBUS_PID" 2>/dev/null
    # A portal or a toolkit may still have fuse-mounted something inside the
    # private runtime dir despite the suppression above. Unmount what can be
    # unmounted, then remove; a leftover busy mount must not make cleanup noisy
    # enough that a real failure scrolls past.
    if [ -n "$ATSPI_RUNTIME" ] && [ -d "$ATSPI_RUNTIME/doc" ]; then
        fusermount -u "$ATSPI_RUNTIME/doc" 2>/dev/null || \
        fusermount3 -u "$ATSPI_RUNTIME/doc" 2>/dev/null || true
    fi
    [ -n "$ATSPI_W" ] && rm -rf "$ATSPI_W" 2>/dev/null
    return 0
}
