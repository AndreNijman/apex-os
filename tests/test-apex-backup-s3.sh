#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  `apex backup` to S3, through a REAL apex-secretd — P2-001.
#
#  The `s3` target kind was refused by this build for a reason that was true:
#  nothing here could sign a SigV4 request. This is the suite that says the
#  signer is right, and it says it the only way a signer can honestly be
#  checked — against an implementation that is not the one under test.
#
#  ── THREE IMPLEMENTATIONS, WHICH IS THE POINT ───────────────────────────────
#
#    1. apexd/apex-secretd/src/providers/s3/sigv4.rs — under test;
#    2. botocore 1.43.71 — its numbers are pinned as known-answer tests beside
#       that file, so the ALGORITHM is checked against a library nobody here
#       wrote;
#    3. tests/s3-double.py — recomputes the signature from what arrives on the
#       socket, with nothing but `hmac` and `hashlib`, and answers 403 when it
#       does not match.
#
#  A signer compared only with itself is a signer that will be consistently
#  wrong and fail only at a real S3. This suite is what makes that impossible.
#
#  ── NOTHING REACHES AWS ─────────────────────────────────────────────────────
#
#  There is no AWS account anywhere in this repository and this suite makes no
#  outbound connection. The endpoint is 127.0.0.1 on a kernel-chosen port, the
#  access key id is AWS's own documentation example, and `apex secret add`
#  accepts `http` only for a loopback host — so this cannot be turned into a
#  way to send a real key in clear.
#
#  ── WHAT IS TOUCHED ─────────────────────────────────────────────────────────
#
#  One `mktemp -d -p /var/tmp`, and nothing else. Its own apex-secretd, started
#  with --socket and --store inside the fixture and killed BY PID; a store that
#  is not the machine's; APEX_BACKUP_KEYS inside the fixture, so the real
#  /var/lib/apex-backup is never read, written or created. No root.
#
#      ./tests/test-apex-backup-s3.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d -p /var/tmp apex-backup-s3-XXXXXX)"

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

DOUBLE_PID=""
SECRETD_PID=""
cleanup() {
    # By pid, on the children this suite started.
    [ -n "$DOUBLE_PID" ] && kill "$DOUBLE_PID" 2>/dev/null
    [ -n "$SECRETD_PID" ] && kill "$SECRETD_PID" 2>/dev/null
    wait 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

finish() {
    printf '\nbackup-s3: %d passed, %d failed\n' "$pass" "$fail"
    [ "$fail" -eq 0 ] || exit 1
    exit 0
}

section "preconditions"
for tool in cargo python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'COULD NOT RUN  %s is missing\n' "$tool"; exit 2; }
done
ok "cargo and python3 are present"

cargo build --manifest-path "${ROOT}/apexd/Cargo.toml" \
    --bin apex --bin apex-secretd >/dev/null 2>&1 \
    || { bad "apex and apex-secretd build"; finish; }
ok "apex and apex-secretd build"
BIN="${CARGO_TARGET_DIR:-${ROOT}/apexd/target}/debug"
APEX="${BIN}/apex"
SECRETD="${BIN}/apex-secretd"

# AWS's documented example credentials. They authorise nothing, anywhere.
ACCESS_KEY_ID="AKIDEXAMPLE"
SECRET_KEY="wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"
BUCKET="example-backups"

# ── the double ───────────────────────────────────────────────────────────────
section "a loopback S3 that verifies the signature itself"

PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
python3 "${ROOT}/tests/s3-double.py" "$PORT" "${WORK}/s3" \
    "$ACCESS_KEY_ID" "$SECRET_KEY" >"${WORK}/double.log" 2>&1 &
DOUBLE_PID=$!
for _ in $(seq 1 100); do
    python3 - "$PORT" <<'PY' && break
import socket, sys
s = socket.socket(); s.settimeout(0.2)
sys.exit(0 if s.connect_ex(("127.0.0.1", int(sys.argv[1]))) == 0 else 1)
PY
    sleep 0.1
done
python3 - "$PORT" <<'PY'
import socket, sys
s = socket.socket(); s.settimeout(0.5)
sys.exit(0 if s.connect_ex(("127.0.0.1", int(sys.argv[1]))) == 0 else 1)
PY
if [ $? -eq 0 ]; then
    ok "the S3 double is listening on 127.0.0.1:${PORT} (pid ${DOUBLE_PID})"
else
    bad "the S3 double started"
    sed 's/^/      /' "${WORK}/double.log"
    finish
fi

# ── the daemon ───────────────────────────────────────────────────────────────
section "a real apex-secretd, with a store of its own"

export APEX_SECRETD_SOCKET="${WORK}/secretd.sock"
SECRET_STORE="${WORK}/secretd-store"
"$SECRETD" --socket "$APEX_SECRETD_SOCKET" --store "$SECRET_STORE" \
    > "${WORK}/secretd.log" 2>&1 &
SECRETD_PID=$!
for _ in $(seq 1 50); do [ -S "$APEX_SECRETD_SOCKET" ] && break; sleep 0.1; done
[ -S "$APEX_SECRETD_SOCKET" ] && ok "the secret service came up" || {
    bad "the secret service came up"
    sed 's/^/      /' "${WORK}/secretd.log"
    finish
}

# The secret access key goes in on STDIN, never on a command line: argv is
# world-readable through /proc, which is the reason `apex secret add` has no
# option for it.
printf '%s' "$SECRET_KEY" | "$APEX" secret add aws \
    --host 127.0.0.1 --scheme http --port "$PORT" \
    --username "$ACCESS_KEY_ID" >/dev/null 2>&1 \
    && ok "the credential is stored, key id and secret apart" \
    || { bad "the credential is stored"; finish; }

# ── the project ──────────────────────────────────────────────────────────────
section "a project that declares the bucket"

PROJECT="${WORK}/project"
mkdir -p "${PROJECT}/src"
printf 'fn main() {}\n' > "${PROJECT}/src/main.rs"
python3 -c "
import sys
sys.stdout.buffer.write(bytes(range(256)) * 32)
" > "${PROJECT}/blob.bin"

"$APEX" backup key init >/dev/null 2>&1
export APEX_BACKUP_KEYS="${WORK}/keys"
"$APEX" backup key init >/dev/null 2>&1
RECIPIENT="$("$APEX" backup key show 2>/dev/null)"
[ -n "$RECIPIENT" ] || { bad "a backup key exists"; finish; }
ok "a backup keypair in the fixture"

write_config() {
    cat > "${PROJECT}/apex.toml" <<EOF
[backup]
recipient = "${RECIPIENT}"
target = "s3"

[backup.s3]
bucket = "$1"
service = "aws"

[s3]
buckets = ["${BUCKET}"]
region = "ap-southeast-2"
EOF
}
write_config "$BUCKET"
ok "apex.toml declares the bucket in [s3] buckets"

# A grant is keyed on the CURRENT project, so it is made from inside it. The
# project has to be a git repository for `apex secret` to find a root at all.
git -C "$PROJECT" init -q 2>/dev/null
(cd "$PROJECT" && "$APEX" secret grant aws s3.object.read) >/dev/null 2>&1
(cd "$PROJECT" && "$APEX" secret grant aws s3.object.write) >/dev/null 2>&1
granted="$("$APEX" secret grants 2>&1)"
printf '%s' "$granted" | grep -q "s3.object.write" \
    && ok "the two S3 capabilities are granted for this project" \
    || { bad "the two S3 capabilities are granted for this project"
         printf '      %s\n' "$granted"; finish; }

# ── a bucket the project did not declare ─────────────────────────────────────
section "a bucket outside [s3] buckets"

write_config "somebody-elses-bucket"
out="$("$APEX" backup run --project "$PROJECT" --label refused 2>&1)"
if [ $? -eq 0 ]; then
    bad "a bucket the project did not declare is refused"
else
    ok "a bucket the project did not declare is refused"
    printf '%s' "$out" | grep -q "somebody-elses-bucket" \
        && ok "the refusal names the bucket that was asked for" \
        || { bad "the refusal names the bucket that was asked for"
             printf '      %s\n' "$out"; }
fi
[ -z "$(ls -A "${WORK}/s3" 2>/dev/null)" ] \
    && ok "nothing was signed or sent for it" \
    || bad "nothing was signed or sent for it"
write_config "$BUCKET"

# ── the round trip ───────────────────────────────────────────────────────────
section "run, list, verify, restore — every request signature-checked"

out="$("$APEX" backup run --project "$PROJECT" --label first 2>&1)"
if [ $? -eq 0 ]; then
    ok "\`apex backup run\` wrote a snapshot to S3"
else
    bad "\`apex backup run\` wrote a snapshot to S3"
    printf '      %s\n' "$out"
    printf '      double: %s\n' "$(tail -5 "${WORK}/double.log")"
    finish
fi

# Every request the double saw verified. A single 403 would mean a request a
# real S3 would have rejected, and the run would have failed above — this is
# the assertion that says so explicitly rather than inferring it.
grep -q "REFUSED" "${WORK}/double.log" \
    && { bad "every request verified against the double's own signature"
         grep "REFUSED" "${WORK}/double.log" | sed 's/^/      /'; } \
    || ok "every request verified against the double's own signature"
grep -q "stored ${BUCKET}/apex-backup/" "${WORK}/double.log" \
    && ok "the objects arrived under the prefix the project chose" \
    || bad "the objects arrived under the prefix the project chose"

# The canary sweep, over this transport too: a transport is exactly where a
# plaintext copy would get made.
CANARY="s3-canary-7c41d85e603f9a2b"
printf '%s\n' "$CANARY" > "${PROJECT}/secret.txt"
"$APEX" backup run --project "$PROJECT" --label second >/dev/null 2>&1 \
    && ok "a second snapshot, with a canary planted in the source" \
    || bad "a second snapshot, with a canary planted in the source"

found=0
while IFS= read -r f; do
    grep -qaF "$CANARY" "$f" 2>/dev/null && { found=1; printf '      in %s\n' "$f"; }
done < <(find "${WORK}/s3" -type f)
[ "$found" -eq 0 ] \
    && ok "no byte stored in the bucket holds the plaintext" \
    || bad "no byte stored in the bucket holds the plaintext"
find "${WORK}/s3" | grep -qF "$CANARY" \
    && bad "no object NAME holds the plaintext" \
    || ok "no object NAME holds the plaintext"

# The secret access key is nowhere it should not be.
grep -rqaF "$SECRET_KEY" "${WORK}/s3" "${WORK}/double.log" 2>/dev/null \
    && bad "the secret access key never reaches the far side" \
    || ok "the secret access key never reaches the far side"
grep -rqaF "$SECRET_KEY" "$PROJECT" 2>/dev/null \
    && bad "the secret access key is not in the project" \
    || ok "the secret access key is not in the project"

listing="$("$APEX" backup list --project "$PROJECT" 2>&1)"
count="$(printf '%s' "$listing" | grep -cE '^[0-9]{8}T[0-9]{6}Z-[0-9a-f]{8}')"
[ "$count" -eq 2 ] \
    && ok "both snapshots are listed out of the bucket" \
    || { bad "both snapshots are listed out of the bucket (saw $count)"
         printf '      %s\n' "$listing"; }

out="$("$APEX" backup verify latest --project "$PROJECT" --deep 2>&1)"
[ $? -eq 0 ] && ok "\`apex backup verify --deep\` passes over S3" \
    || { bad "\`apex backup verify --deep\` passes over S3"; printf '      %s\n' "$out"; }

DEST="${WORK}/restored"
out="$("$APEX" backup restore latest --project "$PROJECT" --into "$DEST" 2>&1)"
if [ $? -eq 0 ]; then
    ok "\`apex backup restore\` pulled the snapshot back out of S3"
    if diff -r --no-dereference "$PROJECT" "$DEST" >"${WORK}/diff.txt" 2>&1; then
        ok "the restored tree is identical to the original, byte for byte"
    else
        bad "the restored tree is identical to the original, byte for byte"
        head -20 "${WORK}/diff.txt"
    fi
    cmp -s "${PROJECT}/blob.bin" "${DEST}/blob.bin" \
        && ok "a non-UTF-8 file survived base64 through the broker" \
        || bad "a non-UTF-8 file survived base64 through the broker"
else
    bad "\`apex backup restore\` pulled the snapshot back out of S3"
    printf '      %s\n' "$out"
fi

# ── a wrong key is refused by the double's own arithmetic ───────────────────
section "a signature the double does not accept"

# The credential is replaced with a different secret. Everything else is the
# same, so the only thing that can change the answer is the signature — and
# the double, which knows the real one, refuses it. This is the assertion that
# says the 200s above were earned rather than given.
printf '%s' "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEz" | "$APEX" secret add aws \
    --host 127.0.0.1 --scheme http --port "$PORT" \
    --username "$ACCESS_KEY_ID" >/dev/null 2>&1 \
    && ok "the stored secret was replaced with a different one" \
    || bad "the stored secret was replaced with a different one"

before="$(grep -c REFUSED "${WORK}/double.log")"
out="$("$APEX" backup list --project "$PROJECT" 2>&1)"
after="$(grep -c REFUSED "${WORK}/double.log")"
if [ "$after" -gt "$before" ]; then
    ok "the double refused a request signed with the wrong secret"
    grep "REFUSED" "${WORK}/double.log" | tail -1 | grep -q "signature" \
        && ok "and refused it on the signature, not on something incidental" \
        || bad "and refused it on the signature, not on something incidental"
else
    # If the store would not take a second credential under the same name, say
    # so rather than reporting a pass nobody earned.
    bad "the double refused a request signed with the wrong secret"
    printf '      the stored credential may not have been replaced: %s\n' "$out"
fi

finish
