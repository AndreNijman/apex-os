#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-browser.sh — assertions against the SHIPPED browser capsule
#  engine, files/system/libexec/apex-browser. Nothing here re-implements it:
#  every case runs the script as a process.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `apex browser run` starts an agent session and then deletes a directory
#  recursively. Neither should happen for real during a test: this repository
#  has already had a suite switch the live CPU scheduler and another raise a
#  keyring dialog, both because a test reached the host. A suite that started
#  real sessions on somebody's laptop, or launched a browser on it, would be
#  the same mistake with a window attached.
#
#  So the AGENT RUNTIME IS FAKED. $PATH is not reduced — the engine takes its
#  `apex` from $APEX_BROWSER_APEX, which exists for this — and the stub records
#  the exact argv, answers the three questions the engine asks, and creates the
#  files a real capsule's browser would have written.
#
#  ── The thing this suite is really about ────────────────────────────────────
#  Two argvs and one file.
#
#  The SESSION argv, because that is where the confinement is asked for. This
#  engine enforces nothing itself; it asks for `--sandbox project --network
#  allowlist`, and a dropped word there is a capsule with a home that is not
#  masked or a network that is not filtered — reported by this command as
#  though it were neither. It would not be a compile error and it would not be
#  visible in the output.
#
#  The BROWSER argv, because `--headless`, `--no-remote` and the capsule's own
#  `--profile` are the three that decide whether a window can appear, whether
#  the run can attach to the user's running browser, and which cookie jar it
#  uses.
#
#  And user.js, because Firefox does not read $HTTP_PROXY: those preferences
#  are the only thing pointing the browser at the egress bridge, so they are
#  part of the security surface rather than configuration.
#
#  ── What is asserted ────────────────────────────────────────────────────────
#    * the session argv, word by word, and that no weaker sandbox or network
#      can reach it
#    * the browser argv, including that the capsule's own --profile is there
#    * user.js: the proxy, the pinned download directory, and no "ask me"
#    * the nomination loop: exactly the named file leaves, an unnominated one
#      does not, and without --download-to nothing leaves at all
#    * refusals: no destination, a traversing nomination, a destination inside
#      the capsule root, --profile or -display among the browser's arguments,
#      and a destination the runtime has not allowed
#    * the three answers a capability lookup can give, which are not two
#    * the removal fences, including that a name pointing outside the root is
#      refused rather than removed
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  No session is started, no browser is launched, no window can appear, nothing
#  is written outside a temp directory. The live half — a real capsule, a real
#  bwrap namespace and a real browser — is tests/browserlab/run-browserlab,
#  which reports could-not-run with a reason on a machine that lacks the parts.
#
#  PASS = every case prints exactly what it should, with a non-zero exit where
#         one is expected.
#
#  Run from anywhere: ./tests/test-apex-browser.sh
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`, as in every suite here: this one COUNTS failures instead of
# aborting, and many assertions run commands that exit non-zero on purpose.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-browser
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }
ENGINE=$(cd "$(dirname "$ENGINE")" && pwd)/$(basename "$ENGINE")

WORK=$(mktemp -d /tmp/apex-browser-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0

ok()  { printf 'PASS  %-62s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-62s %s\n' "$1" "$2"; fail=$((fail+1)); }

is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}
has() {
    local name=$1 want=$2 hay=$3
    if grep -qF -- "$want" <<<"$hay"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want") in: $(head -5 <<<"$hay" | tr '\n' ' ')"; fi
}
hasnt() {
    local name=$1 unwanted=$2 hay=$3
    if grep -qF -- "$unwanted" <<<"$hay"; then
        bad "$name" "found $(printf '%q' "$unwanted") and should not have"
    else ok "$name"; fi
}
exits() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected exit $want, got $got"; fi
}

# ── the sandbox the whole suite runs in ─────────────────────────────────────
BIN="$WORK/bin"
ROOT="$WORK/capsules"
CALLS="$WORK/calls"
LAND="$WORK/landing"
mkdir -p "$BIN" "$ROOT" "$LAND"

# The `apex` the engine calls. Recording, and it also plays the part of the
# capsule: a real session's browser writes into the capsule's own download
# directory, so the stub writes the two files every nomination case needs —
# one that will be nominated and one that never is.
#
# It answers exactly the four questions the engine asks and nothing else, so a
# question the engine starts asking later fails loudly here rather than being
# absorbed.
cat > "$BIN/apex" <<EOF
#!/usr/bin/env bash
{ printf 'apex'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
case "\$1 \$2" in
    "agent allow")
        # A runtime that cannot be asked is a mode, because "nothing is
        # allowed" and "nobody answered" lead to different advice.
        [ "\${FAKE_ALLOW_DOWN:-0}" = 1 ] && {
            printf 'apex: the agent runtime is not running.\n' >&2; exit 1; }
        # With no destination this lists. The suite controls what is on it.
        [ \$# -eq 2 ] && { cat "$WORK/allowlist" 2>/dev/null; exit 0; }
        exit 0 ;;
    "agent run")
        # Find --cwd and behave like a capsule whose browser ran.
        cwd=""; prev=""
        for a in "\$@"; do
            [ "\$prev" = "--cwd" ] && cwd="\$a"
            prev="\$a"
        done
        if [ -n "\$cwd" ]; then
            printf 'BROWSERLAB-REPORT\n' > "\$cwd/report.csv"
            printf 'NEVER-NOMINATED\n'   > "\$cwd/leak.txt"
        fi
        printf 'session 7 — generic in %s\n' "\$cwd"
        exit \${FAKE_RUN_RC:-0} ;;
    "agent status")
        # Silence is a mode, because "the runtime did not answer" is a
        # different fact from "the session finished" and the engine has to
        # tell them apart.
        [ "\${FAKE_STATUS_SILENT:-0}" = 1 ] && exit 1
        printf 'session      7\nstate        %s\noutcome      exited 0\n' "\${FAKE_STATUS_STATE:-complete}"
        exit 0 ;;
    "agent logs")
        printf 'FAKE CAPSULE TRANSCRIPT\n'; exit 0 ;;
    "agent kill"|"agent rm") exit 0 ;;
    "secret list")
        # Three answers, and the suite picks which one by setting these.
        [ "\${FAKE_SECRETD_DOWN:-0}" = 1 ] && {
            printf 'apex secret: the secret service is not running.\n' >&2
            exit 1; }
        cat "$WORK/secrets" 2>/dev/null
        exit 0 ;;
esac
printf 'stub apex: unhandled %s\n' "\$*" >&2
exit 99
EOF
chmod +x "$BIN/apex"

# A browser that is never run by this suite — the stub `apex agent run` stands
# in for the whole session — but which must EXIST, because the engine refuses
# to build a capsule for a browser that is not there.
printf '#!/usr/bin/env bash\nexit 0\n' > "$BIN/browser"
chmod +x "$BIN/browser"

export APEX_BROWSER_ROOT="$ROOT"
export APEX_BROWSER_APEX="$BIN/apex"
export APEX_BROWSER_BIN="$BIN/browser"

printf 'e.example:443\n127.0.0.1:9443\nintranet.example\n' > "$WORK/allowlist"
printf 'intranet intranet.example\n' > "$WORK/secrets"

reset_calls() { : > "$CALLS"; rm -rf "${ROOT:?}"/* 2>/dev/null; }

# session_argv — the single `apex agent run` line the engine emitted.
session_argv() { grep -F 'apex <agent> <run>' "$CALLS" | head -1; }

echo "── the session the engine asks for ─────────────────────────────────────"

reset_calls
out=$("$ENGINE" run --name cap-one --allow e.example:443 \
        -- --screenshot shot.png https://e.example/ 2>&1)
argv=$(session_argv)

has "the session is a generic-adapter one"          '<--agent> <generic>'   "$argv"
has "it asks for the confined sandbox"              '<--sandbox> <project>' "$argv"
has "it asks for the allowlisted network"           '<--network> <allowlist>' "$argv"
has "its working directory is the capsule"          "<--cwd> <$ROOT/cap-one>" "$argv"
has "it detaches rather than taking a terminal"     '<--detach>'            "$argv"
# `strict` is `project` with the network removed, and the CLI refuses it
# alongside `--network allowlist`. An engine that asked for it would produce a
# capsule with no route at all while reporting a destination policy.
hasnt "it never asks for the strict sandbox"        '<--sandbox> <strict>'  "$argv"
hasnt "and never for an open network"               '<--network> <open>'    "$argv"
hasnt "and never for an unconfined session"         '<unrestricted>'        "$argv"

echo "── the browser the session runs ────────────────────────────────────────"

has "the browser is told to be headless"            '<--headless>'          "$argv"
has "and not to join a running browser"             '<--no-remote>'         "$argv"
has "and to use the capsule's own profile"          "<--profile> <$ROOT/cap-one/.profile>" "$argv"
has "the caller's own arguments survive"            '<--screenshot> <shot.png>' "$argv"

# {capsule} exists because Firefox's --screenshot writes NOTHING when given a
# relative filename — measured — and a caller cannot type the path of a
# capsule this engine names for them.
argv2=$(reset_calls; "$ENGINE" run --name cap-tok --allow e.example:443 \
            -- --screenshot '{capsule}/shot.png' https://e.example/ >/dev/null 2>&1; session_argv)
has   "{capsule} becomes the capsule's own directory" "<--screenshot> <$ROOT/cap-tok/shot.png>" "$argv2"
hasnt "and no unexpanded token reaches the browser"   '{capsule}' "$argv2"
has "including the URL"                             '<https://e.example/>'  "$argv"

echo "── user.js, which is the only thing pointing at the bridge ─────────────"

# Read from a capsule the suite keeps, because a real run deletes it. --timeout
# 0 is refused, so the capsule is kept by pointing the stub at a failure: the
# engine still writes the profile before it starts the session.
reset_calls
prof=""
FAKE_RUN_RC=3 "$ENGINE" run --name cap-prof --allow e.example:443 -- https://e.example/ >/dev/null 2>&1
# The capsule is removed on every exit path, so the profile is read from the
# stub's own record of what the browser was pointed at instead: the engine
# writes user.js before it starts the session, and the suite re-creates the
# same call with the directory left in place by pre-making it.
mkdir -p "$ROOT/cap-keep"
prof="$ROOT/cap-keep/.profile"
# Source the engine's writer directly. Sourcing, not re-implementing: this is
# the function the image runs.
( set +u
  APEX_BROWSER_ROOT="$ROOT"
  # shellcheck disable=SC1090
  source "$ENGINE" >/dev/null 2>&1
  write_profile "$prof" "$ROOT/cap-keep" ) >/dev/null 2>&1
userjs=$(cat "$prof/user.js" 2>/dev/null)

has "the proxy is switched on at all"       'network.proxy.type", 1'            "$userjs"
has "http goes to the bridge"               'network.proxy.http", "127.0.0.1"'  "$userjs"
has "https goes to the bridge"              'network.proxy.ssl", "127.0.0.1"'   "$userjs"
has "on the bridge's port"                  'network.proxy.ssl_port", 3128'     "$userjs"
# Firefox bypasses the proxy for loopback by default, and inside the namespace
# loopback is exactly where the bridge is — so the default would skip the one
# reachable address.
has "the loopback bypass is turned off"     'allow_hijacking_localhost", true'  "$userjs"
has "and nothing is exempted from it"       'no_proxies_on", ""'                "$userjs"
has "downloads are pinned to the capsule"   "browser.download.dir\", \"$ROOT/cap-keep\"" "$userjs"
has "and are never asked about"             'always_ask_before_handling_new_types", false' "$userjs"
has "the updater is off"                    'app.update.enabled", false'        "$userjs"
has "telemetry is off"                      'toolkit.telemetry.enabled", false' "$userjs"
# Found by the live lab: with only the telemetry preferences, the daemon
# logged a capsule being denied firefox.settings.services.mozilla.com,
# aus5.mozilla.org and location.services.mozilla.com, over and over, for the
# rest of the run. The allowlist refused all of it — but an allowlist that
# does not describe what the capsule talks to is doing the profile's job.
has "the settings service is not contacted"  'services.settings.server", ""'         "$userjs"
has "nor the geolocation one"                'geo.enabled", false'                   "$userjs"
has "nor the captive-portal probe"           'captive-portal-service.enabled", false' "$userjs"
has "nor safebrowsing"                       'safebrowsing.malware.enabled", false'  "$userjs"
rm -rf "${ROOT:?}/cap-keep"

echo "── the nomination loop ─────────────────────────────────────────────────"

reset_calls
rm -rf "${LAND:?}"/*
out=$("$ENGINE" run --name cap-dl --allow e.example:443 \
        --download report.csv --download-to "$LAND" \
        -- https://e.example/ 2>&1)
if [ -f "$LAND/report.csv" ]; then ok "the nominated file arrives"
else bad "the nominated file arrives" "nothing at $LAND/report.csv"; fi
if [ -e "$LAND/leak.txt" ]; then
    bad "an unnominated file stays in the capsule" "leak.txt arrived and should not have"
else ok "an unnominated file stays in the capsule"; fi
has "and the count says so"  '1 of 1 nominated files left the capsule' "$out"

# Default-deny at the other end.
reset_calls
rm -rf "${LAND:?}"/*
out=$("$ENGINE" run --name cap-nodl --allow e.example:443 \
        --download report.csv -- https://e.example/ 2>&1)
has "without a destination nothing leaves" 'nothing left the capsule' "$out"
if [ -e "$LAND/report.csv" ]; then
    bad "and no destination is invented" "a file appeared at $LAND"
else ok "and no destination is invented"; fi

# The no-clobber rule.
reset_calls
rm -rf "${LAND:?}"/*
printf 'MINE\n' > "$LAND/report.csv"
out=$("$ENGINE" run --name cap-clob --allow e.example:443 \
        --download report.csv --download-to "$LAND" -- https://e.example/ 2>&1)
has "an existing file at the destination is not replaced" 'not overwriting it' "$out"
is  "and its contents are untouched" "MINE" "$(cat "$LAND/report.csv")"
reset_calls
out=$("$ENGINE" run --name cap-force --allow e.example:443 --force \
        --download report.csv --download-to "$LAND" -- https://e.example/ 2>&1)
is  "--force replaces it" "BROWSERLAB-REPORT" "$(cat "$LAND/report.csv")"
rm -rf "${LAND:?}"/*

echo "── the wait loop's states are the runtime's ────────────────────────────"

# protocol.rs::AgentState prints complete, failed and exited as its terminal
# names. `exited` is how a session that ended without publishing a completion
# reads, and the first version of this loop did not list it — such a capsule
# was held until its timeout and then reported as one. A five-second timeout
# makes the difference between "it broke out" and "it waited" unmistakable.
reset_calls
rm -rf "${LAND:?}"/*
start=$SECONDS
out=$(FAKE_STATUS_STATE=exited "$ENGINE" run --name cap-exited --timeout 5 \
        --allow e.example:443 --download report.csv --download-to "$LAND" \
        -- https://e.example/ 2>&1)
took=$((SECONDS - start))
if [ "$took" -lt 4 ]; then ok "a session in state 'exited' ends the wait at once"
else bad "a session in state 'exited' ends the wait at once" "waited ${took}s of a 5s timeout"; fi
hasnt "and is not reported as a timeout" 'did not finish within' "$out"
if [ -f "$LAND/report.csv" ]; then ok "and its nominated file still leaves"
else bad "and its nominated file still leaves" "nothing at $LAND/report.csv"; fi
rm -rf "${LAND:?}"/*

# The third answer. A runtime that stops answering is not a capsule that
# finished, and copying out on that assumption would read a directory the
# browser may still be writing into.
reset_calls
out=$(FAKE_STATUS_SILENT=1 "$ENGINE" run --name cap-silent --timeout 60 \
        --allow e.example:443 --download report.csv --download-to "$LAND" \
        -- https://e.example/ 2>&1); rc=$?
has   "a runtime that stops answering is its own outcome" 'stopped answering' "$out"
hasnt "and is not called a finished capsule"              'nominated files left the capsule' "$out"
if [ -e "$LAND/report.csv" ]; then
    bad "and nothing is copied out on that assumption" "report.csv was copied anyway"
else ok "and nothing is copied out on that assumption"; fi
if [ -e "$ROOT/cap-silent" ]; then
    bad "and the capsule is still torn down" "$ROOT/cap-silent survived"
else ok "and the capsule is still torn down"; fi
rm -rf "${LAND:?}"/*

# A state the runtime does not have must not appear in the engine: an arm
# that can never fire is this repository's dominant defect class.
if grep -qE '^[^#]*\|killed\)' "$ENGINE"; then
    bad "the loop waits on no state the runtime cannot produce" "'killed' is not an AgentState"
else ok "the loop waits on no state the runtime cannot produce"; fi

echo "── refusals ────────────────────────────────────────────────────────────"

reset_calls
out=$("$ENGINE" run -- https://e.example/ 2>&1); rc=$?
has   "a capsule with no destination is refused"     'must name where it may go' "$out"
exits "and it is an error rather than a refusal"     1 "$rc"

out=$("$ENGINE" run --allow e.example:443 --download ../escape --download-to "$LAND" -- https://e.example/ 2>&1)
has "a traversing nomination is refused"             'plain filename' "$out"
out=$("$ENGINE" run --allow e.example:443 --download '*' --download-to "$LAND" -- https://e.example/ 2>&1)
has "a wildcard nomination is refused"               'no wildcards' "$out"
out=$("$ENGINE" run --allow e.example:443 --download report.csv --download-to "$ROOT/inside" -- https://e.example/ 2>&1)
has "a destination inside the capsule root is refused" 'outside' "$out"
out=$("$ENGINE" run --allow e.example:443 --download-to "$LAND" -- https://e.example/ 2>&1)
has "a destination with nothing nominated is refused" 'nothing would leave' "$out"

# The one a test found rather than a reading: clap's trailing var-arg absorbs
# an unrecognised option into the browser's arguments, so `--profile` arrives
# here AFTER the separator — and a second --profile wins on a Firefox command
# line.
out=$("$ENGINE" run --allow e.example:443 -- --profile /home/u/.mozilla https://e.example/ 2>&1); rc=$?
has   "a --profile among the browser's arguments is refused" 'cannot be passed' "$out"
exits "and it is a REFUSAL, not an ordinary error"           2 "$rc"
out=$("$ENGINE" run --allow e.example:443 -- -P other https://e.example/ 2>&1)
has "so is Firefox's short spelling of it"                   'cannot be passed' "$out"
# Measured, not assumed, and the measurement found a hole: `firefox
# --profile=DIR` creates and uses a profile at DIR, so a refusal listing only
# the space-separated form was one character from being walked around.
out=$("$ENGINE" run --allow e.example:443 -- --profile=/home/u/.mozilla https://e.example/ 2>&1)
has "and the = spelling Firefox also accepts"                'cannot be passed' "$out"
out=$("$ENGINE" run --allow e.example:443 -- --display=:0 https://e.example/ 2>&1)
has "the display refusal covers its = spelling too"          'no compositor' "$out"
out=$("$ENGINE" run --allow e.example:443 -- -display :0 https://e.example/ 2>&1)
has "and a display is refused with the reason there is none" 'no compositor' "$out"

out=$("$ENGINE" run --allow nowhere.example:443 -- https://nowhere.example/ 2>&1)
has "a destination the runtime has not allowed is refused"   'apex agent allow nowhere.example:443' "$out"

# The same question, unanswerable. An empty list read as "nothing is allowed"
# would refuse every destination with advice for a problem the caller does not
# have — "permission denied is not absence", one more time.
out=$(FAKE_ALLOW_DOWN=1 "$ENGINE" run --allow e.example:443 -- https://e.example/ 2>&1)
has   "a runtime that cannot be asked is not read as an empty allowlist" 'could not be asked' "$out"
has   "and the advice is to start it"                                    'apex-agentd' "$out"
hasnt "rather than to add the destination"                               'apex agent allow e.example:443' "$out"

echo "── the capability lookup has three answers, not two ────────────────────"

reset_calls
out=$(FAKE_SECRETD_DOWN=1 "$ENGINE" run --capability intranet -- https://intranet.example/ 2>&1)
has   "a service that could not be asked says exactly that"  'could not be asked' "$out"
has   "and names the fix for THAT case"                      'apex-secretd'       "$out"
hasnt "and does not claim the credential is absent"          'has no credential named' "$out"

out=$("$ENGINE" run --capability not-stored -- https://e.example/ 2>&1)
has   "a service that answered and holds no such name says THAT" 'has no credential named' "$out"
hasnt "and does not blame the service"                       'could not be asked' "$out"

reset_calls
out=$("$ENGINE" run --name cap-pin --capability intranet -- https://intranet.example/ 2>&1)
argv=$(session_argv)
has "a capability pins the capsule to the credential's host" 'pins this capsule to intranet.example' "$out"
if [ -n "$argv" ]; then ok "and the capsule actually started"
else bad "and the capsule actually started" "no session argv was recorded"; fi

out=$("$ENGINE" run --capability intranet --allow somewhere.else -- https://x/ 2>&1); rc=$?
has   "widening a pinned capsule at the call site is refused" 'is pinned to' "$out"
exits "and that is a REFUSAL"                                 2 "$rc"

echo "── teardown, and the fences on it ──────────────────────────────────────"

reset_calls
"$ENGINE" run --name cap-gone --allow e.example:443 -- https://e.example/ >/dev/null 2>&1
if [ -e "$ROOT/cap-gone" ]; then
    bad "the capsule directory does not survive the run" "$ROOT/cap-gone is still there"
else ok "the capsule directory does not survive the run"; fi
has "the session is killed"  'apex <agent> <kill> <7>' "$(cat "$CALLS")"
has "and then forgotten"     'apex <agent> <rm> <7>'   "$(cat "$CALLS")"

# The fence, exercised by calling the shipped function with a name that
# resolves outside the root. A `rm -rf` driven by a name is the most dangerous
# line in the engine, so "it refuses" is asserted rather than reviewed.
mkdir -p "$WORK/elsewhere"
printf 'DO NOT DELETE\n' > "$WORK/elsewhere/keep.txt"
ln -sfn "$WORK/elsewhere" "$ROOT/cap-link"
out=$( ( set +u
         APEX_BROWSER_ROOT="$ROOT"
         # shellcheck disable=SC1090
         source "$ENGINE" >/dev/null 2>&1
         remove_capsule_dir cap-link ) 2>&1 ); rc=$?
has   "a capsule directory that is a symlink is refused" 'symlink' "$out"
exits "and the refusal exits 2"                          2 "$rc"
if [ -f "$WORK/elsewhere/keep.txt" ]; then ok "and the thing it pointed at is still there"
else bad "and the thing it pointed at is still there" "keep.txt was removed"; fi
rm -f "$ROOT/cap-link"

out=$( ( set +u
         APEX_BROWSER_ROOT="$ROOT"
         # shellcheck disable=SC1090
         source "$ENGINE" >/dev/null 2>&1
         remove_capsule_dir '../elsewhere' ) 2>&1 ); rc=$?
has   "a traversing capsule name is refused"  'not a valid capsule name' "$out"
exits "and that refusal exits 2 as well"      2 "$rc"
if [ -f "$WORK/elsewhere/keep.txt" ]; then ok "and again nothing was removed"
else bad "and again nothing was removed" "keep.txt was removed"; fi

echo "── help and doctor ─────────────────────────────────────────────────────"

# STDOUT. `apex vm --help` wrote to stderr once, which made `--help | grep`
# read an empty stream and a build assertion pass against nothing.
out=$("$ENGINE" --help 2>/dev/null)
has "--help writes to stdout"                'apex browser' "$out"
has "and says the capsule is headless"       'headless'     "$out"
hasnt "and offers no flag for a window"      '--headed'     "$out"

out=$(APEX_BROWSER_BIN=/nonexistent/browser "$ENGINE" doctor 2>&1); rc=$?
has   "doctor names a missing browser"       'MISSING'      "$out"
exits "and fails rather than reporting fine" 1 "$rc"
# "Permission denied is not absence": a browser that is there and cannot be
# executed is a different answer from no browser at all.
printf 'x\n' > "$WORK/unreadable"; chmod 000 "$WORK/unreadable"
out=$(APEX_BROWSER_BIN="$WORK/unreadable" "$ENGINE" doctor 2>&1)
has "an unexecutable browser is not reported as a missing one" 'PRESENT BUT NOT EXECUTABLE' "$out"
chmod 644 "$WORK/unreadable"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
