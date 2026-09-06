#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Packaging assertions for the protected secret service (roadmap §11).
#
#  The Rust suite tests the daemon. It cannot see the SHIPPED unit, and the gap
#  between the two is a real failure mode: `paths.rs` hard-codes
#  /var/lib/apex-secretd and /run/apex-secretd, systemd creates both from
#  StateDirectory= and RuntimeDirectory=, and nothing in the compiler notices
#  when one of those is renamed. A daemon that silently creates its own store
#  because systemd made a different one works — until ProtectSystem= is
#  tightened, at which point it stops, in production, holding credentials.
#
#  So this asserts the unit and the code agree, and that the security
#  properties AGENTS.md records are present in the file that actually ships.
#
#  NO NETWORK. NO ROOT. Nothing is started; every check is a file read.
#
#      ./tests/test-secret-service.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e` is deliberate, like the other suites here: this COUNTS failures
# instead of aborting on the first, and GitHub Actions invokes a script as
# `bash -e {0}`, under which a failing command inside a `$(...)` assignment
# kills the whole run part-way through and reports every remaining assertion as
# a failure.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT="$ROOT/files/system/units/apex-secretd.service"
SYSUSERS="$ROOT/files/system/sysusers/apex-secretd.conf"
PATHS="$ROOT/apexd/apex-secret-core/src/paths.rs"
VALUE="$ROOT/apexd/apex-secret-core/src/value.rs"
PROTOCOL="$ROOT/apexd/apex-secret-core/src/protocol.rs"
CONTAINERFILE="$ROOT/Containerfile.base"

pass=0; fail=0

ok()  { printf '  ok    %s\n' "$1"; pass=$((pass+1)); }
bad() { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }

# The command comes after the message, and is run here rather than reported
# through `$?`. Deliberate: `test x; want $?` reads the exit status of whatever
# ran most recently, which is the right command only until somebody inserts a
# line between the two.
want()    { local msg="$1"; shift; if "$@"; then ok "$msg"; else bad "$msg"; fi; }
wantnot() { local msg="$1"; shift; if "$@"; then bad "$msg"; else ok "$msg"; fi; }

# A line matching an extended regular expression, anywhere in a file.
has() { grep -qE "$2" "$1"; }

echo "── the unit exists and starts the right thing ──────────────────────────"
want "the unit file is present and not empty" test -s "$UNIT"
want "the sysusers entry is present" test -s "$SYSUSERS"
want "ExecStart is /usr/bin/apex-secretd" has "$UNIT" '^ExecStart=/usr/bin/apex-secretd$'

echo
echo "── it is not root, and holds nothing ───────────────────────────────────"
# The whole point of a separate daemon is a uid nobody can become. Running it
# as root would give it everything it exists to be separate from.
want "runs as apex-secret, not root" has "$UNIT" '^User=apex-secret$'
want "group is apex-secret" has "$UNIT" '^Group=apex-secret$'
want "sysusers creates the apex-secret account" has "$SYSUSERS" '^u apex-secret '
want "the account has no login shell" has "$SYSUSERS" 'nologin'
want "it holds no capabilities" has "$UNIT" '^CapabilityBoundingSet=$'
want "it cannot gain privilege" has "$UNIT" '^NoNewPrivileges=yes$'

echo
echo "── it cannot read a home directory ─────────────────────────────────────"
# THE invariant behind §11's first acceptance criterion. A credential store
# that can read $HOME is one an agent's own files can reach into, and this is
# what makes "outside agent-readable home paths" a property of the process
# rather than a claim about a path.
want "ProtectHome=yes" has "$UNIT" '^ProtectHome=yes$'
want "ProtectSystem=strict" has "$UNIT" '^ProtectSystem=strict$'
want "PrivateTmp=yes" has "$UNIT" '^PrivateTmp=yes$'

echo
echo "── the unit and the code agree about where things live ─────────────────"
# systemd's StateDirectory=X means /var/lib/X and RuntimeDirectory=X means
# /run/X. Both roots are hard-coded on the Rust side; renaming one alone is
# invisible to the compiler and to every Rust test.
state_name="$(sed -n 's/^StateDirectory=\(.*\)$/\1/p' "$UNIT")"
runtime_name="$(sed -n 's/^RuntimeDirectory=\(.*\)$/\1/p' "$UNIT")"
want "StateDirectory=apex-secretd (got '${state_name:-none}')" \
    test "$state_name" = "apex-secretd"
want "RuntimeDirectory=apex-secretd (got '${runtime_name:-none}')" \
    test "$runtime_name" = "apex-secretd"
want "the code's state root matches StateDirectory=" \
    has "$PATHS" "DEFAULT_STATE_DIR: &str = \"/var/lib/${state_name}\""
want "the code's runtime root matches RuntimeDirectory=" \
    has "$PATHS" "DEFAULT_RUNTIME_DIR: &str = \"/run/${runtime_name}\""
want "the store is created 0700" has "$UNIT" '^StateDirectoryMode=0700$'

echo
echo "── no privileged surface, like every other agent-adjacent daemon ───────"
# AGENTS.md forbids all three. This one being a SYSTEM unit makes the check
# matter more than it does for apex-agentd, not less.
wantnot "no polkit action" \
    test -e "$ROOT/files/system/polkit-1/actions/org.apexos.apex-secretd.policy"
wantnot "no system-bus name" \
    test -e "$ROOT/files/system/dbus-1/system.d/org.apexos.ApexSecretd1.conf"
wantnot "nothing in polkit mentions it" \
    grep -rq 'apex-secretd' "$ROOT/files/system/polkit-1"

echo
echo "── the image installs and enables it ───────────────────────────────────"
want "the builder installs the binary" \
    has "$CONTAINERFILE" 'install -Dm755 target/release/apex-secretd'
want "the unit is copied into the image" \
    has "$CONTAINERFILE" 'COPY files/system/units/apex-secretd\.service /usr/lib/systemd/system/'
want "the sysusers entry is copied into the image" \
    has "$CONTAINERFILE" 'COPY files/system/sysusers/apex-secretd\.conf /usr/lib/sysusers\.d/'
want "the service is enabled" \
    has "$CONTAINERFILE" '^RUN systemctl enable apex-secretd\.service$'

echo
echo "── there is no verb that returns a credential ──────────────────────────"
# The vocabulary, read out of the protocol rather than out of documentation.
# A verb added later that hands back a value would show up here.
for evil in Export Reveal Token Read Value Fetch; do
    wantnot "'$evil' is not a request verb" has "$PROTOCOL" "^    ${evil}[ ({]"
done
# And the type that holds a value must stay unserialisable. That is what makes
# a value-bearing response fail to COMPILE rather than fail review, since
# `Response` derives Serialize and a field must too.
derives_serialize() { grep -B4 '^pub struct SecretValue' "$1" | grep -q 'Serialize'; }
wantnot "SecretValue does not derive Serialize" derives_serialize "$VALUE"

echo
if [ "$fail" -eq 0 ]; then
    echo "secret service: $pass assertions passed"
    exit 0
fi
echo "secret service: $fail of $((pass+fail)) assertions FAILED"
exit 1
