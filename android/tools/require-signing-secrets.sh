#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  require-signing-secrets.sh — refuse the release when there is no key.
#
#  This is the refusal path, and it is a script rather than four lines inside
#  `release-android.yml` for one reason: a refusal that only exists inside a
#  workflow can only be tested by dispatching that workflow. Here
#  `tests/test-android-signing.sh` runs it under every permutation of missing
#  secrets and asserts what it says. The workflow calls it with the four
#  secrets in the step's `env:` — which is allowed; the `secrets` context is
#  forbidden in a step-level `if:`, not in `env:`.
#
#  WHAT IT WILL NOT DO: fall back to anything. Not the debug keystore (whose
#  password is the word "android" and which every Android developer on earth
#  has a copy of), not a key generated on the runner, not an unsigned APK. An
#  APK reaching even one phone signed by a key nobody keeps creates exactly the
#  lock-in this whole arrangement exists to avoid: Android will accept an
#  update only from the same key, so that phone is stuck with that build until
#  its owner uninstalls — and uninstalling destroys the paired device key.
#  Publishing nothing is recoverable in five minutes. Publishing that is not
#  recoverable at all.
#
#  Names only. No value of any secret is printed, compared against a literal,
#  or echoed in an error.
#
#  Exit: 0 all four present, 1 refusal.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

# ::error:: makes the line an annotation in the run summary rather than
# something you have to open the log to find. Outside Actions it is noise, so
# it is only emitted there.
annotate() {
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then echo "::error::$*"; fi
    echo "FATAL: $*" >&2
}

missing=()
for var in APEX_KEYSTORE_BASE64 APEX_KEYSTORE_PASSWORD APEX_KEY_ALIAS APEX_KEY_PASSWORD; do
    eval "value=\${$var:-}"
    [ -n "$value" ] || missing+=("$var")
done

if [ "${#missing[@]}" -gt 0 ]; then
    # Named ONE AT A TIME. "signing is not configured" sends somebody to check
    # four things; this says which. Half-configured is the case that would
    # otherwise die minutes later inside Gradle with a message about a
    # keystore, which reads like a mangled secret rather than an absent one.
    for var in "${missing[@]}"; do
        annotate "$var is not set on this repository."
    done
    annotate "This run could only produce an UNSIGNED APK, or one signed by a throwaway key. It will produce neither. See docs/android-signing.md: the key is generated on Andre's machine, the authoritative copy is his and offline, and GitHub holds a working copy that cannot be read back."
    exit 1
fi

# Not a gate — a notice, because it is a guess about a value this script
# deliberately cannot see the inside of. MEASURED 2026-09-20 with keytool from
# OpenJDK 21: a PKCS12 keystore cannot hold a key password different from the
# store password. keytool says so and ignores the one you gave it:
#   "Different store and key passwords not supported for PKCS12 KeyStores."
# So for the keystore generate-signing-key.sh produces, these two secrets hold
# the same value, and differing ones mean one of them was pasted wrong.
if [ "${APEX_KEYSTORE_PASSWORD}" != "${APEX_KEY_PASSWORD}" ]; then
    msg="APEX_KEY_PASSWORD differs from APEX_KEYSTORE_PASSWORD. A PKCS12 keystore cannot have separate passwords, so unless the keystore is an old-style JKS one of the two was pasted wrong and the build will fail later, inside Gradle, with a message about the keystore."
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then echo "::warning::$msg"; fi
    echo "warning: $msg" >&2
fi

# The paste mistake this catches is real and otherwise reads as a wrong
# password: `gh secret set NAME < file` stores the file's trailing newline, and
# the keystore then refuses to open with a password that looks correct in every
# password manager. Checked without printing anything: only whether the value
# has whitespace at either end.
for var in APEX_KEYSTORE_PASSWORD APEX_KEY_ALIAS APEX_KEY_PASSWORD; do
    eval "value=\${$var}"
    trimmed="${value#"${value%%[![:space:]]*}"}"
    trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
    if [ "$value" != "$trimmed" ]; then
        annotate "$var begins or ends with whitespace. That is almost always a trailing newline from 'gh secret set NAME < file', and it makes a correct password wrong. Set it again from a file with no trailing newline."
        exit 1
    fi
done

echo "signing secrets: all four are set"
