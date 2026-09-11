#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Run a command inside a REAL logind session, so the daemon can observe a
#  local origin.
#
#      ./tests/in-login-session.sh ./tests/test-privilege-requests.sh
#      ./tests/in-login-session.sh cargo test --locked
#
#  ## Why this exists
#
#  `apex-agentd` decides whether a human is at this machine by reading the
#  connecting peer's cgroup — `origin::classify`. A process in
#  `session-N.scope` descends from a login; a process under `user@N.service`
#  was started by systemd with nobody present. §7 reserves approving a root
#  operation, and holding a system-access grant, for the first kind.
#
#  That rule is correct and it is also why a test runner started by systemd
#  cannot reach the behaviour those tests are about. An agent dispatched by
#  `apex-roadmap-resume.timer`, a CI job, a bare `systemd-run --user` scope —
#  all of them classify as `scheduled-job`, so the gate refuses before the
#  assertion under test is reached. Measured on 2026-09-07, on the same tree
#  and the same commit:
#
#      from session-4.scope                          38 passed, 0 failed
#      from user@1000.service/app.slice/x.scope      30 passed, 8 failed
#
#  Eight assertions of the approval path — including "a denied request cannot
#  be flipped to approved" — plus `system_grants.rs`'s
#  `renewing_a_grant_that_does_not_exist_…`, had therefore never executed
#  anywhere. Not once, in any environment, in three integration rounds.
#
#  ## What it must NOT do
#
#  The obvious trick is to pick a unit name:
#
#      systemd-run --user --scope --unit=session-4242.scope
#
#  That produces the cgroup `…/user@1000.service/app.slice/session-4242.scope`,
#  which `classify` reads as local because it tests `/session-` + `.scope`
#  before it tests `/user@` + `.service`. It works. It is also the security
#  hole that observation is supposed to close — any unprivileged process can
#  claim §7's first column that way — so a harness built on it would be
#  testing the gate by defeating it, and would break the day the hole is
#  fixed. It is recorded for the fix in ROADMAP/state/agents/p0-014.md; this
#  script deliberately does not use it and does not depend on it.
#
#  ## What it does instead
#
#  It asks logind, through PAM, for a session — the same call `login(1)`,
#  `sshd` and `su -l` make. logind creates a genuine `session-N.scope` as a
#  direct child of `user-<uid>.slice`, with `Class=user` and `Type=tty`, and
#  the command runs in it as the *invoking* user. Nothing is faked: the
#  session is in logind's own records, which is why `GetSessionByPID` below
#  can be used to tell the real thing from the trick above.
#
#  Minting a session needs root — logind's `CreateSession` is reserved for
#  PAM — so this uses `sudo -n`, which never prompts. It drops straight back
#  to `--uid=$(id -u)`; the command itself runs unprivileged.
#
#  ## The contract when it cannot
#
#  No sudo, no logind, no systemd-run: it says exactly which one is missing
#  and then **runs the command anyway, in place**. That is deliberate. The
#  alternative — failing, or skipping — would turn an environment that
#  produces eight legible failures into one that produces none, and this
#  repository's own progress notes record three occasions where a skip became
#  a green tick over nothing asserted. Every path through this script runs the
#  command exactly once.
#
#  ## Five things that were measured the hard way
#
#  * `--pty`, not `--pipe`. `--pipe` leaves a logind session record stuck in
#    `State=closing` after every run (ten of them accumulated during this
#    script's development, and `loginctl terminate-session` cannot remove a
#    record whose scope is already gone), classifies the session as
#    `Class=background`, and flaked once with "Connection timed out" in seven
#    tries. `--pty` gives `Class=user`, `Type=tty` and a controlling terminal
#    — so the peer is observed as `local-terminal` rather than `apex-shell`,
#    which is §7's first column exactly — and ten consecutive mints left the
#    session count unchanged.
#  * **stdin is `/dev/null`, and the redirect is applied INSIDE the session as
#    well as outside it.** Two separate reasons. First, `systemd-run --pty`
#    does not forward a piped stdin — the child blocks on a `read` that never
#    arrives, forever — and a wrapper that hangs in CI is worse than one that
#    fails, so callers must not expect to feed the command through it. Second,
#    `--pty` otherwise hands the command a terminal on fd 0, and code that
#    asks `isatty(0)` then answers differently in here than it does under a
#    plain `cargo test`: `apex/src/dispatch.rs`'s
#    `a_terminal_is_requested_only_when_there_is_one_to_forward` asserts
#    `tty_for_stdin() == Tty::None` and fails, 1881/1 instead of 1882/0.
#    Redirecting fd 0 does NOT give up the controlling terminal — `tty_nr` in
#    `/proc/self/stat` stays set, measured — so the peer is still observed as
#    `local-terminal`, which is the whole point of asking for a pty.
#    (Nothing in these suites reads its own stdin; they pipe into the client
#    binaries they invoke, which is unaffected.)
#  * The command runs via `bash -c '… exec "$@"'` rather than as the unit's
#    `ExecStart`. systemd could not exec a `#!` script here at all — exit 203,
#    from `/var/tmp`, from `$HOME`, from the worktree — while `exec` of the
#    same path from a shell inside the session works. The shell is needed for
#    `stty -onlcr` regardless: without it a pty turns every line ending into
#    `\r\n` and a suite's `grep -qF` on a line end stops matching.
#  * `XDG_RUNTIME_DIR` is deliberately NOT forwarded. pam_systemd sets it for
#    the session it just created, and that is the correct value; forwarding an
#    outer one would point the command at another session's runtime directory.
#  * Detection asks logind, not the cgroup string. `GetSessionByPID` answers
#    "PID … does not belong to any known session" for both a `user@` scope and
#    a `--unit=session-9999.scope` fake, and answers with a session object for
#    a real login — so it is the one test that cannot be talked into the wrong
#    answer by a process choosing its own name.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

note() { printf 'in-login-session: %s\n' "$1" >&2; }

if [ "$#" -eq 0 ]; then
    note "usage: $0 <command> [args…]"
    exit 2
fi

# The recursion guard is checked FIRST, before anything else can go wrong. A
# minted session that somehow still fails detection would otherwise mint
# another, and another.
if [ -n "${APEX_LOGIN_SESSION_WRAPPED:-}" ]; then
    exec "$@"
fi
export APEX_LOGIN_SESSION_WRAPPED=1

# ── are we already in one? ───────────────────────────────────────────────────
#
# Two questions, both of which have to answer yes, and they are not the same
# question. logind is asked whether this process belongs to a session it
# created — that is the unspoofable half. The cgroup is then checked for the
# shape `classify` actually reads, because that is what decides the daemon's
# answer, and a session logind knows about but whose cgroup does not say so
# would still be refused.
already_in_a_login_session() {
    command -v busctl >/dev/null 2>&1 || return 1
    busctl --system call org.freedesktop.login1 /org/freedesktop/login1 \
        org.freedesktop.login1.Manager GetSessionByPID u "$$" >/dev/null 2>&1 || return 1
    local cg
    cg="$(cat /proc/self/cgroup 2>/dev/null)" || return 1
    case "$cg" in
        *"/session-"*".scope"*) return 0 ;;
        *) return 1 ;;
    esac
}

if already_in_a_login_session; then
    note "already in logind session ${XDG_SESSION_ID:-?} ($(cat /proc/self/cgroup)); running here"
    exec "$@"
fi

# ── the preconditions for minting one ────────────────────────────────────────
missing=""
command -v sudo        >/dev/null 2>&1 || missing="sudo is not installed"
[ -n "$missing" ] || command -v systemd-run >/dev/null 2>&1 || missing="systemd-run is not installed"
[ -n "$missing" ] || [ -d /run/systemd/system ] || missing="this is not a systemd system"
[ -n "$missing" ] || [ -d /run/systemd/sessions ] || missing="systemd-logind is not running, so no login session can be created"
[ -n "$missing" ] || sudo -n true >/dev/null 2>&1 || missing="sudo -n does not work here, and creating a login session needs root (logind reserves CreateSession for PAM)"

if [ -n "$missing" ]; then
    note "cannot create a login session: ${missing}"
    note "running in place. The daemon will observe this as \`scheduled-job\` or"
    note "refuse to classify it, so every assertion that needs a human at this"
    note "machine — approving a request, holding a system-access grant — will"
    note "fail for that reason and not for the reason it is testing."
    exec "$@"
fi

# ── the environment the command needs, named rather than inherited ───────────
#
# A PAM session starts a fresh environment, so anything the command needs has
# to be passed explicitly. The list is deliberately an allowlist: forwarding
# the whole environment would carry the outer session's XDG_RUNTIME_DIR,
# DBUS_SESSION_BUS_ADDRESS and XDG_SESSION_ID into a session they do not
# belong to.
declare -a setenv=()
pass() {
    local name="$1" value
    [ -n "${!name+x}" ] || return 0
    value="${!name}"
    case "$value" in
        *$'\n'*) note "not forwarding ${name}: its value contains a newline" ; return 0 ;;
    esac
    setenv+=("--setenv=${name}=${value}")
}

for v in PATH HOME LANG LC_ALL TMPDIR; do pass "$v"; done
# TERM is forced rather than forwarded. Two reasons, and the second is not
# cosmetic: systemd 257+ writes an OSC 3008 session-tracking sequence to the
# pty when the unit's TERM is one it thinks can render it, and that sequence
# arrives glued to the front of the command's first line of output — measured,
# it is the unit's TERM that decides, not systemd-run's own. Everything this
# wrapper is for runs with its output captured to a file or a CI log, where a
# terminal type nothing is going to interpret buys colour codes and progress
# bars in a transcript and nothing else.
setenv+=("--setenv=TERM=dumb")
# Everything the toolchain and these suites steer with. Read out of the
# environment rather than listed, so a variable a suite grows tomorrow does
# not have to be added here.
while IFS= read -r -d '' kv; do
    case "$kv" in
        APEX_*|CARGO_*|RUST*|GITHUB_*|CI=*|ACTIONS_*|XDG_STATE_HOME=*|XDG_CONFIG_HOME=*|XDG_DATA_HOME=*)
            pass "${kv%%=*}" ;;
    esac
done < <(env -0)

# ── mint it ──────────────────────────────────────────────────────────────────
#
# `--wait` propagates the command's exit status; `--pty` is what makes logind
# call it a `user` session with a terminal. The inner shell reports what it
# actually got — not what was asked for — because the whole point of this
# script is that the environment is now different, and a log that only says
# "asked for a session" is not evidence that one was created.
#
# The marker file is not belt and braces. `systemd-run --wait` returns the
# COMMAND's exit status, so a unit that failed to start and a command that
# exited 1 are the same number, and starting one did fail once in seven tries
# during development ("Failed to start transient service unit: Remote peer
# disconnected"). Without the marker that failure would look like the tests
# having run and reported nothing — silence, which is the one outcome worse
# than a red run. The inner shell writes the marker before it execs, so an
# empty marker means the command never started and it is run in place instead.
# It is never run twice: the fallback is reached only when nothing wrote here.
marker="$(mktemp 2>/dev/null)" || marker=""

sudo -n env TERM=dumb systemd-run \
    --quiet --wait --pty --collect \
    --uid="$(id -u)" --gid="$(id -g)" \
    --property=PAMName=login \
    --working-directory="$PWD" \
    "${setenv[@]}" \
    -- /bin/bash -c '
        m="$1"; shift
        stty -onlcr 2>/dev/null
        [ -n "$m" ] && printf "ran\n" > "$m" 2>/dev/null
        printf "in-login-session: logind session %s, %s, uid %s\n" \
            "${XDG_SESSION_ID:-?}" "$(cat /proc/self/cgroup)" "$(id -u)" >&2
        exec "$@" </dev/null
    ' in-login-session "$marker" "$@" </dev/null
rc=$?

if [ -z "$marker" ] || [ -s "$marker" ]; then
    # It ran — or there was no marker to tell us otherwise, in which case the
    # exit status is the best evidence there is and second-guessing it would
    # risk running the command a second time.
    rm -f "$marker"
    exit "$rc"
fi

rm -f "$marker"
note "systemd-run exited ${rc} without starting the command, so no login session"
note "was entered. Running in place; the assertions that need a human at this"
note "machine will fail for that reason and not for the reason they are testing."
exec "$@"
