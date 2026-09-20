#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  The Android signing gates, exercised rather than described.
#
#  ONE MISTAKE IS BEING GUARDED AGAINST, and it has two halves:
#
#    * publishing an APK signed by NO key — which nobody can install; and
#    * publishing an APK signed by the WRONG key — which one person installs,
#      and is then stuck with forever. Android accepts an update only from the
#      key that signed what is already there, so a phone that installs a build
#      signed by a throwaway key can never move to a real one. Its owner's only
#      escape is uninstalling, which destroys this app's paired device key.
#
#  There is no server-side fix for either, so the refusals below are the
#  product. The happy paths are here to prove the refusals are not firing for
#  everything.
#
#  NOTHING HERE IS STUBBED WHERE IT COULD BE REAL. It generates a real keystore
#  with keytool, links a real 700-byte APK with aapt2, signs it with apksigner
#  and reads the signature back — a fake signer on PATH would only prove the
#  fake agrees with itself. It creates NO repository secret, cuts no release,
#  and every scrap of key material it makes is shredded on the way out.
#
#      ./tests/test-android-signing.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Counted, not aborted: nearly every assertion runs something that is SUPPOSED
# to exit non-zero, and under `-e` the first refusal would end the suite and
# report every later assertion as a failure. GitHub Actions invokes a script as
# `bash -e {0}`, so this matters here and not only at a prompt.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# NOT /tmp, and not by preference: on the machine this was written on /tmp is a
# 15 GB tmpfs carved out of 29 GB of RAM, and `generate-signing-key.sh` refuses
# to write a signing key onto a tmpfs — correctly, since it is RAM and is gone
# at the next reboot. A suite whose work directory is /tmp would exercise that
# refusal instead of the thing it means to test.
WORK="$(mktemp -d "${TMPDIR:-/var/tmp}/apex-android-signing.XXXXXX")"
# Key material, even throwaway key material, is shredded rather than unlinked —
# and the suite says so in its own output at the end, so "it was cleaned up" is
# a fact a reader can check rather than a claim.
cleanup() {
    find "$WORK" -type f \( -name '*.jks' -o -name '*.p12' -o -name 'password*' \) \
        -exec shred -u {} + 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

# `said <text> <needle>` — substring match without a pipeline, because
# `printf … | grep -q X` returns 141 on a match under `pipefail` (SIGPIPE on
# the writer) and this repository has been mis-seeded by exactly that before.
said() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

REQUIRE="$ROOT/android/tools/require-signing-secrets.sh"
VERIFY="$ROOT/android/tools/verify-signing-identity.sh"
GENERATE="$ROOT/android/tools/generate-signing-key.sh"
ARTIFACTS="$ROOT/android/tools/release-artifacts.sh"
PIN="$ROOT/android/signing-certificate.sha256"
WORKFLOW="$ROOT/.github/workflows/release-android.yml"
PRVAL="$ROOT/.github/workflows/pr-validation.yml"
DOC="$ROOT/docs/android-signing.md"

for f in "$REQUIRE" "$VERIFY" "$GENERATE" "$ARTIFACTS" "$PIN" "$WORKFLOW" "$PRVAL" "$DOC"; do
    [ -f "$f" ] || { echo "FATAL: $f is missing; the thing this suite tests does not exist" >&2; exit 2; }
done
for f in "$REQUIRE" "$VERIFY" "$GENERATE"; do
    [ -x "$f" ] || { echo "FATAL: $f is not executable, so CI could not run it" >&2; exit 2; }
done

# A missing prerequisite is a FAILURE here, never a skip: a suite that prints
# "0 passed, 0 failed" and exits 0 arrives at the aggregate gate as a success,
# and no gate can see the difference.
for tool in python3 keytool shred openssl; do
    command -v "$tool" >/dev/null 2>&1 \
        || { echo "FATAL: $tool is required; this suite cannot check a signature without it" >&2; exit 2; }
done
python3 -c 'import yaml' 2>/dev/null \
    || { echo "FATAL: python3 needs PyYAML to read the workflow; without it the workflow assertions would inspect nothing" >&2; exit 2; }

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] || { echo "FATAL: ANDROID_HOME/ANDROID_SDK_ROOT is unset, so apksigner and aapt2 cannot be found" >&2; exit 2; }
APKSIGNER=""; AAPT2=""
for d in $(ls -1 "$SDK/build-tools" 2>/dev/null | sort -V); do
    [ -x "$SDK/build-tools/$d/apksigner" ] && APKSIGNER="$SDK/build-tools/$d/apksigner"
    [ -x "$SDK/build-tools/$d/aapt2" ]     && AAPT2="$SDK/build-tools/$d/aapt2"
done
[ -n "$APKSIGNER" ] || { echo "FATAL: no apksigner under $SDK/build-tools" >&2; exit 2; }
[ -n "$AAPT2" ]     || { echo "FATAL: no aapt2 under $SDK/build-tools" >&2; exit 2; }
PLATFORM=""
for d in $(ls -1 "$SDK/platforms" 2>/dev/null | sort -V); do
    [ -f "$SDK/platforms/$d/android.jar" ] && PLATFORM="$SDK/platforms/$d/android.jar"
done
[ -n "$PLATFORM" ] || { echo "FATAL: no platforms/*/android.jar under $SDK" >&2; exit 2; }

# ─────────────────────────────────────────────────────────────────────────────
section "the release refuses when there is no key, and says which secret"
# ─────────────────────────────────────────────────────────────────────────────

ALL_SET=(APEX_KEYSTORE_BASE64=a2V5 APEX_KEYSTORE_PASSWORD=Sup3rSecretValue
         APEX_KEY_ALIAS=apex-release APEX_KEY_PASSWORD=Sup3rSecretValue)

# `env -i`, so the environment under test is exactly what is listed and not
# whatever this shell happens to carry — and so the arguments are assignments
# only. GNU env stops reading options at the first NAME=VALUE, so a `-u` after
# one would be taken for the command to run.
run_require() { env -i PATH="$PATH" "$@" "$REQUIRE" 2>&1; }

out=$(run_require "${ALL_SET[@]}"); rc=$?
if [ "$rc" -eq 0 ] && said "$out" "all four are set"; then
    ok "four secrets present is not refused (the gate is not refusing everything)"
else
    bad "a fully configured repository was refused (rc=$rc): $out"
fi

# The value must never appear in the output — of the happy path or of any
# refusal. A secret echoed into a log is a secret.
if said "$out" "Sup3rSecretValue"; then
    bad "the password appeared in the output"
else
    ok "no secret value is printed"
fi

for missing in APEX_KEYSTORE_BASE64 APEX_KEYSTORE_PASSWORD APEX_KEY_ALIAS APEX_KEY_PASSWORD; do
    three=()
    for kv in "${ALL_SET[@]}"; do
        case "$kv" in "$missing"=*) continue ;; esac
        three+=("$kv")
    done
    out=$(run_require "${three[@]}"); rc=$?
    if [ "$rc" -ne 0 ] && said "$out" "$missing is not set"; then
        ok "$missing missing refuses the release, and names that variable"
    else
        bad "$missing missing was not refused, or was not named (rc=$rc): $out"
    fi
done

out=$(run_require); rc=$?
if [ "$rc" -ne 0 ]; then
    ok "a repository with no signing secrets at all refuses"
else
    bad "a repository with no signing secrets was allowed to proceed"
fi
# The message has to be actionable and must not offer a way round itself.
if said "$out" "docs/android-signing.md"; then
    ok "the refusal points at the document that says who holds the key"
else
    bad "the refusal does not say where to read about the key: $out"
fi
if said "$out" "UNSIGNED"; then
    ok "the refusal says what it is refusing to produce"
else
    bad "the refusal does not say why it matters: $out"
fi

# The classic `gh secret set NAME < file` mistake: the file's trailing newline
# becomes part of the password, and the keystore then fails to open with a
# password that looks right everywhere a human can see it.
out=$(run_require APEX_KEYSTORE_BASE64=a2V5 "APEX_KEYSTORE_PASSWORD=Sup3rSecretValue
" APEX_KEY_ALIAS=apex-release "APEX_KEY_PASSWORD=Sup3rSecretValue
"); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "whitespace"; then
    ok "a password with a trailing newline is refused before the build"
else
    bad "a trailing newline in the password was accepted (rc=$rc): $out"
fi

# Differing passwords are a NOTICE, not a gate: PKCS12 cannot hold two, but an
# old-style JKS can, and refusing one would be refusing on a guess.
out=$(run_require APEX_KEYSTORE_BASE64=a2V5 APEX_KEYSTORE_PASSWORD=Sup3rSecretValue \
      APEX_KEY_ALIAS=apex-release APEX_KEY_PASSWORD=Different123); rc=$?
if [ "$rc" -eq 0 ] && said "$out" "PKCS12"; then
    ok "differing store and key passwords warn rather than refuse"
else
    bad "the PKCS12 password notice is missing or has become a gate (rc=$rc): $out"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "the workflow cannot reach a build without passing that gate"
# ─────────────────────────────────────────────────────────────────────────────
#
# Static, and structural rather than textual: a `grep` for a script name would
# be satisfied by a comment mentioning it, and this repository has shipped a
# check that matched the comment explaining what it forbade.

python3 - "$WORKFLOW" > "$WORK/wf.txt" 2>&1 <<'PY'
import sys, yaml
doc = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
job = doc["jobs"]["release"]
steps = job["steps"]
print("STEPS=%d" % len(steps))
def body(s):
    return (s.get("run") or "") + " " + (s.get("uses") or "")
for i, s in enumerate(steps):
    name = s.get("name", "")
    r = body(s)
    if "require-signing-secrets.sh" in r:
        print("REQUIRE_AT=%d" % i)
        print("REQUIRE_IF=%s" % ("yes" if "if" in s else "no"))
        env = s.get("env") or {}
        for k in ("APEX_KEYSTORE_BASE64", "APEX_KEYSTORE_PASSWORD",
                  "APEX_KEY_ALIAS", "APEX_KEY_PASSWORD"):
            if k in env and "secrets." + k in str(env[k]):
                print("REQUIRE_ENV=%s" % k)
    if "verify-signing-identity.sh --published" in r:
        print("PUBLISHED_AT=%d" % i)
        print("PUBLISHED_IF=%s" % ("yes" if "if" in s else "no"))
    if "release-artifacts.sh" in r:
        print("BUILD_AT=%d" % i)
    if "base64 -d" in r:
        print("MATERIALISE_AT=%d" % i)
    # A `secrets.` reference inside a step-level `if:` silently evaluates to
    # false: the context is not available there. That is a gate that never runs.
    if "secrets." in str(s.get("if", "")):
        print("SECRET_IN_IF=%s" % name)
    # A run: block over 21000 characters surfaces as a failed run with no job
    # in it, which reads as infrastructure rather than as this file.
    if len(s.get("run") or "") > 21000:
        print("OVERSIZE=%s" % name)
PY
wf=$(cat "$WORK/wf.txt")

if said "$wf" "STEPS="; then
    n=$(sed -n 's/^STEPS=//p' "$WORK/wf.txt")
    if [ "${n:-0}" -ge 8 ]; then
        ok "release-android.yml parses as YAML and has $n steps to inspect"
    else
        bad "the workflow parsed but has only ${n:-0} steps; this section would inspect nothing"
    fi
else
    bad "release-android.yml did not parse: $wf"
fi

req_at=$(sed -n 's/^REQUIRE_AT=//p' "$WORK/wf.txt")
pub_at=$(sed -n 's/^PUBLISHED_AT=//p' "$WORK/wf.txt")
build_at=$(sed -n 's/^BUILD_AT=//p' "$WORK/wf.txt")
mat_at=$(sed -n 's/^MATERIALISE_AT=//p' "$WORK/wf.txt")

if [ -n "$req_at" ]; then
    ok "the workflow runs require-signing-secrets.sh"
else
    bad "nothing in release-android.yml runs the secrets gate, so a release could be attempted with no key"
fi
if [ "$(sed -n 's/^REQUIRE_IF=//p' "$WORK/wf.txt")" = "no" ]; then
    ok "the secrets gate carries no if:, so it cannot be skipped into a pass"
else
    bad "the secrets gate has an if: — a skipped step counts as success here"
fi
envs=$(sed -n 's/^REQUIRE_ENV=//p' "$WORK/wf.txt" | sort | tr '\n' ' ')
if said "$envs" "APEX_KEYSTORE_BASE64" && said "$envs" "APEX_KEYSTORE_PASSWORD" \
   && said "$envs" "APEX_KEY_ALIAS" && said "$envs" "APEX_KEY_PASSWORD"; then
    ok "all four secrets reach the gate through env:, where the secrets context works"
else
    bad "the gate cannot see every secret it checks; it has: $envs"
fi
if [ -n "$req_at" ] && [ -n "$mat_at" ] && [ "$req_at" -lt "$mat_at" ]; then
    ok "the gate runs before the keystore is materialised"
else
    bad "the keystore is written to the runner before the gate that refuses (gate=$req_at, materialise=$mat_at)"
fi
if [ -n "$req_at" ] && [ -n "$build_at" ] && [ "$req_at" -lt "$build_at" ]; then
    ok "the gate runs before the build"
else
    bad "the build can start before the signing gate (gate=$req_at, build=$build_at)"
fi
if [ -n "$pub_at" ] && [ -n "$build_at" ] && [ "$pub_at" -lt "$build_at" ]; then
    ok "the published-certificate gate runs before the build"
else
    bad "nothing checks that a certificate is published before the build (published=$pub_at, build=$build_at)"
fi
if [ "$(sed -n 's/^PUBLISHED_IF=//p' "$WORK/wf.txt")" = "no" ]; then
    ok "the published-certificate gate carries no if: either"
else
    bad "the published-certificate gate has an if:"
fi
if said "$wf" "SECRET_IN_IF="; then
    bad "a step condition reads the secrets context, which is always false there: $(sed -n 's/^SECRET_IN_IF=//p' "$WORK/wf.txt")"
else
    ok "no step condition depends on the secrets context"
fi
if said "$wf" "OVERSIZE="; then
    bad "a run: block is over the 21000-character cap: $(sed -n 's/^OVERSIZE=//p' "$WORK/wf.txt")"
else
    ok "no run: block is near the 21000-character cap"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "a key nobody published is refused, and so is no published key at all"
# ─────────────────────────────────────────────────────────────────────────────

# Two real keystores. `-storepass:file`, never an argument, because an argument
# is visible in `ps` to every process on the machine.
KEYDIR="$WORK/keys"; mkdir -p "$KEYDIR"
printf '%s' 'throwaway-not-a-release-key' > "$KEYDIR/password"
make_key() {
    keytool -genkeypair -storetype PKCS12 -keystore "$KEYDIR/$1.jks" -alias "$1" \
        -keyalg RSA -keysize 2048 -validity 30 -dname "CN=THROWAWAY $1, O=test" \
        -storepass:file "$KEYDIR/password" >/dev/null 2>&1
}
fingerprint_of() {
    keytool -exportcert -keystore "$KEYDIR/$1.jks" -alias "$1" \
        -storepass:file "$KEYDIR/password" 2>/dev/null > "$WORK/$1.der" || return 1
    local s; s=$(sha256sum "$WORK/$1.der") || return 1
    printf '%s' "${s%% *}"
}
make_key alpha || { echo "FATAL: keytool could not generate a test key" >&2; exit 2; }
make_key beta  || { echo "FATAL: keytool could not generate a test key" >&2; exit 2; }
FP_A=$(fingerprint_of alpha); FP_B=$(fingerprint_of beta)
case "$FP_A" in
    [0-9a-f]*) : ;;
    *) echo "FATAL: could not read a fingerprint out of the test keystore" >&2; exit 2 ;;
esac
[ "$FP_A" != "$FP_B" ] || { echo "FATAL: two generated keys have the same fingerprint; the fixture is wrong" >&2; exit 2; }

pin_file() { printf '%s\n' "$2" > "$WORK/pin-$1"; printf '%s' "$WORK/pin-$1"; }

# The state the repository is in today.
p=$(pin_file unset "UNSET")
out=$("$VERIFY" --published --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "UNSET"; then
    ok "an UNSET fingerprint refuses the release rather than signing with anything"
else
    bad "UNSET did not refuse (rc=$rc): $out"
fi
if said "$out" "generate-signing-key.sh"; then
    ok "and it says how to make the key"
else
    bad "the UNSET refusal does not name the generator: $out"
fi

out=$("$VERIFY" --published --pin "$WORK/no-such-pin" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "does not exist"; then
    ok "a missing fingerprint file refuses; it is not read as 'nothing to check'"
else
    bad "a missing pin file did not refuse (rc=$rc): $out"
fi

p=$(pin_file junk "not-a-fingerprint")
out=$("$VERIFY" --published --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ]; then
    ok "a malformed fingerprint refuses"
else
    bad "a malformed fingerprint was accepted"
fi

p=$(pin_file alpha "$FP_A")
out=$("$VERIFY" --published --pin "$p" 2>&1); rc=$?
if [ "$rc" -eq 0 ] && said "$out" "$FP_A"; then
    ok "a published fingerprint is accepted and printed (the gate is not refusing everything)"
else
    bad "a valid fingerprint was refused (rc=$rc): $out"
fi

# keytool prints uppercase and colon-separated; apksigner prints bare
# lowercase. Both must mean the same thing, or a correct value pasted from the
# wrong tool reads as a wrong key.
upper=$(printf '%s' "$FP_A" | tr 'a-f' 'A-F' | sed 's/\(..\)/\1:/g; s/:$//')
p=$(pin_file upper "$upper")
out=$(APEX_KEYSTORE_PASSWORD=throwaway-not-a-release-key \
      "$VERIFY" --keystore "$KEYDIR/alpha.jks" --alias alpha --pin "$p" 2>&1); rc=$?
if [ "$rc" -eq 0 ]; then
    ok "the keytool spelling of a fingerprint is the same value as the apksigner one"
else
    bad "a colon-separated uppercase fingerprint was not recognised (rc=$rc): $out"
fi

section "the keystore is checked against the published certificate"

p=$(pin_file alpha "$FP_A")
out=$(APEX_KEYSTORE_PASSWORD=throwaway-not-a-release-key \
      "$VERIFY" --keystore "$KEYDIR/alpha.jks" --alias alpha --pin "$p" 2>&1); rc=$?
if [ "$rc" -eq 0 ] && said "$out" "verified"; then
    ok "the published key opens and matches"
else
    bad "the right keystore was refused (rc=$rc): $out"
fi

out=$(APEX_KEYSTORE_PASSWORD=throwaway-not-a-release-key \
      "$VERIFY" --keystore "$KEYDIR/beta.jks" --alias beta --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "NOT the published signing certificate"; then
    ok "a DIFFERENT key in the secret is refused before anything is built"
else
    bad "a keystore holding the wrong key was accepted (rc=$rc): $out"
fi

out=$(env -u APEX_KEYSTORE_PASSWORD "$VERIFY" --keystore "$KEYDIR/alpha.jks" --alias alpha --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "APEX_KEYSTORE_PASSWORD"; then
    ok "no password is a refusal, and the password is never an argument"
else
    bad "a missing password did not refuse (rc=$rc): $out"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "the APK that would be published is read back, not inferred"
# ─────────────────────────────────────────────────────────────────────────────

APKDIR="$WORK/apk"; mkdir -p "$APKDIR"
cat > "$APKDIR/AndroidManifest.xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="com.apexos.signing.fixture">
    <uses-sdk android:minSdkVersion="28" android:targetSdkVersion="36" />
    <application android:hasCode="false" />
</manifest>
XML
"$AAPT2" link -o "$APKDIR/plain.apk" --manifest "$APKDIR/AndroidManifest.xml" -I "$PLATFORM" \
    >"$WORK/aapt2.log" 2>&1 \
    || { echo "FATAL: aapt2 could not link the fixture APK; this section cannot check a signature" >&2
         cat "$WORK/aapt2.log" >&2; exit 2; }

sign_with() {  # sign_with <key> <out>
    "$APKSIGNER" sign --ks "$KEYDIR/$1.jks" --ks-key-alias "$1" \
        --ks-pass "file:$KEYDIR/password" --min-sdk-version 28 \
        --v1-signing-enabled false --v2-signing-enabled true --v3-signing-enabled true \
        --out "$2" "$APKDIR/plain.apk" >/dev/null 2>&1
}
sign_with alpha "$APKDIR/alpha.apk" || { echo "FATAL: apksigner could not sign the fixture APK" >&2; exit 2; }
sign_with beta  "$APKDIR/beta.apk"  || { echo "FATAL: apksigner could not sign the fixture APK" >&2; exit 2; }

p=$(pin_file alpha "$FP_A")
out=$("$VERIFY" --apk "$APKDIR/alpha.apk" --pin "$p" 2>&1); rc=$?
if [ "$rc" -eq 0 ] && said "$out" "$FP_A"; then
    ok "an APK signed by the published key is accepted"
else
    bad "an APK signed by the published key was refused (rc=$rc): $out"
fi

out=$("$VERIFY" --apk "$APKDIR/beta.apk" --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "$FP_B"; then
    ok "an APK signed by an unpublished key is refused, and the unexpected signer is named"
else
    bad "an APK signed by the wrong key was published-able (rc=$rc): $out"
fi

# The half-done rotation: the repository publishes two certificates and the
# build only used one. Every phone holding the other would refuse the update.
p=$(printf '%s\n%s\n' "$FP_A" "$FP_B" > "$WORK/pin-both"; printf '%s' "$WORK/pin-both")
out=$("$VERIFY" --apk "$APKDIR/alpha.apk" --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "and the APK is not signed by it"; then
    ok "a published certificate missing from the APK is refused (a half-finished rotation)"
else
    bad "an APK missing one of the published signers was accepted (rc=$rc): $out"
fi

p=$(pin_file alpha "$FP_A")
out=$("$VERIFY" --apk "$APKDIR/plain.apk" --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ]; then
    ok "an unsigned APK is refused rather than reported as having no problem"
else
    bad "an unsigned APK passed the signing-identity check"
fi

# A gate that cannot look must fail. This repository's dominant CI defect is a
# check that runs, inspects nothing, and reports success.
out=$(env -u ANDROID_HOME -u ANDROID_SDK_ROOT -u APKSIGNER \
      "$VERIFY" --apk "$APKDIR/alpha.apk" --pin "$p" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "apksigner"; then
    ok "with no apksigner it refuses instead of passing the APK unchecked"
else
    bad "a run that could not look reported no problem (rc=$rc): $out"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "release-artifacts.sh actually uses both checks, in the right order"
# ─────────────────────────────────────────────────────────────────────────────

# The call is written `"$here/verify-signing-identity.sh" --keystore`, so the
# pattern has to allow the closing quote: a grep for the bare
# `verify-signing-identity.sh --keystore` matches nothing and reports the check
# as missing — which is how this assertion first failed against a file that
# already had the call in it.
ks_line=$(grep -nE 'verify-signing-identity\.sh"? --keystore' "$ARTIFACTS" | head -1 | cut -d: -f1)
apk_line=$(grep -nE 'verify-signing-identity\.sh"? --apk' "$ARTIFACTS" | head -1 | cut -d: -f1)
gradle_line=$(grep -n '\./gradlew' "$ARTIFACTS" | head -1 | cut -d: -f1)
# The INVOCATION, not the first mention: `verifyReleaseSigning` appears in this
# script's header comment thirty lines before anything runs, and comparing
# against that would compare against prose.
verify_line=$(grep -n 'gradlew.*verifyReleaseSigning' "$ARTIFACTS" | head -1 | cut -d: -f1)
if [ -n "$ks_line" ] && [ -n "$gradle_line" ] && [ "$ks_line" -lt "$gradle_line" ]; then
    ok "the keystore is checked against the published certificate before Gradle runs"
else
    bad "the keystore identity check is missing or runs after the build (check=$ks_line, gradle=$gradle_line)"
fi
if [ -n "$apk_line" ] && [ -n "$verify_line" ] && [ "$apk_line" -gt "$verify_line" ]; then
    ok "the built APK's signer is read back after the signature is verified"
else
    bad "the APK identity check is missing or runs before the artefacts exist (check=$apk_line, verify=$verify_line)"
fi

# Behavioural, not only textual: a real keystore, a fixture repository whose
# published fingerprint is a DIFFERENT key, and the refusal must arrive before
# a single line of Gradle.
FIX="$WORK/fix"
mkdir -p "$FIX/android/tools" "$FIX/android/app" \
         "$FIX/android/core/src/main/kotlin/com/apexos/remote/core"
cp "$ARTIFACTS" "$VERIFY" "$FIX/android/tools/"
printf 'val APEX_MARKETING_VERSION = "0.1.0"\n' > "$FIX/android/app/build.gradle.kts"
{
    printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n'
    printf 'val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(1)\n'
} > "$FIX/android/core/src/main/kotlin/com/apexos/remote/core/Client.kt"

printf '%s\n' "$FP_B" > "$FIX/android/signing-certificate.sha256"
out=$(APEX_KEYSTORE="$KEYDIR/alpha.jks" APEX_KEYSTORE_PASSWORD=throwaway-not-a-release-key \
      APEX_KEY_ALIAS=alpha APEX_KEY_PASSWORD=throwaway-not-a-release-key \
      "$FIX/android/tools/release-artifacts.sh" --out "$WORK/relout" --code 5 \
      --name 0.1.0+5.gdeadbeef 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "NOT the published signing certificate"; then
    ok "a release built with an unpublished key is refused"
else
    bad "release-artifacts.sh accepted a key the repository does not publish (rc=$rc): $out"
fi
if said "$out" "gradlew" || said "$out" "Gradle"; then
    bad "the refusal arrived after the build started"
else
    ok "and the refusal arrives before the build, not ten minutes into it"
fi

# The other half, or the assertion above would pass for a script that refuses
# everything: with the RIGHT key the identity check passes and the run gets as
# far as the build it cannot do in a fixture.
printf '%s\n' "$FP_A" > "$FIX/android/signing-certificate.sha256"
out=$(APEX_KEYSTORE="$KEYDIR/alpha.jks" APEX_KEYSTORE_PASSWORD=throwaway-not-a-release-key \
      APEX_KEY_ALIAS=alpha APEX_KEY_PASSWORD=throwaway-not-a-release-key \
      "$FIX/android/tools/release-artifacts.sh" --out "$WORK/relout" --code 5 \
      --name 0.1.0+5.gdeadbeef 2>&1); rc=$?
if said "$out" "signing certificate verified: $FP_A"; then
    ok "the matching key passes the identity check and the run proceeds"
else
    bad "the identity check refused the key it publishes, so the check above proves nothing (rc=$rc): $out"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "the generator is a thing Andre runs, and it refuses to run anywhere else"
# ─────────────────────────────────────────────────────────────────────────────

out=$(GITHUB_ACTIONS=true "$GENERATE" --out "$WORK/ci-key" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "must not run in CI"; then
    ok "the generator refuses to run in Actions"
else
    bad "the generator would run in CI (rc=$rc): $out"
fi
[ -e "$WORK/ci-key/release.jks" ] && bad "the CI refusal still wrote a key" \
    || ok "and it wrote no key material on the way out"

out=$(env -u GITHUB_ACTIONS -u CI "$GENERATE" --out /tmp/apex-key-should-not-happen 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "tmpfs"; then
    ok "it refuses to put a signing key on a tmpfs, which is RAM and is gone at reboot"
else
    bad "a key would have been written to /tmp (rc=$rc): $out"
fi

mkdir -p "$WORK/inrepo" && git -C "$WORK/inrepo" init --quiet 2>/dev/null
out=$(env -u GITHUB_ACTIONS -u CI "$GENERATE" --out "$WORK/inrepo/key" 2>&1); rc=$?
if [ "$rc" -ne 0 ] && said "$out" "inside the git repository"; then
    ok "it refuses to write a key inside a checkout"
else
    bad "a signing key would have been written inside a git repository (rc=$rc): $out"
fi

# And then the real thing, end to end, because "Andre runs one command" is the
# deliverable and a command nobody has run is a guess.
#
# GITHUB_ACTIONS is unset deliberately for this one run: the guard above exists
# so that a PERSON does not generate the real key on a runner, and it has just
# been asserted. What happens here is a throwaway 4096-bit key in a temporary
# directory, shredded when this suite exits.
GEN_OUT="$WORK/generated"
GEN_REPO="$WORK/genrepo"
mkdir -p "$GEN_REPO/android" "$GEN_REPO/docs"
cp "$PIN" "$GEN_REPO/android/signing-certificate.sha256"
printf 'phone section\n<!-- fingerprint:begin -->\nUNSET\n<!-- fingerprint:end -->\nafter\n' > "$GEN_REPO/README.md"
printf 'doc\n<!-- fingerprint:begin -->\nUNSET\n<!-- fingerprint:end -->\nafter\n' > "$GEN_REPO/docs/android-signing.md"
out=$(env -u GITHUB_ACTIONS -u CI "$GENERATE" --out "$GEN_OUT" --repo "$GEN_REPO" --alias apex-release 2>&1); rc=$?
if [ "$rc" -eq 0 ]; then
    ok "the command Andre runs completes"
else
    bad "the generator failed (rc=$rc): $out"
fi
for f in release.jks password.txt keystore.base64 BACKUP-README.txt; do
    if [ -s "$GEN_OUT/$f" ]; then ok "it wrote $f"; else bad "it did not write $f"; fi
done
perm=$(stat -c%a "$GEN_OUT/release.jks" 2>/dev/null)
if [ "$perm" = "600" ]; then ok "the keystore is 0600"; else bad "the keystore is $perm, not 0600"; fi
perm=$(stat -c%a "$GEN_OUT/password.txt" 2>/dev/null)
if [ "$perm" = "600" ]; then ok "the password file is 0600"; else bad "the password file is $perm, not 0600"; fi

# The password must not be in the output. This one is not hypothetical: the
# terminal it prints to is scrolled back, logged and, here, recorded in an
# agent transcript.
genpw=$(cat "$GEN_OUT/password.txt")
if [ -n "$genpw" ] && said "$out" "$genpw"; then
    bad "the generated password was printed to the terminal"
else
    ok "the password is never printed, only written to a 0600 file"
fi
case "$genpw" in
    *[!0-9a-f]* | "") bad "the generated password is not the 48 hex characters it should be" ;;
    *) [ "${#genpw}" -eq 48 ] && ok "the password is 96 bits of randomness" \
                              || bad "the password is ${#genpw} characters" ;;
esac
# A trailing newline here becomes part of the GitHub secret and makes a correct
# password wrong.
if [ "$(wc -c < "$GEN_OUT/password.txt")" = "48" ]; then
    ok "the password file has no trailing newline to be pasted into a secret"
else
    bad "the password file is $(wc -c < "$GEN_OUT/password.txt") bytes; a trailing newline would silently break signing"
fi

if said "$out" "cannot be read back" || said "$out" "CANNOT BE READ BACK"; then
    ok "it says that a GitHub secret cannot be read back, which is why GitHub is not the backup"
else
    bad "the generator does not say that a GitHub secret cannot be read back: $out"
fi
if said "$out" "gh secret set APEX_KEYSTORE_BASE64"; then
    ok "it prints the exact commands that set the secrets"
else
    bad "it does not print the gh secret set commands"
fi
# Read from a file, never `--body`: a password on a command line is a password
# in shell history.
if said "$out" "--body"; then
    bad "it tells Andre to put a password on a command line"
else
    ok "the secrets are set from files, so no password reaches shell history"
fi

# What it published, and that the three copies agree.
gen_fp=$(grep -v '^#' "$GEN_REPO/android/signing-certificate.sha256" | tr -d '[:space:]')
case "$gen_fp" in
    [0-9a-f]*) [ "${#gen_fp}" -eq 64 ] && ok "it published a real fingerprint" \
                                       || bad "the published fingerprint is ${#gen_fp} characters" ;;
    *) bad "it did not publish a fingerprint: '$gen_fp'" ;;
esac
if said "$out" "$gen_fp"; then
    ok "and printed it, so it can be compared with a downloaded APK"
else
    bad "the fingerprint it published was not printed"
fi
for doc in "$GEN_REPO/README.md" "$GEN_REPO/docs/android-signing.md"; do
    if grep -q "$gen_fp" "$doc"; then
        ok "the fingerprint reached ${doc#"$GEN_REPO"/}"
    else
        bad "${doc#"$GEN_REPO"/} still carries the old value; two published fingerprints would disagree"
    fi
done
# Non-vacuous: the markers were really rewritten, not merely present.
if grep -q 'UNSET' "$GEN_REPO/README.md"; then
    bad "README.md still says UNSET after the key was generated"
else
    ok "no stale UNSET is left behind"
fi

# The generated keystore is a real one: the same checks the release runs must
# accept it end to end. This is the whole path Andre is being handed.
p=$(pin_file gen "$gen_fp")
out=$(APEX_KEYSTORE_PASSWORD="$genpw" "$VERIFY" --keystore "$GEN_OUT/release.jks" \
      --alias apex-release --pin "$p" 2>&1); rc=$?
if [ "$rc" -eq 0 ]; then
    ok "the key it generated satisfies the gate the release runs"
else
    bad "the generated key does not match the fingerprint it published (rc=$rc): $out"
fi
b64=$(cat "$GEN_OUT/keystore.base64")
printf '%s' "$b64" | base64 -d > "$WORK/decoded.jks" 2>/dev/null
if cmp -s "$WORK/decoded.jks" "$GEN_OUT/release.jks"; then
    ok "keystore.base64 decodes to exactly the keystore, so the secret will too"
else
    bad "keystore.base64 does not decode back to the keystore"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "what the repository publishes, and what the docs say, are one value"
# ─────────────────────────────────────────────────────────────────────────────

repo_fp=$(grep -v '^#' "$PIN" | tr -d '[:space:]')
[ -n "$repo_fp" ] || bad "android/signing-certificate.sha256 holds nothing at all"
marker_files=0
for doc in "$ROOT/README.md" "$ROOT"/docs/*.md; do
    grep -q 'fingerprint:begin' "$doc" 2>/dev/null || continue
    marker_files=$((marker_files + 1))
    val=$(sed -n '/fingerprint:begin/,/fingerprint:end/p' "$doc" | sed '1d;$d' | tr -d '[:space:]')
    if [ "$val" = "$repo_fp" ]; then
        ok "${doc#"$ROOT"/} publishes the same value as android/signing-certificate.sha256"
    else
        bad "${doc#"$ROOT"/} says '$val' and the repository publishes '$repo_fp'"
    fi
done
if [ "$marker_files" -ge 2 ]; then
    ok "the fingerprint is published in $marker_files documents, README included"
else
    bad "only $marker_files document publishes the fingerprint; the loop above checked almost nothing"
fi

# The documented decision, and the sentence that is the reason for it.
for phrase in "cannot be read back" "apksigner rotate" "generate-signing-key.sh"; do
    if grep -qF "$phrase" "$DOC"; then
        ok "docs/android-signing.md covers: $phrase"
    else
        bad "docs/android-signing.md never mentions $phrase"
    fi
done

section "pr-validation never holds the signing key"

if grep -q 'secrets.APEX_KEYSTORE\|secrets.APEX_KEY_' "$PRVAL"; then
    bad "pr-validation.yml still materialises the signing key on every PR runner"
else
    ok "pr-validation.yml references no signing secret at all"
fi
if grep -q 'test-android-signing.sh' "$PRVAL"; then
    ok "pr-validation runs this suite"
else
    bad "this suite is in no workflow, so it gates nothing"
fi
# The selector, run rather than read: a suite-only change must select the job
# that runs the suite. Eight instances of this bug are recorded in that file.
selector=$(sed -n 's/^ *grep -Eq .\(\^(android.*\). <<<"\$files" && android=true/\1/p' "$PRVAL" | head -1)
if [ -z "$selector" ]; then
    selector=$(grep -o "'\^(android/[^']*'" "$PRVAL" | head -1 | tr -d "'")
fi
if [ -n "$selector" ]; then
    hit=0
    for path in tests/test-android-signing.sh android/tools/generate-signing-key.sh \
                android/signing-certificate.sha256 .github/workflows/release-android.yml; do
        printf '%s\n' "$path" | grep -Eq "$selector" || { bad "the android selector does not match $path"; hit=1; }
    done
    [ "$hit" = 0 ] && ok "the android job's path selector matches every file this unit adds"
else
    bad "could not find the android path selector in pr-validation.yml to test it"
fi

printf '\n%s\n' "────────────────────────────────────────────────────────────"
printf 'passed %d, failed %d\n' "$pass" "$fail"
printf 'every keystore this suite made is shredded on exit; none was a release key\n'
if [ "$((pass + fail))" -lt 40 ]; then
    echo "FATAL: only $((pass + fail)) assertions ran; this suite is supposed to make more than that, so something exited early" >&2
    exit 2
fi
[ "$fail" -eq 0 ] || exit 1
