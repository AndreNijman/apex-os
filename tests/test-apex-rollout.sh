#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-rollout.sh — §26's staged rollout, end to end, against real
#  cryptography and the workflow's own publisher code.
#
#  ── What this proves that a unit test cannot ────────────────────────────────
#  `apexd-core::channel`'s decision table is pure and is tested there. This
#  suite covers the part between the registry and that table, which is where
#  the two halves can disagree without anybody noticing:
#
#   1. The document the PUBLISHER writes is the document the CLIENT parses. The
#      fixture does not hand-write a rollout document — it EXTRACTS the python
#      out of .github/workflows/promote-channel.yml and runs it. A field
#      renamed on either side fails this suite, which is the only thing
#      connecting a workflow that has never run to a client that has to read
#      what it writes.
#   2. The OCI object the publisher assembles is one `verify::fetch` can read,
#      built by the same extracted python.
#   3. The signature is verified BEFORE the bytes are parsed, under the
#      rollout identity — which is a DIFFERENT Sigstore identity from the
#      image's, derived by `trust::expected_rollout_signer`.
#   4. A machine outside the ramp is held, and a halt stops every machine —
#      driven through the real binary, with the slot set by the fixture.
#
#  ── What is deliberately NOT done ──────────────────────────────────────────
#  No image is staged, no deployment is touched, `bootc` is never spawned and
#  nothing reaches the network. Every registry answer is a file in the fixture
#  root, which is the path `verify::fetch` takes under APEX_TRUST_ROOT.
#
#  ── The cryptography is real ────────────────────────────────────────────────
#  Minted P-256 root, intermediate and leaf, a Sigstore SAN and OIDC-issuer
#  extension on the leaf, and a genuine cosign simple-signing payload signed
#  with `openssl dgst -sha256 -sign`. The binary does the whole verification.
#
#  Usage: tests/test-apex-rollout.sh
#         APEX=/path/to/apex tests/test-apex-rollout.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
has() { if grep -qF -- "$1" "$2"; then ok "$3"; else
        bad "$3 — no '$1' in:"; sed 's/^/       /' "$2" >&2; fi; }
hasnt() { if grep -qF -- "$1" "$2"; then
        bad "$3 — found '$1' in:"; sed 's/^/       /' "$2" >&2; else ok "$3"; fi; }

command -v openssl >/dev/null 2>&1 || { echo "FATAL: no openssl" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "FATAL: no python3" >&2; exit 1; }

APEX="${APEX:-$REPO/apexd/target/debug/apex}"
[[ -x "$APEX" ]] || APEX="${CARGO_TARGET_DIR:-}/debug/apex"
if [[ ! -x "$APEX" ]]; then
    echo "FATAL: no apex binary. Build it, or set APEX=/path/to/apex." >&2
    echo "       A skipped assertion reports as a pass, which is the bug this refuses." >&2
    exit 1
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/apex-rollout.XXXXXX")"
trap 'chmod -R u+rwX "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

WORKFLOW="$REPO/.github/workflows/promote-channel.yml"
IMAGE='ghcr.io/andrenijman/apex-os'
# The identity the IMAGE is signed under, and the one the document is signed
# under. The second is not written into the fixture: the binary derives it, and
# that derivation is one of the things under test.
SIGNER="https://github.com/AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/main"
ROLLOUT_SIGNER="https://github.com/AndreNijman/apex-os/.github/workflows/promote-channel.yml@refs/heads/main"
ISSUER='https://token.actions.githubusercontent.com'
# What `:candidate` resolves to in the fixture registry — the digest a machine
# on that channel is about to pull, and the one a rollout entry has to name.
DIGEST='sha256:daf8c8eb2928ab995a67ea9df43aa78116f638278bd0e7d32135a8b272e4ebec'
OTHER='sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
# Machine ids whose slot is known. MEASURED against the binary rather than
# reimplemented: `channel::bucket` is FNV-1a with a multiplier one hex digit
# longer than the published FNV prime, and a second implementation of it here
# chose two ids that landed in the wrong buckets — the ramp assertions passed
# and the admitted one did not, which is the good version of that mistake.
# The slot is asserted below out of the binary's own output for the same reason.
MID_SLOT_5='000000000000000000000000000000ca'
MID_SLOT_60='00000000000000000000000000000029'

# ═════════════════════════════════════════════════════════════════════════════
# The publisher's own code, lifted out of the workflow.
#
# Not a copy. A copy is how the writer and the reader drift, and this whole
# mechanism is one workflow writing a document one binary parses with nothing
# else between them.
# ═════════════════════════════════════════════════════════════════════════════
extract_step() {
    WORKFLOW="$WORKFLOW" STEP="$1" python3 - <<'PY'
import os, re, sys, yaml
d = yaml.safe_load(open(os.environ["WORKFLOW"]))
want = os.environ["STEP"]
for s in d["jobs"]["promote"]["steps"]:
    if s.get("name") == want:
        body = re.search(r"python3 - <<'PY'\n(.*?)\nPY\n", s["run"], re.S)
        if not body:
            sys.exit(f"the step {want!r} has no python block any more")
        print(body.group(1))
        sys.exit(0)
sys.exit(f"the workflow has no step named {want!r}")
PY
}

WRITER="$TMP/writer.py"
PACKER="$TMP/packer.py"
if ! extract_step 'Write the new rollout document' > "$WRITER" 2>"$TMP/x.err"; then
    echo "FATAL: could not lift the document writer out of $WORKFLOW:" >&2
    cat "$TMP/x.err" >&2; exit 1
fi
if ! extract_step 'Publish and sign it' > "$PACKER" 2>"$TMP/x.err"; then
    echo "FATAL: could not lift the artifact packer out of $WORKFLOW:" >&2
    cat "$TMP/x.err" >&2; exit 1
fi
[[ -s "$WRITER" && -s "$PACKER" ]] || { echo "FATAL: an extracted step is empty" >&2; exit 1; }

# ── one certificate authority, minted once ───────────────────────────────────
CA="$TMP/ca"; mkdir -p "$CA/db"; : > "$CA/db/index.txt"
CA_NB="$(date -u -d '1 year ago' +%Y%m%d%H%M%SZ)"
CA_NA="$(date -u -d '9 years' +%Y%m%d%H%M%SZ)"
cat > "$CA/ca.cnf" <<CNF
[ca]
default_ca = apex_test
[apex_test]
dir             = $CA
database        = \$dir/db/index.txt
new_certs_dir   = \$dir/db
default_md      = sha256
policy          = apex_pol
email_in_dn     = no
rand_serial     = yes
unique_subject  = no
[apex_pol]
countryName             = optional
stateOrProvinceName     = optional
localityName            = optional
organizationName        = optional
organizationalUnitName  = optional
commonName              = optional
emailAddress            = optional
CNF

# `openssl ca -startdate/-enddate` rather than `x509 -req -not_before`: the
# latter needs OpenSSL 3.5 and the CI runner is ubuntu-24.04 with 3.0.13.
# test-apex-trust-enforcement.sh records what happens when only one of the two
# minting sites feature-detects.
sign_cert() {
    local csr="$1" key="$2" signer="$3" nb="$4" na="$5" ext="$6" out="$7"
    local args=(-batch -config "$CA/ca.cnf" -notext -md sha256
                -keyfile "$key" -startdate "$nb" -enddate "$na"
                -extfile "$ext" -in "$csr" -out "$out")
    if [ "$signer" = SELFSIGN ]; then args+=(-selfsign); else args+=(-cert "$signer"); fi
    openssl ca "${args[@]}" >/dev/null 2>&1
}
printf 'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n' > "$CA/ca.ext"
openssl ecparam -name prime256v1 -genkey -noout -out "$CA/root.key" 2>/dev/null
openssl req -new -key "$CA/root.key" -subj '/O=apex test/CN=apex test root' -out "$CA/root.csr" 2>/dev/null
sign_cert "$CA/root.csr" "$CA/root.key" SELFSIGN "$CA_NB" "$CA_NA" "$CA/ca.ext" "$CA/root.pem"
openssl ecparam -name prime256v1 -genkey -noout -out "$CA/int.key" 2>/dev/null
openssl req -new -key "$CA/int.key" -subj '/O=apex test/CN=apex test intermediate' -out "$CA/int.csr" 2>/dev/null
sign_cert "$CA/int.csr" "$CA/root.key" "$CA/root.pem" "$CA_NB" "$CA_NA" "$CA/ca.ext" "$CA/int.pem"
[ -s "$CA/root.pem" ] && [ -s "$CA/int.pem" ] \
    || { echo "FATAL: the test CA did not mint; openssl is $(openssl version)" >&2; exit 1; }

# mint_leaf <outdir> <san-uri>
#
# The ten-minute window Fulcio actually issues, placed in the past — the state
# every real signature is in by the time a machine reads it. A verifier that
# forgot `openssl verify -attime` would refuse every document.
mint_leaf() {
    local out="$1" san="$2"
    openssl ecparam -name prime256v1 -genkey -noout -out "$out/leaf.key" 2>/dev/null
    openssl req -new -key "$out/leaf.key" -subj '/CN=apex rollout leaf' -out "$out/leaf.csr" 2>/dev/null
    printf 'subjectAltName=critical,URI:%s\n1.3.6.1.4.1.57264.1.8=ASN1:UTF8String:%s\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=codeSigning\n' \
        "$san" "$ISSUER" > "$out/leaf.ext"
    sign_cert "$out/leaf.csr" "$CA/int.key" "$CA/int.pem" \
        "$(date -u -d '2 hours ago' +%Y%m%d%H%M%SZ)" \
        "$(date -u -d '110 minutes ago' +%Y%m%d%H%M%SZ)" "$out/leaf.ext" "$out/leaf.pem"
}

# ═════════════════════════════════════════════════════════════════════════════
# fixture <name> <machine-id> <channel> <percent> <halt> <reason> [signer] [entry-digest]
#
# Builds a complete fixture root: a deployment on `:candidate`, a registry that
# answers for the channel tag and the rollout tag, and a signed rollout
# document produced by the workflow's own python.
# ═════════════════════════════════════════════════════════════════════════════
fixture() {
    local name="$1" mid="$2" chan="$3" percent="$4" halt="$5" reason="$6"
    local signer="${7:-$ROLLOUT_SIGNER}" entry_digest="${8:-$DIGEST}"
    local root="$TMP/$name" work="$TMP/$name.work"
    local csum=f3f505fc39fb268c59f4458365c96b764a7bd7d30f2f51e98bb6a009666b7852
    local bootcsum=1d98b51dd76621b656c50e4f22dc7e5eade9b0f869443a3efa90eee08eb9373e

    mkdir -p "$root/proc" "$root/etc" "$root/usr/share/apex-os/trust" "$root/registry" \
             "$root/ostree/boot.0/default/$bootcsum" \
             "$root/ostree/deploy/default/deploy/$csum.0" "$work"
    printf 'root=UUID=x rw quiet ostree=/ostree/boot.0/default/%s/0\n' "$bootcsum" > "$root/proc/cmdline"
    ln -sfn "../../../deploy/default/deploy/$csum.0" "$root/ostree/boot.0/default/$bootcsum/0"
    printf '[origin]\ncontainer-image-reference=ostree-unverified-registry:%s:candidate\n' "$IMAGE" \
        > "$root/ostree/deploy/default/deploy/$csum.0.origin"
    printf '{"deployments":[{"booted":true,"base-commit-meta":{"ostree.manifest-digest":"%s"}}]}\n' \
        "$DIGEST" > "$root/rpm-ostree-status.json"
    printf '%s\n' "$mid" > "$root/etc/machine-id"
    cp "$CA/root.pem" "$root/usr/share/apex-os/trust/fulcio-root.pem"
    # The IMAGE signer. The rollout signer is NOT written: the binary derives it
    # by swapping the workflow filename, and that derivation is under test.
    printf '%s\n' "$SIGNER" > "$root/usr/share/apex-os/trust/expected-signer"
    printf '%s\n' "$ISSUER" > "$root/usr/share/apex-os/trust/expected-issuer"
    # Two tags, two answers. One `registry/resolve` for both would make the
    # document's digest binding untestable.
    printf '%s\n' "$DIGEST" > "$root/registry/resolve.candidate"

    # ── the document, written by the workflow's own code ─────────────────────
    # ROLLOUT_DIR is the workflow's own redirection point, defaulted to /tmp
    # there and pointed here. Nothing about the document is written by this
    # suite: a copy of the publisher is how the publisher and the client drift.
    TARGET="$chan" DIGEST="$entry_digest" PERCENT="$percent" HALT="$halt" \
        REASON="$reason" FOUND=no IMAGE="$IMAGE" ROLLOUT_DIR="$work" \
        python3 "$WRITER" >/dev/null \
      || { echo "FATAL: the workflow's document writer failed" >&2; exit 1; }
    [ -s "$work/rollout.json" ] \
      || { echo "FATAL: the writer produced no rollout.json" >&2; exit 1; }

    # ── the OCI object, packed by the workflow's own code ────────────────────
    mkdir -p "$work/rollout-new"
    ROLLOUT_DIR="$work" python3 "$PACKER" \
      || { echo "FATAL: the workflow's artifact packer failed" >&2; exit 1; }
    [ -s "$work/rollout-new/manifest.json" ] \
      || { echo "FATAL: the packer produced no manifest" >&2; exit 1; }
    cp -r "$work/rollout-new" "$root/registry/rollout"

    # The digest of the object the packer wrote, which is what cosign signs and
    # what `verify::resolve` must answer for `:rollout`.
    local rhex rdigest
    rhex="$(openssl dgst -sha256 -r "$root/registry/rollout/manifest.json" | awk '{print $1}')"
    rdigest="sha256:$rhex"
    printf '%s\n' "$rdigest" > "$root/registry/resolve.rollout"

    # ── a genuine cosign signature over that digest ──────────────────────────
    mint_leaf "$work" "$signer"
    printf '{"critical":{"identity":{"docker-reference":"%s"},"image":{"docker-manifest-digest":"%s"},"type":"cosign container image signature"},"optional":null}' \
        "$IMAGE" "$rdigest" > "$work/payload.json"
    openssl dgst -sha256 -sign "$work/leaf.key" -out "$work/sig.der" "$work/payload.json" 2>/dev/null
    openssl x509 -pubkey -noout -in "$work/leaf.pem" > "$work/pub.pem" 2>/dev/null
    # A fixture that fails to be what it claims makes a test that passes for the
    # wrong reason. Checked with openssl before the binary is ever asked.
    openssl dgst -sha256 -verify "$work/pub.pem" -signature "$work/sig.der" "$work/payload.json" >/dev/null 2>&1 \
      || { echo "FATAL: the '$name' fixture's own signature does not verify" >&2; exit 1; }

    local art="$root/registry/${rdigest/:/-}.sig"; mkdir -p "$art"
    local phex; phex="$(openssl dgst -sha256 -r "$work/payload.json" | awk '{print $1}')"
    cp "$work/payload.json" "$art/$phex"
    SIG_B64="$(base64 -w0 < "$work/sig.der")" LEAF="$(cat "$work/leaf.pem")" \
    CHAIN="$(cat "$CA/int.pem")" HEX="$phex" SIZE="$(wc -c < "$work/payload.json")" \
    OUT="$art/manifest.json" python3 - <<'PY'
import json, os
json.dump({
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "layers": [{
    "mediaType": "application/vnd.dev.cosign.simplesigning.v1+json",
    "size": int(os.environ["SIZE"]),
    "digest": "sha256:" + os.environ["HEX"],
    "annotations": {
      "dev.cosignproject.cosign/signature": os.environ["SIG_B64"],
      "dev.sigstore.cosign/certificate": os.environ["LEAF"],
      "dev.sigstore.cosign/chain": os.environ["CHAIN"],
    },
  }],
}, open(os.environ["OUT"], "w"))
PY
    [ -s "$art/manifest.json" ] || { echo "FATAL: no signature manifest for '$name'" >&2; exit 1; }
    printf '%s' "$root"
}

run() { APEX_TRUST_ROOT="$1" "$APEX" "${@:2}" > "$TMP/out" 2> "$TMP/err"; echo $?; }
both() { cat "$TMP/out" "$TMP/err" > "$TMP/all"; }
jq_get() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get(sys.argv[2])))' "$TMP/out" "$1"; }

# ═════════════════════════════════════════════════════════════════════════════
sec "the publisher and the client agree on the document, by construction"
R="$(fixture ramp "$MID_SLOT_60" candidate 25 false '')"
rc="$(run "$R" channel rollout --json)"
both
[[ "$rc" == 0 ]] || { bad "apex channel rollout exited $rc"; sed 's/^/       /' "$TMP/all" >&2; }
if python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$TMP/out" 2>/dev/null; then
    ok "apex channel rollout --json emits JSON"
else
    bad "apex channel rollout --json did not emit JSON"; sed 's/^/       /' "$TMP/all" >&2
fi
[[ "$(jq_get documentFrom)" == '"registry"' ]] \
    && ok "the document the workflow wrote was fetched, verified and parsed" \
    || { bad "documentFrom is $(jq_get documentFrom), not registry"; sed 's/^/       /' "$TMP/all" >&2; }
[[ "$(jq_get fetchError)" == 'null' ]] \
    && ok "and nothing about the fetch was ignored" \
    || bad "fetchError is $(jq_get fetchError)"
[[ "$(jq_get notes)" == '[]' ]] \
    && ok "and the decision discarded nothing" \
    || bad "notes is $(jq_get notes)"
[[ "$(jq_get percent)" == '25' ]] \
    && ok "the ramp the workflow published is the ramp the client read" \
    || bad "percent is $(jq_get percent), expected 25"

sec "a machine outside the ramp is held, and is told it is not broken"
[[ "$(jq_get verdict)" == '"held"' ]] \
    && ok "slot 60 is held by a 25% ramp" \
    || bad "verdict is $(jq_get verdict), expected held"
[[ "$(jq_get slot)" == '60' ]] \
    && ok "and the slot the binary derived from the fixture's machine-id is 60" \
    || bad "slot is $(jq_get slot), expected 60"
rc="$(run "$R" channel rollout)"; both
has 'held' "$TMP/all" "the report says held"
has 'not its turn' "$TMP/all" "and says it is not its turn, rather than implying a fault"

sec "a machine inside the ramp takes it"
R5="$(fixture inramp "$MID_SLOT_5" candidate 25 false '')"
rc="$(run "$R5" channel rollout --json)"; both
[[ "$(jq_get verdict)" == '"admitted"' ]] \
    && ok "slot 5 is admitted by the same 25% ramp" \
    || { bad "verdict is $(jq_get verdict), expected admitted"; sed 's/^/       /' "$TMP/all" >&2; }
[[ "$(jq_get slot)" == '5' ]] && ok "and its slot is 5" || bad "slot is $(jq_get slot)"

sec "a halt stops every machine, and carries the publisher's words"
RH="$(fixture halted "$MID_SLOT_5" candidate 100 true 'gpu-driver fails to bind on RTX 30-series')"
rc="$(run "$RH" channel rollout --json)"; both
[[ "$(jq_get verdict)" == '"halted"' ]] \
    && ok "a halt beats a 100% ramp and the machine's slot" \
    || { bad "verdict is $(jq_get verdict), expected halted"; sed 's/^/       /' "$TMP/all" >&2; }
has 'RTX 30-series' "$TMP/out" "and the reason reaches the user verbatim"

sec "an entry about a digest this channel no longer serves is stale"
RS="$(fixture stale "$MID_SLOT_60" candidate 1 false '' "$ROLLOUT_SIGNER" "$OTHER")"
rc="$(run "$RS" channel rollout --json)"; both
[[ "$(jq_get verdict)" == '"admitted"' ]] \
    && ok "a 1% entry for another digest does not hold this one back" \
    || { bad "verdict is $(jq_get verdict)"; sed 's/^/       /' "$TMP/all" >&2; }
has 'stale' "$TMP/out" "and the machine says the entry was stale rather than silently ignoring it"

sec "a document signed by the wrong identity is not a document"
# The image's own signer, which is the mistake a reasonable person makes: the
# document lives in the same repository and is signed by the same account. It
# is a different WORKFLOW, and the client derives that.
RW="$(fixture wrongsigner "$MID_SLOT_60" candidate 1 false '' "$SIGNER")"
rc="$(run "$RW" channel rollout --json)"; both
[[ "$(jq_get verdict)" == '"admitted"' ]] \
    && ok "a ramp signed by the image workflow does not hold anybody back" \
    || { bad "verdict is $(jq_get verdict) — an unverified document was obeyed"; sed 's/^/       /' "$TMP/all" >&2; }
[[ "$(jq_get documentFrom)" == '"none"' ]] \
    && ok "and it was not used at all" \
    || bad "documentFrom is $(jq_get documentFrom)"
has 'did not verify' "$TMP/out" "and the reason names the signature, not the network"
hasnt 'RTX' "$TMP/out" "nothing from an unverified document reaches the decision"

sec "the publisher's serial is monotonic, which is what makes a replay detectable"
# The workflow's own writer, run twice: the second run must produce a HIGHER
# serial than the document it read. A publisher that reset the counter would
# make every machine refuse its next real document as a replay, and the client
# cannot tell the two apart — the defence has to be correct on the writing side.
SER="$TMP/serial"; mkdir -p "$SER"
TARGET=candidate DIGEST="$DIGEST" PERCENT=5 HALT=false REASON='' FOUND=no \
    IMAGE="$IMAGE" ROLLOUT_DIR="$SER" python3 "$WRITER" >/dev/null
cp "$SER/rollout.json" "$SER/rollout-current.json"
first="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["serial"])' "$SER/rollout.json")"
# And a halt on a DIFFERENT channel, so the carry-forward is exercised too.
TARGET=beta DIGEST="$OTHER" PERCENT=100 HALT=true REASON='boot loop' FOUND=yes \
    IMAGE="$IMAGE" ROLLOUT_DIR="$SER" python3 "$WRITER" >/dev/null
second="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["serial"])' "$SER/rollout.json")"
[[ "$second" -gt "$first" ]] \
    && ok "a second publish raises the serial ($first -> $second)" \
    || bad "the serial went $first -> $second; every machine would refuse the next document as a replay"
if python3 - "$SER/rollout.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
chans = {e["channel"]: e for e in d["channels"]}
sys.exit(0 if chans.get("candidate", {}).get("percent") == 5 and chans.get("beta", {}).get("halt") else 1)
PY
then ok "and promoting one channel leaves the other channel's entry alone"
else bad "publishing beta cleared candidate's ramp"; cat "$SER/rollout.json" >&2; fi

sec "the update path consults this, and not only the report verb"
# A stop nobody's update path reads is a report. Asserted on the source, because
# `apex update` needs root and its next step stages an image.
OPS="$REPO/apexd/apex/src/ops.rs"
if grep -q 'crate::channel::rollout_hold()' "$OPS"; then
    ok "ops::update calls channel::rollout_hold"
else
    bad "ops::update does not call the rollout gate — the ramp would be a readout"
fi
# Before the write that arms the next update's health gate, and after the
# signature gate, which is a harder stop.
if python3 - "$OPS" <<'PY'
import sys
s = open(sys.argv[1]).read()
i = s.index("crate::channel::rollout_hold()")
j = s.index("crate::channel::record_update")
k = s.index("if let Some(code) = trust_gate(")
sys.exit(0 if k < i < j else 1)
PY
then ok "and it runs after the signature gate and before the health record is written"
else bad "the rollout gate is in the wrong place in ops::update"
fi

sec "the workflow the client's trust derivation names still exists"
# `trust::expected_rollout_signer` swaps `build-image.yml` for
# `promote-channel.yml`. Renaming the file would make every machine reject
# every document, silently, as "no rollout is staged".
[[ -f "$REPO/.github/workflows/promote-channel.yml" ]] \
    && ok "promote-channel.yml is where the derived identity says it is" \
    || bad "no .github/workflows/promote-channel.yml — every rollout document would be refused"
if grep -q 'ROLLOUT_WORKFLOW: &str = "promote-channel.yml"' "$REPO/apexd/apex/src/trust.rs"; then
    ok "and trust.rs derives that exact filename"
else
    bad "trust.rs does not derive promote-channel.yml"
fi
if grep -q 'cosign sign --yes "$IMAGE@$got"' "$WORKFLOW"; then
    ok "and the workflow signs the object it publishes"
else
    bad "promote-channel.yml does not sign the rollout object"
fi

sec "status is still root-free and contacts nothing"
# The property `docs/update-channels.md` promises. `rollout` is the verb that
# goes to the registry; `status` must not have quietly acquired that.
if grep -q 'ChannelCmd::Status { json } => status(json)' "$REPO/apexd/apex/src/channel.rs"; then
    ok "status is still its own verb"
else
    bad "the status verb changed shape"
fi
rc="$(run "$R" channel status)"; both
hasnt 'rollout document' "$TMP/all" "status does not fetch the rollout document"

sec "the bytes read are the bytes the signature was checked over"
# The attack the ordering alone does not stop. `fetch_rollout` resolves
# `:rollout` to a digest and verifies a signature over THAT DIGEST, then fetches
# by TAG — a second round trip a registry is free to answer differently. Here
# the tag serves a HALT while `resolve.rollout` and the `.sig` artifact still
# name the document that was signed. Without the manifest-digest check, a
# signature over one document would stand behind the bytes of another, and every
# machine on this channel would obey a halt nobody signed.
RB="$(fixture binding "$MID_SLOT_60" candidate 25 false '')"
RBSRC="$(fixture bindingsrc "$MID_SLOT_60" candidate 100 true 'THE SWAPPED DOCUMENT')"
rm -rf "$RB/registry/rollout"
cp -r "$RBSRC/registry/rollout" "$RB/registry/rollout"
# The fixture must really be the situation it claims: served bytes changed,
# resolve answer and signature untouched.
if cmp -s "$RB/registry/rollout/manifest.json" "$RBSRC/registry/rollout/manifest.json"; then
    ok "the swapped fixture serves the halt document"
else
    bad "the swap did not take"
fi
rc="$(run "$RB" channel rollout --json)"; both
[[ "$(jq_get verdict)" != '"halted"' ]] \
    && ok "a halt served under another document's signature is not obeyed" \
    || { bad "THE SWAP WAS OBEYED — a signature over one object backed another's bytes"; sed 's/^/       /' "$TMP/all" >&2; }
[[ "$(jq_get documentFrom)" == '"none"' ]] \
    && ok "and no document was used at all" \
    || bad "documentFrom is $(jq_get documentFrom)"
hasnt 'THE SWAPPED DOCUMENT' "$TMP/out" "nothing from the substituted document reaches the user"
has 'the signature that was checked is over' "$TMP/out" "and the machine names the digest mismatch rather than a generic parse error"

sec "channel set tells a tag that does not exist from a registry that did not answer"
# docs/update-channels.md promises both halves, and they are different facts. A
# tag the registry ANSWERED about and does not hold would strand the machine, so
# it is refused; a registry that could not be reached is the user's aeroplane,
# and refusing a configuration change over that would be this program deciding a
# laptop may not be configured offline.
#
# `channel set` needs euid 0, which `ops::require_root` reads from
# /proc/self/status. `unshare -r` supplies that without privilege, and
# --dry-run runs the whole check and then changes nothing.
if unshare -r true 2>/dev/null; then
    setrun() { APEX_TRUST_ROOT="$1" unshare -r "$APEX" "${@:2}" > "$TMP/out" 2> "$TMP/err"; echo $?; }

    RSET="$(fixture setcheck "$MID_SLOT_60" candidate 100 false '')"
    printf 'reading manifest beta in %s: manifest unknown\n' "$IMAGE" > "$RSET/registry/resolve.beta.error"
    rc="$(setrun "$RSET" channel set beta --dry-run)"; both
    [[ "$rc" != 0 ]] \
        && ok "a channel tag the registry does not serve is refused" \
        || { bad "channel set took a tag that does not resolve (exit $rc)"; sed 's/^/       /' "$TMP/all" >&2; }
    has 'does not resolve' "$TMP/all" "and the refusal says the tag does not resolve"
    has 'no update available' "$TMP/all" "and says what the machine would have seen instead"
    hasnt 'bootc switch' "$TMP/all" "and nothing was switched"

    # --force overrides it, which is the documented escape hatch.
    rc="$(setrun "$RSET" channel set beta --dry-run --force)"; both
    [[ "$rc" == 0 ]] \
        && ok "--force takes the tag anyway" \
        || { bad "--force still refused (exit $rc)"; sed 's/^/       /' "$TMP/all" >&2; }
    has 'because --force' "$TMP/all" "and says it was forced"

    # The other half: a registry that did not answer at all.
    printf 'dial tcp 140.82.114.34:443: connect: connection refused\n' > "$RSET/registry/resolve.beta.error"
    rc="$(setrun "$RSET" channel set beta --dry-run)"; both
    [[ "$rc" == 0 ]] \
        && ok "an unreachable registry does not block the switch" \
        || { bad "channel set refused because the network was down (exit $rc)"; sed 's/^/       /' "$TMP/all" >&2; }
    has 'the network, not the tag' "$TMP/all" "and says which of the two it was"
    hasnt 'Not switching' "$TMP/all" "and does not claim the tag is missing"
else
    bad "unshare -r is unavailable, so the channel set gate was not exercised"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
