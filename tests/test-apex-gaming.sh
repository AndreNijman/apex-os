#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-gaming.sh — assertions against the REAL `apex` binary for
#  roadmap §12: per-game profiles (`apex game profile`) and boot-to-game
#  readiness (`apex gaming`).
#
#  ── Why this is a separate file from test-apex-modes.sh ─────────────────────
#  Same reason test-apex-resolve.sh is separate from test-apex-pkg.sh: it is a
#  separate claim. That suite asserts that `apex mode`, `apex workload` and
#  `apex perf` REPORT and never act. This one asserts that a profile is STORED
#  losslessly, that a stored profile resolves to one ordered plan, and that
#  applying it is gated. The cost of splitting is that the tripwire and its
#  negative control have to be repeated rather than inherited, so both are
#  repeated here in full — a separate file with an unarmed tripwire would be
#  worse than no separate file.
#
#  ── The harm this area has already done ─────────────────────────────────────
#  An earlier game-mode suite applied its plans through a live writer, which
#  shelled out to `scxctl` — a D-Bus client for scx_loader whose polkit action
#  is NOT passwordless. Running the tests raised a burst of "Authentication is
#  required to start, stop, or switch sched-ext schedulers" prompts on the
#  developer's own desktop and then blocked for 177 seconds on a password,
#  which read as a slow test suite rather than as a test reaching the host.
#
#  So this suite is built so that it CANNOT reach the machine, and then asserts
#  it:
#
#    * **Every** invocation redirects DBUS_SYSTEM_BUS_ADDRESS at a path that
#      does not exist. There is no case in this file that can reach a live
#      apexd, which also makes every assertion deterministic — otherwise the
#      same case would behave differently on a developer's laptop (apexd
#      running) and on a CI runner (no system bus at all).
#    * Every invocation gets a throwaway HOME and XDG_CONFIG_HOME, so the only
#      file any of it can write is one this script created.
#    * Fake `scxctl`, `nvidia-smi`, `systemctl`, `sudo`, `pkexec` and friends
#      sit first on PATH and the run FAILS if any is invoked — with a negative
#      control proving the fakes really are there, because without that every
#      isolation assertion below would pass vacuously.
#    * `steam`, `gamescope` and `mangoapp` are also faked, and that is an
#      assertion rather than a side effect: `apex gaming` must find them by
#      LOOKUP and never spawn one. If the readiness probe ever starts running
#      `steam --version` to identify it, this catches it on the first run.
#
#  ── One thing it cannot cover, and says so ──────────────────────────────────
#  `/usr/libexec/apex-session-select` is reached by ABSOLUTE path, so no amount
#  of PATH faking would intercept it. Nothing in this file's code paths calls
#  it — `apex gaming` only stats it — and that is asserted by the empty call
#  log rather than by PATH.
#
#  PASS = every verb behaves, every refusal refuses, the guard is proven to be
#         load-bearing, and no external command was spawned.
#
#  Run from anywhere:  ./tests/test-apex-gaming.sh
#  Uses $APEX_BIN if set; otherwise builds the binary with cargo.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e`, for the same reason as every other suite here: this one COUNTS
# failures instead of aborting, and many assertions run commands that exit
# non-zero on purpose. GitHub Actions invokes a script as `bash -e {0}`, and
# under `-e` the first such command ends the script — silently truncating the
# run rather than reporting anything, which is worse than a failure.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d /tmp/apex-gaming-test.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()      { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()     { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
section() { printf '\n── %s ──────────────────────────────\n' "$1"; }

# A missing prerequisite is a FAILURE, never a skip. A suite that reports
# "0 passed, 0 failed" and a green tick has asserted precisely nothing, which
# has happened in this repository three times.
command -v python3 >/dev/null 2>&1 || {
    echo "FATAL: python3 is required to validate the JSON output" >&2
    exit 2
}

# ── the binary under test ────────────────────────────────────────────────────
APEX="${APEX_BIN:-}"
if [ -z "$APEX" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "FATAL: no \$APEX_BIN and no cargo; this suite cannot test anything" >&2
        exit 2
    fi
    echo "building apex (set \$APEX_BIN to skip this)…"
    if ! cargo build -p apex --locked --manifest-path "${ROOT}/apexd/Cargo.toml" >&2; then
        echo "FATAL: could not build the apex binary" >&2
        exit 2
    fi
    APEX="${ROOT}/apexd/target/debug/apex"
fi
[ -x "$APEX" ] || { echo "FATAL: $APEX is not an executable" >&2; exit 2; }
echo "testing $APEX"

# ── fake tools: the outermost safety net, and an assertion in its own right ──
BIN="${WORK}/bin"; mkdir -p "$BIN"
CALLS="${WORK}/calls.log"; : > "$CALLS"
export APEX_TEST_CALLS="$CALLS"

# Anything that escalates privilege, unlocks a secret, switches the scheduler
# or reconfigures the session. Nothing in §12's code paths may invoke one of
# these, so any call at all is a failure — not a mode, not an argument, the
# invocation itself. `scxctl` is the one that mattered historically.
#
# `steam`, `gamescope` and `mangoapp` are in the same list on purpose. The
# readiness probe must find them with a PATH lookup and never run one: a probe
# that shelled out to identify a program would be slow, would inherit whatever
# that program does on startup, and in Steam's case would try to talk to a
# running client.
for tool in sudo pkexec su doas systemctl scxctl nvidia-smi secret-tool \
            loginctl busctl hyprctl wlr-randr rpm-ostree dnf dnf5 \
            steam gamescope mangoapp; do
    cat > "${BIN}/${tool}" <<FAKE
#!/bin/sh
echo "FORBIDDEN ${tool} \$*" >> "\$APEX_TEST_CALLS"
exit 1
FAKE
    chmod +x "${BIN}/${tool}"
done

# Prove the trap works before relying on it. Without this, a typo in the loop
# above would make every "nothing forbidden was called" assertion pass for the
# wrong reason — the exact shape of a vacuous test.
PATH="${BIN}:${PATH}" sudo -n true >/dev/null 2>&1
if grep -q '^FORBIDDEN sudo' "$CALLS"; then
    ok "the forbidden-tool trap records a call (self-test)"
else
    bad "the forbidden-tool trap records a call (self-test)" \
        "the fakes are not on PATH; every isolation assertion below would be vacuous"
fi
: > "$CALLS"

# And prove it specifically for `steam`, because that fake does double duty:
# the readiness probe is supposed to FIND it and not RUN it, and "the log is
# empty" only means something if running it would have written to the log.
PATH="${BIN}:${PATH}" steam -gamepadui >/dev/null 2>&1
if grep -q '^FORBIDDEN steam' "$CALLS"; then
    ok "the steam fake is on PATH and records a call (self-test)"
else
    bad "the steam fake is on PATH and records a call (self-test)" \
        "the readiness assertions about program lookups would be vacuous"
fi
: > "$CALLS"

# ── running apex against a throwaway machine ─────────────────────────────────
#
# DBUS_SYSTEM_BUS_ADDRESS points at a path that does not exist, on EVERY
# invocation. That is not belt-and-braces: it is what makes this file's
# assertions the same on a developer's desktop (where apexd is running and a
# property read would succeed) and on a CI runner (where there is no system bus
# at all). A suite whose expected output depends on which of those it is
# running on is a suite that will be "fixed" by whoever sees it fail.
DEAD_BUS="unix:path=${WORK}/no-such-bus"

newhome() {
    local h="${WORK}/$1"
    mkdir -p "${h}/.config" "${h}/.local/state"
    printf '%s' "$h"
}

# apex_in <home> <args...>
apex_in() {
    local h="$1"; shift
    env -i \
        HOME="$h" \
        XDG_CONFIG_HOME="${h}/.config" \
        XDG_STATE_HOME="${h}/.local/state" \
        PATH="${BIN}:/usr/bin:/bin" \
        DBUS_SYSTEM_BUS_ADDRESS="$DEAD_BUS" \
        APEX_TEST_CALLS="$CALLS" \
        "$APEX" "$@" 2>&1
}

# apex_env <home> <VAR=VALUE...> -- <args...>
apex_env() {
    local h="$1"; shift
    local -a extra=()
    while [ "${1:-}" != "--" ]; do extra+=("$1"); shift; done
    shift
    env -i \
        HOME="$h" \
        XDG_CONFIG_HOME="${h}/.config" \
        XDG_STATE_HOME="${h}/.local/state" \
        PATH="${BIN}:/usr/bin:/bin" \
        DBUS_SYSTEM_BUS_ADDRESS="$DEAD_BUS" \
        APEX_TEST_CALLS="$CALLS" \
        "${extra[@]}" \
        "$APEX" "$@" 2>&1
}

want() {  # name, expected substring, actual text
    local name=$1 substr=$2 text=$3
    if grep -qF -- "$substr" <<<"$text"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$substr"), got: $(head -3 <<<"$text" | tr '\n' ' ')"; fi
}
lacks() {  # name, forbidden substring, actual text
    local name=$1 substr=$2 text=$3
    if grep -qF -- "$substr" <<<"$text"; then
        bad "$name" "found $(printf '%q' "$substr") and should not have"
    else ok "$name"; fi
}
is() {
    local name=$1 wantv=$2 got=$3
    if [ "$got" = "$wantv" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$wantv"), got $(printf '%q' "$got")"; fi
}
# Pull a value out of the JSON with python, so a malformed document fails
# loudly instead of a grep quietly matching nothing.
jget() { python3 -c "$2" <<<"$1" 2>&1; }

# ─────────────────────────────────────────────────────────────────────────────
section "the games file lives beside the blueprint, inside the sandbox"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome storage)"
p="$(apex_in "$H" game profile path)"
is "profile path is the sandbox's own games.toml" "${H}/.config/apex/games.toml" "$p"
# The storage decision, as an executable claim: two files, one directory.
#
# `paths.user` and not `source`: `source` is null until a blueprint exists, so
# comparing against it would have made this pass for every possible value of
# the games path — which is what the first draft of this assertion did.
bp="$(jget "$(apex_in "$H" blueprint show --json)" \
     'import json,sys; print(json.load(sys.stdin)["paths"]["user"])')"
want "the blueprint path is readable, so this comparison means something" \
     "${H}/.config/apex/blueprint.toml" "$bp"
if [ "$p" != "$bp" ]; then
    ok "profiles are NOT stored in the blueprint"
else
    bad "profiles are NOT stored in the blueprint" "both resolve to $p"
fi
is "…but they sit in the same directory" "$(dirname "$bp")" "$(dirname "$p")"

out="$(apex_in "$H" game profile list)"
want "an empty machine says so rather than failing" "No per-game profiles" "$out"
if [ -e "${H}/.config/apex/games.toml" ]; then
    bad "listing nothing creates nothing" "the file was created by a read"
else
    ok "listing nothing creates nothing"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "storing a profile, and reading back exactly what was stored"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome store)"
out="$(apex_in "$H" game profile set 1091500 --title 'Cyberpunk 2077' \
       --mode gaming --tier balanced --fan manual:200 --note 'stutters on power-saver')"
want "set reports what it stored" "mode gaming, tier balanced, fan manual:200" "$out"

FILE="${H}/.config/apex/games.toml"
[ -f "$FILE" ] && ok "set wrote the file" || bad "set wrote the file" "no $FILE"

# The lossless round trip, through the built binary rather than only through
# a unit test: every field that survives validation comes back byte-identical
# in the JSON view.
j="$(apex_in "$H" game profile show 1091500 --json)"
is "show --json is valid JSON" "ok" \
   "$(jget "$j" 'import json,sys; json.load(sys.stdin); print("ok")')"
is "the title round-trips" "Cyberpunk 2077" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["title"])')"
is "the mode round-trips" "gaming" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["mode"])')"
is "the tier round-trips" "balanced" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["tier"])')"
is "the fan mode round-trips WITH its duty cycle" "manual:200" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["fan"])')"
is "the note round-trips" "stutters on power-saver" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["note"])')"

# A second write must reproduce the file exactly. A serialiser that normalised
# on the second pass would round-trip once and drift forever after.
before="$(cat "$FILE")"
apex_in "$H" game profile set 1091500 --note 'stutters on power-saver' >/dev/null
if [ "$before" = "$(cat "$FILE")" ]; then
    ok "rewriting the same values reproduces the file byte for byte"
else
    bad "rewriting the same values reproduces the file byte for byte" \
        "$(diff <(printf '%s' "$before") "$FILE" | head -4 | tr '\n' ' ')"
fi

# The file says about itself that a rewrite eats comments, because it does.
want "the file warns that set rewrites it" "REWRITE" "$(cat "$FILE")"
want "…and names the field that does survive" "note =" "$(cat "$FILE")"

# ─────────────────────────────────────────────────────────────────────────────
section "set is incremental — the bug a two-way merge would have"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome incremental)"
apex_in "$H" game profile set 620 --mode gaming --tier balanced >/dev/null
apex_in "$H" game profile set 620 --fan max >/dev/null
j="$(apex_in "$H" game profile show 620 --json)"
is "a later set does not drop an earlier tier" "balanced" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["tier"])')"
is "…and does store the new fan mode" "max" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["fan"])')"
# An empty value is the one way to clear a field.
apex_in "$H" game profile set 620 --tier '' >/dev/null
j="$(apex_in "$H" game profile show 620 --json)"
is "an empty value clears a field" "None" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["tier"])')"
is "…and leaves the others alone" "max" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["fan"])')"

# ─────────────────────────────────────────────────────────────────────────────
section "refusals — a bad value never becomes a stored profile"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome refuse)"
out="$(apex_in "$H" game profile set 620 --mode turbo)"; rc=$?
is "an unknown mode exits 2" 2 "$rc"
want "…and the refusal lists the real modes" "gaming" "$out"
if [ -e "${H}/.config/apex/games.toml" ]; then
    bad "a refused set writes nothing at all" "the file exists"
else
    ok "a refused set writes nothing at all"
fi

out="$(apex_in "$H" game profile set 620 --tier ultra)"; rc=$?
is "an unknown tier exits 2" 2 "$rc"
want "…and lists the real tiers" "power-saver" "$out"
out="$(apex_in "$H" game profile set 620 --fan loud)"; rc=$?
is "an unknown fan mode exits 2" 2 "$rc"
want "…and lists the real fan keywords" "manual:<0-255>" "$out"

# The two keys that are recognised only so the refusal can be useful. They can
# only arrive by hand-editing, which is exactly the case they exist for.
H="$(newhome refused-keys)"
mkdir -p "${H}/.config/apex"
printf '[game.620]\nscheduler = "scx_rusty"\n' > "${H}/.config/apex/games.toml"
out="$(apex_in "$H" game profile list)"; rc=$?
if [ "$rc" = 0 ]; then bad "a per-game scheduler is refused" "exited 0"
else ok "a per-game scheduler is refused"; fi
want "…and the refusal says where the setting really lives" "sysprofile" "$out"
want "…naming the key that does set it" "scx" "$out"

printf '[game.620]\ngpu = "locked"\n' > "${H}/.config/apex/games.toml"
out="$(apex_in "$H" game profile list)"
want "a per-game GPU policy is refused, with the reason" "nvidia" "$out"

# An unknown key is a typo, and a blueprint-style refusal is the whole point of
# a schema.
printf '[game.620]\nturbo = true\n' > "${H}/.config/apex/games.toml"
out="$(apex_in "$H" game profile list)"
want "an unknown key is refused rather than ignored" "turbo" "$out"

# And the file is never overwritten when it does not parse.
printf 'this is not toml [[[\n' > "${H}/.config/apex/games.toml"
out="$(apex_in "$H" game profile set 620 --fan max)"; rc=$?
if [ "$rc" = 0 ]; then bad "set refuses to touch a file it cannot read" "exited 0"
else ok "set refuses to touch a file it cannot read"; fi
is "…and the user's file is untouched" "this is not toml [[[" \
   "$(cat "${H}/.config/apex/games.toml")"

# ─────────────────────────────────────────────────────────────────────────────
section "hostile ids — these become TOML keys and reach argv"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome hostile)"
# Two things about the shape of these invocations, both learned the hard way.
#
# `--fan max` goes BEFORE the `--`. With it after, clap reads `--fan` as an
# unexpected positional and exits 2 — for EVERY id, legal ones included. The
# first draft of this loop had it after, so all seven cases passed without ever
# reaching the id check. A mutation that accepted every id reddened nothing
# here, which is what exposed it.
#
# And the exit code alone is not enough: 2 is also what clap returns for a
# usage error, so the refusal must be identified by its message too. Otherwise
# a future flag rename would silently turn this whole section vacuous again.
for badid in '-rf' 'a.b' 'x/y' 'a;b' 'a$b' '.hidden' '_lead'; do
    out="$(apex_in "$H" game profile set --fan max -- "$badid")"; rc=$?
    if [ "$rc" = 2 ] && grep -qF "refusing the id" <<<"$out"; then
        ok "the id '$badid' is refused by the id check"
    else
        bad "the id '$badid' is refused by the id check" \
            "exit $rc: $(head -1 <<<"$out")"
    fi
done
# The one whose consequence is corruption rather than a bad command line: a
# dot would make TOML read one profile as nested inside another.
out="$(apex_in "$H" game profile set --fan max -- 'a.b')"
want "a dotted id explains that TOML would nest it" "nested table" "$out"
if [ -e "${H}/.config/apex/games.toml" ]; then
    bad "no hostile id created a file" "the file exists"
else
    ok "no hostile id created a file"
fi
# …and the ones that are fine really are fine, or the check above would be
# refusing everything and passing for the wrong reason.
for goodid in '620' '1091500' 'my-old-shooter' 'Doom_1993'; do
    apex_in "$H" game profile set "$goodid" >/dev/null; rc=$?
    if [ "$rc" = 0 ]; then ok "the id '$goodid' is accepted"
    else bad "the id '$goodid' is accepted" "exit $rc"; fi
done

# ─────────────────────────────────────────────────────────────────────────────
section "the plan — one ordered list, with the reasons attached"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome plan)"
apex_in "$H" game profile set 1091500 --mode gaming --tier balanced --fan max >/dev/null
out="$(apex_in "$H" game profile show 1091500)"

# `show` must work with no daemon: the intent comes from the file, and only
# the delta needs apexd. This ran against a dead bus, so if the intent block is
# here, that separation holds.
want "show works with no daemon at all" "asks for (in order):" "$out"
want "…and says why the delta is unknown" "changes now: unknown" "$out"

steps="$(jget "$(apex_in "$H" game profile show 1091500 --json)" \
        'import json,sys; print("|".join(json.load(sys.stdin)["asks_for"]))')"
# THE ordering rule: entering game mode makes the daemon apply the
# SYSPROFILE's own [game] tier and fan_mode, so the per-title ones have to come
# after it or they are silently overwritten. Asserted positionally, because
# "the step is present" would pass with the order reversed.
gm="${steps%%game mode on*}"
if [ "$gm" != "$steps" ]; then
    ok "the plan enters game mode"
    after="${steps#*game mode on}"
    want "the tier is re-asserted AFTER game mode" "tier -> balanced" "$after"
    if [ "${steps##*|}" = "fan mode -> max" ]; then
        ok "the fan step is the very last one"
    else
        bad "the fan step is the very last one" "last step is ${steps##*|}"
    fi
else
    bad "the plan enters game mode" "$steps"
fi

want "the reason for the re-assert travels with the plan" "AFTER game mode" "$out"
want "the reason for the fan ordering does too" "fan step is last" "$out"

# What the profile relies on but cannot set is reported, the way §11's service
# sets are — not quietly assumed.
want "the scheduler is reported, not claimed" "not chosen per title" "$out"
want "…and so are the GPU clock locks" "game.nvidia" "$out"
want "the irqbalance conflict is reported too" "irqbalance.service" "$out"

# A profile naming no mode composes gaming, and says so rather than leaving it
# to be guessed.
H="$(newhome default-mode)"
out="$(apex_in "$H" game profile set 620)"
want "a profile with no mode composes gaming" "composes 'gaming'" "$out"
is "…and stores it as such" "gaming" \
   "$(jget "$(apex_in "$H" game profile show 620 --json)" \
      'import json,sys; print(json.load(sys.stdin)["mode"])')"

# A non-gaming profile must NOT claim the scheduler, or the report would be
# boilerplate rather than a measurement of the mode.
H="$(newhome couch)"
apex_in "$H" game profile set 620 --mode couch >/dev/null
out="$(apex_in "$H" game profile show 620)"
lacks "couch does not claim the sched-ext scheduler" "not chosen per title" "$out"
want "…and its intent says it leaves game mode" "game mode off" "$out"

# ─────────────────────────────────────────────────────────────────────────────
section "the launch command names only verbs this binary has"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome launch)"
apex_in "$H" game profile set 1091500 >/dev/null
c="$(apex_in "$H" game profile launch-command 1091500)"
is "launch-command prints the applying form" \
   "apex game profile apply 1091500 && %command%" "$c"
# The string is worthless if the verb it names does not exist, and a rename
# would otherwise leave a paste-ready command that does nothing.
verb="${c#apex }"; verb="${verb%% \&\&*}"
# shellcheck disable=SC2086 # deliberate word splitting of the parsed verb
out="$(apex_env "$H" APEX_MODE_NO_APPLY=1 -- ${verb})"; rc=$?
is "the verb it names is real (it refuses for the guard's reason)" 2 "$rc"
want "…and not for 'unrecognised subcommand'" "APEX_MODE_NO_APPLY is set" "$out"
# And `show` must say that nothing undoes it, since no launch wrapper ships.
want "show states that applying does not undo itself" "does not undo itself" \
     "$(apex_in "$H" game profile show 1091500)"

# ─────────────────────────────────────────────────────────────────────────────
section "the apply guard, and proof that it is load-bearing"
# ─────────────────────────────────────────────────────────────────────────────
H="$(newhome guard)"
apex_in "$H" game profile set 620 --mode gaming >/dev/null

out="$(apex_env "$H" APEX_MODE_NO_APPLY=1 -- game profile apply 620)"; rc=$?
is "the guard refuses with exit 2" 2 "$rc"
want "the guard says why" "APEX_MODE_NO_APPLY is set" "$out"

# The ordering proof. The bus is redirected to nothing on every invocation, so
# a guard checked AFTER the connection would report the bus failure instead.
# Only a guard checked BEFORE it can produce this message.
want "the guard is checked before the bus connection" "APEX_MODE_NO_APPLY is set" "$out"

# The negative control, and the whole reason the assertion above means
# anything: with the guard UNSET the same command must fail for a DIFFERENT
# reason. Without this, "the guard message appeared" would be consistent with a
# command that was failing anyway.
out="$(apex_in "$H" game profile apply 620)"; rc=$?
if [ "$rc" = 0 ]; then
    bad "without the guard the command really would act" "exited 0"
else
    ok "without the guard the command really would act"
fi
lacks "the guard is what short-circuits, not the dead bus" "APEX_MODE_NO_APPLY" "$out"
want "…and it fails on the bus instead" "cannot" "$out"

# The guard precedes even the file read, which is the strongest form of the
# claim: a profile that does not exist would otherwise be reported first.
out="$(apex_env "$H" APEX_MODE_NO_APPLY=1 -- game profile apply no-such-game)"; rc=$?
is "the guard precedes even the profile lookup" 2 "$rc"
want "…refusing for the guard's reason, not 'no such profile'" \
     "APEX_MODE_NO_APPLY is set" "$out"

# A dry run reaches the same wall and never panics.
out="$(apex_in "$H" game profile apply 620 --dry-run)"
want "a dry run with no daemon fails cleanly" "cannot" "$out"
lacks "a dry run never panics" "panicked" "$out"

# An unknown profile is a refusal, not a silent success.
out="$(apex_in "$H" game profile apply nope --dry-run)"; rc=$?
if [ "$rc" = 0 ]; then bad "applying an unknown profile refuses" "exited 0"
else ok "applying an unknown profile refuses"; fi
out="$(apex_in "$H" game profile remove nope)"; rc=$?
if [ "$rc" = 0 ]; then bad "removing an unknown profile refuses" "exited 0"
else ok "removing an unknown profile refuses"; fi

# remove takes exactly what it was asked for and nothing else.
H="$(newhome remove)"
apex_in "$H" game profile set 620 >/dev/null
apex_in "$H" game profile set 730 >/dev/null
apex_in "$H" game profile remove 620 >/dev/null
ids="$(jget "$(apex_in "$H" game profile list --json)" \
      'import json,sys; print(" ".join(g["id"] for g in json.load(sys.stdin)["profiles"]))')"
is "remove deletes only the named profile" "730" "$ids"

# Removing the LAST profile leaves a file that exists and holds no profiles —
# just the header and the schema version. That is correct rather than tidy, and
# it is asserted because the two states are easy to conflate: "no file" and "a
# file with nothing in it" must both read as nothing stored, or a user who
# removed their last profile would see a listing failure instead of an empty
# one.
apex_in "$H" game profile remove 730 >/dev/null
out="$(apex_in "$H" game profile list)"; rc=$?
is "listing after the last removal still exits 0" 0 "$rc"
want "…and reports nothing stored" "No per-game profiles" "$out"
if [ -f "${H}/.config/apex/games.toml" ]; then
    ok "…even though the file itself remains"
else
    bad "…even though the file itself remains" "remove deleted the file"
fi
is "…and the emptied file still parses" "0" \
   "$(jget "$(apex_in "$H" game profile list --json)" \
      'import json,sys; print(len(json.load(sys.stdin)["profiles"]))')"

# ─────────────────────────────────────────────────────────────────────────────
section "§12 readiness — measured against fixture machines"
# ─────────────────────────────────────────────────────────────────────────────
# A Gaming edition, built as files rather than described in prose, so the
# assertions below are about a tree that has the same shape as a real image.
mkgaming() {
    local r=$1
    mkdir -p "$r/usr/share/wayland-sessions" "$r/usr/libexec" \
             "$r/etc/sudoers.d" "$r/etc/security/limits.d" "$r/var/lib/apex-greet" \
             "$r/sys/class/input"
    printf '[Desktop Entry]\nName=APEX Gaming Mode\n' \
        > "$r/usr/share/wayland-sessions/apex-gaming.desktop"
    printf '#!/usr/bin/env bash\n' > "$r/usr/libexec/apex-gaming-session"
    printf '#!/usr/bin/env bash\n' > "$r/usr/libexec/apex-session-select"
    chmod +x "$r/usr/libexec/apex-gaming-session" "$r/usr/libexec/apex-session-select"
    printf '%%wheel ALL=(root) NOPASSWD: /usr/libexec/apex-session-select *\n' \
        > "$r/etc/sudoers.d/040-apex-session-select"
    printf '@wheel - rtprio 20\n' > "$r/etc/security/limits.d/30-apex-gaming-rtprio.conf"
    printf 'apex-gaming' > "$r/var/lib/apex-greet/last-session"
}

# The capability bitmap for BTN_GAMEPAD (0x130 = 304), printed the way the
# kernel prints it TO THIS PROCESS. The word width belongs to the reader, not
# to the kernel, so it is computed rather than hard-coded — the same reason the
# Rust side takes the width explicitly.
mkpad() {
    local dir=$1 name=$2
    mkdir -p "$dir/capabilities"
    python3 - "$dir" "$name" <<'PY'
import ctypes, pathlib, sys
d, name = pathlib.Path(sys.argv[1]), sys.argv[2]
w = ctypes.sizeof(ctypes.c_void_p) * 8
bit = 0x130
words = ["%x" % (1 << (bit % w))] + ["0"] * (bit // w)
(d / "capabilities" / "key").write_text(" ".join(words) + "\n")
(d / "name").write_text(name + "\n")
PY
}

H="$(newhome ready)"
GAMING="${WORK}/root-gaming"; mkgaming "$GAMING"
PREMERGE="${WORK}/root-premerge"; mkdir -p "$PREMERGE/var/lib/apex-greet" "$PREMERGE/sys/class/input"
printf 'hyprland' > "$PREMERGE/var/lib/apex-greet/last-session"

j="$(apex_env "$H" "APEX_ROOT=$GAMING" -- gaming --json)"
is "gaming --json is valid JSON" "ok" \
   "$(jget "$j" 'import json,sys; json.load(sys.stdin); print("ok")')"
is "a complete Gaming edition reads as ready" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["ready"])')"
is "…and as set to boot into the game" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["boots_to_game"])')"
is "…and reports the greeter's own record" "apex-gaming" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["preselected_session"])')"

# THE second-switch assertion. Under a fixture root, program presence must be
# UNMEASURED — no filesystem root can redirect a PATH lookup, and this suite
# has fake steam/gamescope first on PATH, so a leak would report them present
# and the same assertion would pass or fail depending on the runner.
is "a fixture root does not measure program presence" "None" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["checks"]["steam"]["value"])')"
is "…and says why" "False" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["probes_programs"])')"
# `.get`, not `[...]`: when the value IS measured there is no `unavailable`
# key, and a KeyError traceback is a correct red that reads like a broken
# suite. The assertion is the same; only the failure output is legible.
want "…with the reason naming the PATH problem" "PATH lookup" \
     "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["checks"]["steam"].get("unavailable"))')"

# APEX publishes ONE image, so a root with no gaming session is not "Daily" —
# it is a machine still booting a pre-merge image, or a damaged one. The blocker
# has to say that, and must not read like something `apex install` can fix.
j="$(apex_env "$H" "APEX_ROOT=$PREMERGE" -- gaming --json)"
is "an image without the gaming session is not ready" "False" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["ready"])')"
is "…and is not set to boot into the game" "False" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["boots_to_game"])')"
want "…and points at an OS update, not a package" "apex update" \
     "$(jget "$j" 'import json,sys; print(" | ".join(json.load(sys.stdin)["blockers"]))')"
is "…and names no edition, because there are none" "0" \
   "$(jget "$j" 'import json,sys; print(" | ".join(json.load(sys.stdin)["blockers"]).count("Gaming edition"))')"
rc_out="$(apex_env "$H" "APEX_ROOT=$PREMERGE" -- gaming >/dev/null 2>&1; echo $?)"
is "gaming exits non-zero when Gaming Mode would not start" 1 "$rc_out"
rc_out="$(apex_env "$H" "APEX_ROOT=$GAMING" -- gaming >/dev/null 2>&1; echo $?)"
is "…and zero when it would" 0 "$rc_out"

# Blockers and warnings are different claims, and conflating them would make
# the verb useless: a machine missing the switch helper still runs Gaming Mode.
NOSWITCH="${WORK}/root-noswitch"; mkgaming "$NOSWITCH"
rm -f "$NOSWITCH/usr/libexec/apex-session-select"
j="$(apex_env "$H" "APEX_ROOT=$NOSWITCH" -- gaming --json)"
is "a missing switch helper is not a blocker" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["ready"])')"
want "…it is a warning that says what is lost" "power menu" \
     "$(jget "$j" 'import json,sys; print(" | ".join(json.load(sys.stdin)["warnings"]))')"

# The failure a COPY without --chmod=0755 produces: the session file exists and
# cannot start. That IS a blocker, and distinguishing it from "missing" is the
# point of checking the mode.
NOEXEC="${WORK}/root-noexec"; mkgaming "$NOEXEC"
chmod 0644 "$NOEXEC/usr/libexec/apex-gaming-session"
j="$(apex_env "$H" "APEX_ROOT=$NOEXEC" -- gaming --json)"
is "a session script that is not executable blocks" "False" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["ready"])')"
want "…and says it is not executable" "not executable" \
     "$(jget "$j" 'import json,sys; print(" | ".join(json.load(sys.stdin)["blockers"]))')"

# ─────────────────────────────────────────────────────────────────────────────
section "controllers, read from sysfs and not from /dev"
# ─────────────────────────────────────────────────────────────────────────────
PAD="${WORK}/root-pad"; mkgaming "$PAD"
mkpad "$PAD/sys/class/input/input5" "Microsoft X-Box 360 pad"
j="$(apex_env "$H" "APEX_ROOT=$PAD" -- gaming --json)"
is "a gamepad is found by its capability bitmap" "Microsoft X-Box 360 pad" \
   "$(jget "$j" 'import json,sys; print(",".join(json.load(sys.stdin)["gamepads"]))')"

# A keyboard sets plenty of low bits and nothing at 0x130, so the kernel elides
# every word above word 0 — which is why the reader indexes from the RIGHT.
KBD="${WORK}/root-kbd"; mkgaming "$KBD"
mkdir -p "$KBD/sys/class/input/input3/capabilities"
printf '40000000\n' > "$KBD/sys/class/input/input3/capabilities/key"
printf 'AT Translated Set 2 keyboard\n' > "$KBD/sys/class/input/input3/name"
j="$(apex_env "$H" "APEX_ROOT=$KBD" -- gaming --json)"
is "a keyboard is not counted as a gamepad" "" \
   "$(jget "$j" 'import json,sys; print(",".join(json.load(sys.stdin)["gamepads"]))')"
is "…and a machine with no controller is still ready" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["ready"])')"
want "…with the missing controller reported as a warning" "no input device" \
     "$(jget "$j" 'import json,sys; print(" | ".join(json.load(sys.stdin)["warnings"]))')"

# A container has no /sys/class/input at all, and that is not "no controller".
NOINPUT="${WORK}/root-noinput"; mkgaming "$NOINPUT"
rmdir "$NOINPUT/sys/class/input"
j="$(apex_env "$H" "APEX_ROOT=$NOINPUT" -- gaming --json)"
is "no /sys/class/input is unavailable, not empty" "None" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["gamepads"])')"

# ─────────────────────────────────────────────────────────────────────────────
section "program presence is a LOOKUP, never a spawn"
# ─────────────────────────────────────────────────────────────────────────────
# With no APEX_ROOT the probe measures PATH for real, and this suite's fake
# steam/gamescope/mangoapp are first on it. So they must come back present —
# which proves the lookup happens — while the call log stays empty, which
# proves nothing was executed. Neither half means much without the other.
: > "$CALLS"
j="$(apex_in "$H" gaming --json)"
is "the real probe does measure PATH" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["probes_programs"])')"
is "…and finds steam there" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["checks"]["steam"]["value"])')"
is "…and gamescope" "True" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["checks"]["gamescope"]["value"])')"
if [ -s "$CALLS" ]; then
    bad "…without running either of them" "$(tr '\n' ';' < "$CALLS")"
else
    ok "…without running either of them"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "the tripwire: nothing in §12 spawned an external command"
# ─────────────────────────────────────────────────────────────────────────────
# Every invocation above ran with fake scxctl / nvidia-smi / systemctl / sudo /
# pkexec / steam / gamescope first on PATH. `scxctl` is the one that raised
# polkit prompts and switched the developer's scheduler; `systemctl` would mean
# a mode's service sets had stopped being a report and become an action.
if [ -s "$CALLS" ]; then
    bad "no forbidden command was spawned by any verb" "$(tr '\n' ';' < "$CALLS")"
else
    ok "no forbidden command was spawned by any verb"
fi

# The negative control for the whole file, repeated at the end because the log
# was truncated in the middle of the run. An empty file proves nothing unless
# writing to it still works.
PATH="${BIN}:${PATH}" scxctl switch -s scx_lavd >/dev/null 2>&1
if grep -q '^FORBIDDEN scxctl' "$CALLS"; then
    ok "the tripwire is still armed at the end of the run (negative control)"
else
    bad "the tripwire is still armed at the end of the run (negative control)" \
        "the fakes were not on PATH; every spawn check above was vacuous"
fi

# Nothing may have escaped the sandbox into the developer's own configuration.
if [ -e "${HOME:-/nonexistent}/.config/apex/games.toml" ]; then
    # Only a failure if this run created it, which is why the check is on the
    # mtime rather than on existence.
    if [ "${HOME}/.config/apex/games.toml" -nt "$WORK" ]; then
        bad "the suite never wrote the developer's own games file" \
            "${HOME}/.config/apex/games.toml was modified during this run"
    else
        ok "the suite never wrote the developer's own games file"
    fi
else
    ok "the suite never wrote the developer's own games file"
fi

echo
printf 'apex-gaming: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
