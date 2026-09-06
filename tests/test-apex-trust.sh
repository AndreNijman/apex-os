#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-trust.sh — executable assertions for roadmap §27's trust readout.
#
#  Two modes, the same split `test-boot-v2.sh` uses and for the same reason:
#
#    (no argument)     Structural checks that need no toolchain — that the
#                      trust block is actually wired into `apex status`, that
#                      the module has no `.ok()`-shaped reader that could turn
#                      a refused read into a fact, and that the identity the
#                      report expects is the identity the workflow signs under.
#                      Wired into pr-validation's `static` job, which has no
#                      path filter.
#
#    --with-binary     Drives the built `apex` against fixture roots. It DIES
#                      if the binary is absent rather than skipping: a skipped
#                      check counts as a success, which is the failure this
#                      repository has now recorded three times.
#
#  ── What this is guarding ───────────────────────────────────────────────────
#  Every APEX image is cosign-signed and SBOM-attested in CI, and no installed
#  machine checks any of it: `/etc/containers/policy.json` ships one
#  `insecureAcceptAnything` default, and the booted deployment's origin says
#  `ostree-unverified-registry:` in its own words. That is the state this
#  command exists to make visible, so the assertions below are mostly about the
#  three ways a trust readout can lie: calling a refused read "unverified",
#  calling an unreachable registry "unsigned", and calling a certificate's own
#  claim about itself "verified".
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WITH_BINARY=0
[[ "${1:-}" == "--with-binary" ]] && WITH_BINARY=1

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
has() { # has <needle> <haystack-file> <label>
    if grep -qF -- "$1" "$2"; then ok "$3"; else
        bad "$3 — no '$1' in:"; sed 's/^/       /' "$2" >&2
    fi
}
hasnt() {
    if grep -qF -- "$1" "$2"; then
        bad "$3 — found '$1' in:"; sed 's/^/       /' "$2" >&2
    else ok "$3"; fi
}

TRUSTRS="$REPO/apexd/apex/src/trust.rs"
MAINRS="$REPO/apexd/apex/src/main.rs"
WORKFLOW="$REPO/.github/workflows/build-image.yml"
for f in "$TRUSTRS" "$MAINRS" "$WORKFLOW"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done

TMP="$(mktemp -d)"
trap 'chmod -R u+rwX "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

# ═════════════════════════════════════════════════════════════════════════════
sec "the trust block is wired into apex status, not merely available"
# §27's second criterion is "`apex status` surfaces trust". A module nobody
# calls satisfies a grep and not a user, and `cmd_status` is where the wiring
# can be lost in a refactor without any test noticing.
if grep -q 'trust::render_block' "$MAINRS"; then
    ok "main.rs calls trust::render_block"
else
    bad "main.rs never calls trust::render_block — apex status shows no trust state"
fi
# Placement matters as much as presence: `cmd_status` returns early when apexd
# is not running, so a trust block added after that branch would only appear on
# machines whose daemon is healthy.
status_body="$(sed -n '/^async fn cmd_status/,/^}/p' "$MAINRS")"
call_line="$(printf '%s\n' "$status_body" | grep -n 'trust::render_block' | head -1 | cut -d: -f1)"
early_return="$(printf '%s\n' "$status_body" | grep -n 'return 0;' | head -1 | cut -d: -f1)"
if [[ -n "$call_line" && -n "$early_return" && "$call_line" -lt "$early_return" ]]; then
    ok "the trust block is printed before cmd_status's early return"
else
    bad "the trust block comes after cmd_status's early return (line $call_line vs $early_return) — a machine with no apexd would see no trust state"
fi

sec "the expected signer is the identity CI actually signs under"
# These two drifting apart is silent: `apex trust --verify` would report
# "claimed X, expected Y" on a perfectly good image, and the first person to
# see it would reasonably conclude the image was tampered with.
if grep -q 'IDENTITY="https://github.com/.*/.github/workflows/build-image.yml@' "$WORKFLOW"; then
    ok "build-image.yml signs under <repository>/.github/workflows/build-image.yml@<ref>"
else
    bad "build-image.yml's cosign identity is not the shape trust.rs expects"
fi
if grep -q 'AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/main' "$TRUSTRS"; then
    ok "trust.rs expects that identity for this repository"
else
    bad "trust.rs's EXPECTED_SIGNER does not name build-image.yml on refs/heads/main"
fi

sec "no reader in trust.rs can turn a refused read into a fact"
# The EACCES class this repository swept in September: four readers collapsed
# "permission denied" into "absent" and then reported the guess as a checked
# fact. A trust readout is the worst place for it, so the module has one file
# reader and it returns Result. This catches a future `.ok()` being added back.
if grep -nE 'read_to_string\([^)]*\)\s*\.ok\(\)|fs::read\([^)]*\)\s*\.ok\(\)' "$TRUSTRS"; then
    bad "trust.rs discards the reason a read failed"
else
    ok "trust.rs has no read that drops its error"
fi
# `Path::exists` answers false for EACCES too. The one use is the cosign probe,
# where a false negative downgrades the report to "not checked" — the safe
# direction — and nothing else may use it.
exists_uses="$(grep -c 'Path::new(.*)\.exists()' "$TRUSTRS")"
if [[ "$exists_uses" -le 1 ]]; then
    ok "at most one Path::exists in trust.rs (the cosign probe, which fails safe)"
else
    bad "trust.rs has $exists_uses Path::exists calls; each one reads EACCES as absence"
fi

# ═════════════════════════════════════════════════════════════════════════════
if [[ "$WITH_BINARY" -eq 0 ]]; then
    printf '\n%s\n' "── binary checks skipped (pass --with-binary) ──"
    printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
    [[ "$FAIL" -eq 0 ]] || exit 1
    exit 0
fi

APEX="${APEX:-$REPO/apexd/target/debug/apex}"
[[ -x "$APEX" ]] || APEX="${CARGO_TARGET_DIR:-}/debug/apex"
if [[ ! -x "$APEX" ]]; then
    echo "FATAL: no apex binary. Build it, or set APEX=/path/to/apex." >&2
    echo "       A skipped assertion reports as a pass, which is the bug this refuses." >&2
    exit 1
fi

# Build one fixture root. $1 = name, $2 = the origin's
# container-image-reference (empty for none), $3 = policy.json body.
fixture() {
    local root="$TMP/$1" ref="$2" policy="$3"
    local csum=f3f505fc39fb268c59f4458365c96b764a7bd7d30f2f51e98bb6a009666b7852
    local bootcsum=1d98b51dd76621b656c50e4f22dc7e5eade9b0f869443a3efa90eee08eb9373e
    mkdir -p "$root/proc" "$root/etc/containers" \
             "$root/ostree/boot.0/default/$bootcsum" \
             "$root/ostree/deploy/default/deploy"
    printf 'root=UUID=x rw quiet ostree=/ostree/boot.0/default/%s/0\n' "$bootcsum" \
        > "$root/proc/cmdline"
    # The real layout: the ostree= argument is a symlink whose target is
    # relative to its own directory. Reproducing that here is the point — a
    # fixture that used an absolute path would not exercise the resolver.
    ln -sfn "../../../deploy/default/deploy/$csum.0" \
        "$root/ostree/boot.0/default/$bootcsum/0"
    mkdir -p "$root/ostree/deploy/default/deploy/$csum.0"
    if [[ -n "$ref" ]]; then
        printf '[origin]\ncontainer-image-reference=%s\n' "$ref" \
            > "$root/ostree/deploy/default/deploy/$csum.0.origin"
    else
        printf '[origin]\n' > "$root/ostree/deploy/default/deploy/$csum.0.origin"
    fi
    printf '%s\n' "$policy" > "$root/etc/containers/policy.json"
    printf '%s' "$root"
}

PERMISSIVE='{"default":[{"type":"insecureAcceptAnything"}],"transports":{"docker-daemon":{"":[{"type":"insecureAcceptAnything"}]}}}'
STRICT='{"default":[{"type":"reject"}],"transports":{"docker":{"ghcr.io/andrenijman/apex-os":[{"type":"sigstoreSigned","keyPath":"/etc/pki/apex.pub"}]}}}'

run() { APEX_TRUST_ROOT="$1" "$APEX" "${@:2}" > "$TMP/out" 2> "$TMP/err"; echo $?; }

sec "the state every APEX machine is actually in, reported in plain words"
# This fixture is byte-for-byte what the L16 has: an unverified pull against a
# policy that accepts anything.
R="$(fixture live 'ostree-unverified-registry:ghcr.io/andrenijman/apex-os:daily' "$PERMISSIVE")"
rc="$(run "$R" trust)"
[[ "$rc" == 0 ]] && ok "apex trust exits 0 on an unverified image" \
    || bad "apex trust exited $rc on the normal state; every script running it would fail"
has 'ghcr.io/andrenijman/apex-os:daily' "$TMP/out" "the image reference is printed without its ostree scheme"
has 'no signature was checked' "$TMP/out" "an unverified pull says no signature was checked"
has 'accepts any image, signed or not' "$TMP/out" "the policy is described as accepting anything"
has 'not contacted' "$TMP/out" "the registry line says it was not contacted"
hasnt 'unsigned' "$TMP/out" "the offline report never uses the word unsigned, which is a registry answer"

sec "a signed pull and a strict policy read differently"
R="$(fixture signed 'ostree-image-signed:registry:ghcr.io/andrenijman/apex-os:apex' "$STRICT")"
run "$R" trust >/dev/null
has 'the signature policy was applied' "$TMP/out" "an ostree-image-signed origin reports the policy was applied"
has 'requires a signature (sigstoreSigned)' "$TMP/out" "a sigstoreSigned scope is reported as requiring a signature"
# The strict fixture's DEFAULT is `reject`; the narrower docker scope is what
# applies. Getting this backwards would tell the user their next update will be
# refused when it will not.
hasnt 'rejects this image' "$TMP/out" "the narrower docker scope beats the default"

sec "a refused read is never a measurement"
# This is the whole EACCES lesson, at the binary. A /proc/cmdline nobody could
# read must not report as an image nobody verified: one is a fact about the
# machine, the other is a fact about the reader.
R="$(fixture unread 'ostree-unverified-registry:ghcr.io/x/y:z' "$PERMISSIVE")"
chmod 0000 "$R/proc/cmdline"
if [[ -r "$R/proc/cmdline" ]]; then
    printf '  skip  this user reads a 0000 file (root or CAP_DAC_OVERRIDE)\n'
else
    run "$R" trust --json >/dev/null
    has '"state": "unavailable"' "$TMP/out" "an unreadable cmdline is unavailable"
    has 'Permission denied' "$TMP/out" "the refusal reason is carried into the report"
    hasnt '"state": "unverified"' "$TMP/out" "it is NOT reported as an unverified pull"
fi
chmod 0644 "$R/proc/cmdline"

# The same, one file over: a policy that could not be read must not report as a
# policy that permits everything, which is the reading that would make an
# operator relax.
R="$(fixture nopolicy 'ostree-unverified-registry:ghcr.io/x/y:z' "$PERMISSIVE")"
rm -f "$R/etc/containers/policy.json"
run "$R" trust --json >/dev/null
if grep -q '"nextPullPolicy"' "$TMP/out" && \
   python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d["nextPullPolicy"]["state"]=="unavailable" else 1)' "$TMP/out"; then
    ok "a missing policy.json is unavailable, not accepts-anything"
else
    bad "a missing policy.json did not report as unavailable"
fi

sec "a deployment that is not a container deployment says so"
R="$(fixture bare '' "$PERMISSIVE")"
run "$R" trust >/dev/null
has 'not deployed from a container image' "$TMP/out" "an origin with no image reference is named as such"

sec "--verify never reaches the network under a fixture root"
# A suite that contacted ghcr.io would be measuring GitHub's availability, and
# would go red for reasons that have nothing to do with this code.
R="$(fixture verify 'ostree-unverified-registry:ghcr.io/x/y:z' "$PERMISSIVE")"
start=$(date +%s)
run "$R" trust --verify >/dev/null
elapsed=$(( $(date +%s) - start ))
has 'does not run under a fixture root' "$TMP/out" "--verify refuses to run against a fixture"
[[ "$elapsed" -lt 5 ]] && ok "--verify returned in ${elapsed}s, so nothing was dialled" \
    || bad "--verify took ${elapsed}s under a fixture root; something reached the network"

sec "apex status carries the trust block"
R="$(fixture instatus 'ostree-unverified-registry:ghcr.io/andrenijman/apex-os:daily' "$PERMISSIVE")"
APEX_TRUST_ROOT="$R" "$APEX" status > "$TMP/out" 2>/dev/null
has 'Trust' "$TMP/out" "apex status prints a Trust section"
has 'no signature was checked' "$TMP/out" "apex status carries the pull verification state"
has 'not contacted' "$TMP/out" "apex status says the registry was not contacted"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
