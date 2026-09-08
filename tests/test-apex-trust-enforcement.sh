#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-trust-enforcement.sh — roadmap §27's enforcement half, proven
#  against real cryptography and never against a real deployment.
#
#  `test-apex-trust.sh` guards the READOUT: that `apex trust` describes the
#  machine's trust state without lying about it. This file guards the GATE: that
#  `apex update` refuses an image whose signature does not verify, permits one
#  it merely could not check, and tells the difference in the words it prints.
#
#  ── Why a second file ───────────────────────────────────────────────────────
#  Every assertion here needs a minted certificate authority and a signature
#  made with it, so the suite is slower and needs openssl. The readout suite is
#  in pr-validation's `static` job and must stay fast; this one is the crypto.
#
#  ── What is deliberately NOT done ──────────────────────────────────────────
#  No image is staged, no deployment is rolled back, and `bootc` is never
#  spawned. That is not restraint on the test's part — it is an invariant of
#  the program: under `APEX_TRUST_ROOT`, `apex update` prints its decision and
#  returns, because every trust fact in play is a file somebody wrote for a
#  test and this program does not deploy on fixture facts. That invariant is
#  what makes all three decisions exercisable through the real binary.
#
#  ── The cryptography is real ────────────────────────────────────────────────
#  Not a stub and not a mock. Each fixture mints a P-256 root, an intermediate
#  signed by it and a leaf signed by that, gives the leaf a Sigstore subject
#  alternative name and OIDC-issuer extension, and signs a genuine cosign
#  simple-signing payload with `openssl dgst -sha256 -sign`. The binary then
#  does the whole verification: blob hash, ECDSA, chain to the pinned root,
#  identity, and the payload's binding to the digest. A test that faked any of
#  that would prove the fake worked.
#
#  Usage: tests/test-apex-trust-enforcement.sh
#         APEX=/path/to/apex tests/test-apex-trust-enforcement.sh
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

APEX="${APEX:-$REPO/apexd/target/debug/apex}"
[[ -x "$APEX" ]] || APEX="${CARGO_TARGET_DIR:-}/debug/apex"
if [[ ! -x "$APEX" ]]; then
    echo "FATAL: no apex binary. Build it, or set APEX=/path/to/apex." >&2
    echo "       A skipped assertion reports as a pass, which is the bug this refuses." >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'chmod -R u+rwX "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

# The identity and issuer the fixtures sign under. Not the production ones —
# the fixture's own, supplied to the binary through the two image-owned
# override files, because a suite that had to expect the real GitHub identity
# could not mint a certificate for it and would have to skip the identity
# check. That check is one of the two arms that make this verification mean
# anything.
SIGNER='https://github.com/AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/main'
ISSUER='https://token.actions.githubusercontent.com'
# The digest the fixture registry serves for the origin's tag. Any 64 hex
# digits; what matters is that the signed payload binds THIS one.
DIGEST='sha256:daf8c8eb2928ab995a67ea9df43aa78116f638278bd0e7d32135a8b272e4ebec'

# ── one certificate authority, minted once ───────────────────────────────────
CA="$TMP/ca"; mkdir -p "$CA"
# The authority is minted with a notBefore a year in the PAST, which is what a
# real Fulcio root has. Getting this wrong is instructive rather than obvious:
# with a root created "now", the chain check at `-attime <leaf notBefore>`
# fails with `certificate is not yet valid` at depth 2 — the CA is not yet
# valid at the instant its own leaf was issued — and the fixture would look
# like a verifier bug.
CA_NB="$(date -u -d '1 year ago' +%Y%m%d%H%M%SZ)"
CA_NA="$(date -u -d '9 years' +%Y%m%d%H%M%SZ)"
mint_ca() {
    openssl ecparam -name prime256v1 -genkey -noout -out "$CA/root.key" 2>/dev/null
    openssl req -x509 -new -key "$CA/root.key" -sha256 \
        -not_before "$CA_NB" -not_after "$CA_NA" \
        -subj '/O=apex test/CN=apex test root' -out "$CA/root.pem" \
        -addext 'basicConstraints=critical,CA:TRUE' \
        -addext 'keyUsage=critical,keyCertSign,cRLSign' 2>/dev/null
    openssl ecparam -name prime256v1 -genkey -noout -out "$CA/int.key" 2>/dev/null
    openssl req -new -key "$CA/int.key" -subj '/O=apex test/CN=apex test intermediate' \
        -out "$CA/int.csr" 2>/dev/null
    printf 'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n' > "$CA/int.ext"
    openssl x509 -req -in "$CA/int.csr" -CA "$CA/root.pem" -CAkey "$CA/root.key" \
        -sha256 -not_before "$CA_NB" -not_after "$CA_NA" \
        -extfile "$CA/int.ext" -out "$CA/int.pem" 2>/dev/null
}
mint_ca

# mint_leaf <outdir> <san-uri> <issuer> [expired]
#
# `expired` mints the ten-minute window Fulcio actually issues, placed in the
# past, which is the state EVERY real APEX signature is in by the time any
# machine reads it. A verifier that forgets `openssl verify -attime` refuses
# every good image, so the default fixture leaf is the expired one and the
# suite would go red the moment `-attime` was dropped.
NOT_BEFORE_OK=0
openssl x509 -help 2>&1 | grep -q -- '-not_before' && NOT_BEFORE_OK=1
mint_leaf() {
    local out="$1" san="$2" iss="$3" expired="${4:-}"
    openssl ecparam -name prime256v1 -genkey -noout -out "$out/leaf.key" 2>/dev/null
    openssl req -new -key "$out/leaf.key" -subj '/CN=apex test leaf' -out "$out/leaf.csr" 2>/dev/null
    printf 'subjectAltName=critical,URI:%s\n1.3.6.1.4.1.57264.1.8=ASN1:UTF8String:%s\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=codeSigning\n' \
        "$san" "$iss" > "$out/leaf.ext"
    local window=()
    if [[ -n "$expired" && "$NOT_BEFORE_OK" -eq 1 ]]; then
        window=(-not_before "$(date -u -d '2 hours ago' +%Y%m%d%H%M%SZ)"
                -not_after  "$(date -u -d '110 minutes ago' +%Y%m%d%H%M%SZ)")
    else
        window=(-days 1)
    fi
    openssl x509 -req -in "$out/leaf.csr" -CA "$CA/int.pem" -CAkey "$CA/int.key" \
        -sha256 "${window[@]}" -extfile "$out/leaf.ext" -out "$out/leaf.pem" 2>/dev/null
}

# crypto_fixture <name> <trust.conf body> [<tamper>] [<san>] [<expired>]
#
# tamper: "" none | "sig" flip the signature | "payload" sign a different
# digest | "transport" replace the artifact with a registry transport error |
# "missing" no artifact at all (the registry answered and holds nothing)
crypto_fixture() {
    local name="$1" conf="$2" tamper="${3:-}" san="${4:-$SIGNER}" expired="${5:-expired}"
    local root="$TMP/$name"
    local csum=f3f505fc39fb268c59f4458365c96b764a7bd7d30f2f51e98bb6a009666b7852
    local bootcsum=1d98b51dd76621b656c50e4f22dc7e5eade9b0f869443a3efa90eee08eb9373e

    mkdir -p "$root/proc" "$root/etc/containers" "$root/etc/apex" \
             "$root/usr/share/apex-os/trust" "$root/registry" \
             "$root/ostree/boot.0/default/$bootcsum" \
             "$root/ostree/deploy/default/deploy/$csum.0" "$root/work"
    printf 'root=UUID=x rw quiet ostree=/ostree/boot.0/default/%s/0\n' "$bootcsum" > "$root/proc/cmdline"
    ln -sfn "../../../deploy/default/deploy/$csum.0" "$root/ostree/boot.0/default/$bootcsum/0"
    printf '[origin]\ncontainer-image-reference=ostree-unverified-registry:ghcr.io/andrenijman/apex-os:daily\n' \
        > "$root/ostree/deploy/default/deploy/$csum.0.origin"
    printf '{"default":[{"type":"insecureAcceptAnything"}]}\n' > "$root/etc/containers/policy.json"
    # What `rpm-ostree status --json` would say. `apex trust --verify` reads
    # the BOOTED digest from here; the gate reads registry/resolve instead.
    # They are the same value in this fixture only so that `--verify` and
    # `--gate` can be compared; on a real machine they differ constantly.
    printf '{"deployments":[{"booted":true,"base-commit-meta":{"ostree.manifest-digest":"%s"}}]}\n' \
        "$DIGEST" > "$root/rpm-ostree-status.json"

    # What the machine pins and expects. The root here is the fixture's own
    # authority: pinning is the whole point, and a verification that trusted
    # the chain the signature handed it would prove nothing.
    cp "$CA/root.pem" "$root/usr/share/apex-os/trust/fulcio-root.pem"
    printf '%s\n' "$SIGNER" > "$root/usr/share/apex-os/trust/expected-signer"
    printf '%s\n' "$ISSUER" > "$root/usr/share/apex-os/trust/expected-issuer"
    printf '%s\n' "$conf" > "$root/etc/apex/trust.conf"
    # What `:daily` currently resolves to. The gate verifies THIS, not the
    # booted digest — the four APEX tags alias one digest that moves on every
    # main build, so they are routinely different.
    printf '%s\n' "$DIGEST" > "$root/registry/resolve"

    local tag="${DIGEST/:/-}.sig"
    if [[ "$tamper" == "missing" ]]; then printf '%s' "$root"; return; fi
    if [[ "$tamper" == "transport" ]]; then
        printf 'dial tcp: lookup ghcr.io: no such host\n' > "$root/registry/$tag.error"
        printf '%s' "$root"; return
    fi

    local art="$root/registry/$tag"; mkdir -p "$art"
    mint_leaf "$root/work" "$san" "$ISSUER" "$expired"

    # A genuine cosign simple-signing payload.
    local bound="$DIGEST"
    [[ "$tamper" == "payload" ]] && bound="sha256:$(printf 'a%.0s' $(seq 64))"
    printf '{"critical":{"identity":{"docker-reference":"ghcr.io/andrenijman/apex-os"},"image":{"docker-manifest-digest":"%s"},"type":"cosign container image signature"},"optional":null}' \
        "$bound" > "$root/work/payload.json"
    openssl dgst -sha256 -sign "$root/work/leaf.key" \
        -out "$root/work/sig.der" "$root/work/payload.json" 2>/dev/null
    if [[ "$tamper" == "sig" ]]; then
        # A byte FLIPPED inside the DER, not one appended to it. An ECDSA
        # P-256 signature is at most 72 DER bytes, so appending changes the
        # length as well as the content and openssl can then reject it for
        # being unparseable rather than for being wrong — a fixture failing
        # for a reason it does not claim. Flipping the last byte corrupts `s`
        # and leaves a well-formed DER.
        python3 -c '
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
b[-1] ^= 0xff
open(p, "wb").write(b)' "$root/work/sig.der" \
          || { echo "FATAL: could not corrupt the $name fixture signature" >&2; exit 1; }
    fi

    # A fixture that fails to break what it claims to break makes a test that
    # passes for the wrong reason, and this repository has recorded that class
    # of bug three times. So the fixture checks its own tamper, with openssl,
    # before the binary is ever asked about it.
    openssl x509 -pubkey -noout -in "$root/work/leaf.pem" > "$root/work/pub.pem" 2>/dev/null
    sig_verifies() {
        openssl dgst -sha256 -verify "$root/work/pub.pem" \
            -signature "$root/work/sig.der" "$root/work/payload.json" >/dev/null 2>&1
    }
    case "$tamper" in
      sig)
        if sig_verifies; then
            echo "FATAL: the '$name' fixture's signature still verifies ($(wc -c < "$root/work/sig.der") DER bytes); the tamper did not take" >&2
            exit 1
        fi ;;
      payload)
        # Here the signature is GENUINE — that is the point. What must be
        # wrong is only which digest it covers.
        if ! sig_verifies; then
            echo "FATAL: the '$name' fixture's signature does not verify; it must be valid over the wrong digest" >&2
            exit 1
        fi
        grep -q "$DIGEST" "$root/work/payload.json" \
          && { echo "FATAL: the '$name' fixture signs the digest it is supposed to differ from" >&2; exit 1; } ;;
      "")
        if ! sig_verifies; then
            echo "FATAL: the '$name' fixture's own signature does not verify; the suite would prove nothing" >&2
            exit 1
        fi ;;
    esac

    # `skopeo copy … dir:` names each blob by its bare hex digest — measured
    # against the real ghcr.io signature artifact, not assumed — so the
    # fixture uses the same layout and the same code path reads it.
    local hex; hex="$(openssl dgst -sha256 -r "$root/work/payload.json" | awk '{print $1}')"
    cp "$root/work/payload.json" "$art/$hex"
    SIG_B64="$(base64 -w0 < "$root/work/sig.der")" \
    LEAF="$(cat "$root/work/leaf.pem")" CHAIN="$(cat "$CA/int.pem")" \
    HEX="$hex" SIZE="$(wc -c < "$root/work/payload.json")" \
    OUT="$art/manifest.json" \
    python3 -c '
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
}, open(os.environ["OUT"], "w"))' \
      || { echo "FATAL: could not write the fixture manifest" >&2; exit 1; }
    printf '%s' "$root"
}

run() { APEX_TRUST_ROOT="$1" "$APEX" "${@:2}" > "$TMP/out" 2> "$TMP/err"; echo $?; }
both() { cat "$TMP/out" "$TMP/err" > "$TMP/all"; }

# The gate is driven through `apex trust --gate` rather than `apex update`.
#
# Not a convenience. `apex update` refuses to run without root — correctly,
# because bootc writes to /ostree and /boot — so a suite that drove it would
# have to run under sudo, on the author's daily-driver laptop, against a
# program whose next step is staging an image. One guarded `if` would be all
# that stood between a test run and a real deployment, and no assertion is
# worth that. `--gate` calls the same `verify::gate`, `verify::decide` and
# `verify::refusal`, in the same order, and prints the refusal from the same
# producer. That `apex update` calls them too, before anything that writes
# machine state, is asserted on the source at the end of this file.
gate() { run "$1" trust --gate "${@:2}"; }

# ═════════════════════════════════════════════════════════════════════════════
sec "a real signature, verified by the binary with skopeo and openssl only"
# The claim the whole unit rests on. cosign is not packaged for Fedora — `dnf5
# repoquery cosign 'cosign*' 'sigstore*'` is empty across every configured
# repository — so if this needed cosign it could never run on an APEX machine.
R="$(crypto_fixture good 'signature=enforce')"
rc="$(run "$R" trust --verify)"
both
has 'verified — signed by' "$TMP/all" "the binary verifies a genuine signature end to end"
has "$SIGNER" "$TMP/all" "and names the identity it verified"
has 'transparency log was not checked' "$TMP/all" "it does not overclaim: the rekor entry is not checked"
[[ "$rc" == 0 ]] && ok "apex trust --verify exits 0 on a verified image" \
    || { bad "apex trust --verify exited $rc on a good signature"; sed 's/^/       /' "$TMP/all" >&2; }
[[ ! -x /usr/bin/cosign ]] && ok "and cosign is not installed, so it cannot have been used" \
    || printf '  note  cosign IS present on this machine; the no-cosign claim is untested here\n'

sec "the ten-minute Fulcio window, which every real signature is already past"
# A Fulcio leaf lives ten MINUTES. Measured on the live ghcr.io signature for
# the digest `:daily` serves: notBefore 2026-09-07 12:33:22, notAfter 12:43:22.
# `openssl verify` at the current time says "certificate has expired" for a
# perfectly good signature, so the chain must be checked at a past instant —
# the leaf's own notBefore, which the chain authenticates, and NOT the bundle's
# rekor integratedTime, which nothing here authenticates.
if [[ "$NOT_BEFORE_OK" -eq 1 ]]; then
    exp_nb="$(openssl x509 -noout -startdate -dateopt iso_8601 -in "$R/work/leaf.pem")"
    exp_na="$(openssl x509 -noout -enddate -dateopt iso_8601 -in "$R/work/leaf.pem")"
    if openssl verify -CAfile "$CA/root.pem" -untrusted "$CA/int.pem" \
         "$R/work/leaf.pem" >/dev/null 2>&1; then
        bad "the fixture leaf is not expired ($exp_nb..$exp_na); this suite would pass without -attime"
    else
        ok "the fixture leaf IS expired now ($exp_nb .. $exp_na), as every real one is"
        # ...and the verification above still succeeded, which it can only have
        # done by verifying the chain at a past instant.
        has 'verified — signed by' "$TMP/all" "so the verified result above proves -attime is used"
    fi
else
    bad "this openssl has no -not_before, so the expiry trap could not be exercised"
fi

sec "a tampered signature is a FAILURE, and is worded as one"
R="$(crypto_fixture tampered 'signature=enforce' sig)"
rc="$(gate "$R")"
both
has 'does not verify' "$TMP/all" "apex update refuses an image whose signature does not verify"
has 'this update is being held' "$TMP/all" "and says the update is held"
hasnt 'could not be verified' "$TMP/all" "a tampered signature is not reported as unverifiable"
[[ "$rc" == 1 ]] && ok "apex update exits 1" || bad "apex update exited $rc on a bad signature"
hasnt 'update finished' "$TMP/all" "the update never got as far as running"

sec "a signature over a DIFFERENT digest does not count as one over this one"
# The substitution a moved tag makes easy, and the reason the payload's
# bindings are checked at all: the ECDSA verification alone proves only that
# the expected identity signed something.
R="$(crypto_fixture othersig 'signature=enforce' payload)"
rc="$(gate "$R")"
both
has 'does not verify' "$TMP/all" "a valid signature over another image is refused"
has "$DIGEST" "$TMP/all" "and the refusal names the digest being deployed"
[[ "$rc" == 1 ]] && ok "apex update exits 1" || bad "apex update exited $rc"

sec "a signature from an identity this machine does not expect is refused"
R="$(crypto_fixture wrongsan 'signature=enforce' '' 'https://github.com/attacker/apex-os/.github/workflows/build-image.yml@refs/heads/main')"
rc="$(gate "$R")"
both
has 'attacker/apex-os' "$TMP/all" "the refusal names who actually signed"
has 'AndreNijman/apex-os' "$TMP/all" "and who this machine expected"
[[ "$rc" == 1 ]] && ok "apex update exits 1" || bad "apex update exited $rc"

sec "an unreachable registry is never refused as unsigned"
# The rule this unit must not break. `CouldNotRun` is neither a pass nor a
# failure, and under `warn` an offline machine must still be able to update —
# a gate that failed closed here would refuse the update that fixes a machine
# whose only problem was its network.
R="$(crypto_fixture offline 'signature=warn' transport)"
rc="$(gate "$R")"
both
has 'could not be checked' "$TMP/all" "the reason is that it could not be checked"
has 'no such host' "$TMP/all" "and the transport error is carried verbatim"
hasnt 'has no signature' "$TMP/all" "an unreachable registry is NOT reported as unsigned"
hasnt 'does not verify' "$TMP/all" "nor as a signature that failed"
[[ "$rc" == 0 ]] && ok "under warn, an offline machine still updates" \
    || bad "apex update exited $rc offline under warn; a flaky network must not stop updates"

R="$(crypto_fixture offlinestrict 'signature=enforce' transport)"
rc="$(gate "$R")"
both
has 'could not be verified' "$TMP/all" "under enforce it is refused, and worded as unverifiABLE"
hasnt 'does not verify.' "$TMP/all" "still not worded as a failure"
[[ "$rc" == 1 ]] && ok "and exits 1" || bad "apex update exited $rc under enforce"

sec "a registry that answers and holds nothing IS unsigned"
# The other half of the distinction: the registry replied, and nobody signed
# this digest. That is the substituted-image signal.
R="$(crypto_fixture unsigned 'signature=enforce' missing)"
rc="$(gate "$R")"
both
has 'has no signature' "$TMP/all" "an image nobody signed is refused as unsigned"
hasnt 'could not be checked' "$TMP/all" "and not as a check that failed to run"
[[ "$rc" == 1 ]] && ok "apex update exits 1" || bad "apex update exited $rc"

R="$(crypto_fixture unsignedwarn 'signature=warn' missing)"
rc="$(gate "$R")"
both
has 'has no signature' "$TMP/all" "under warn the same image warns"
[[ "$rc" == 0 ]] && ok "and updates" || bad "apex update exited $rc under warn"

sec "off means off, and enforce is what ships"
R="$(crypto_fixture switchedoff 'signature=off
provenance=off' sig)"
rc="$(gate "$R")"
both
has 'switched off' "$TMP/all" "a user who switched checking off is told it was switched off"
[[ "$rc" == 0 ]] && ok "and gets the update" || bad "apex update exited $rc with checking off"
# The shipped default, with no /etc override at all.
R="$(crypto_fixture default '' sig)"
rc="$(gate "$R")"
both
[[ "$rc" == 1 ]] && ok "with no configuration at all, a bad signature is still refused" \
    || bad "the built-in default let a tampered image through (exit $rc)"

sec "a typo in trust.conf never silently relaxes a check"
R="$(crypto_fixture typo 'signature = of' sig)"
rc="$(gate "$R")"
both
has 'is not one of enforce, warn, off' "$TMP/all" "the unusable value is named"
has 'trust.conf:1' "$TMP/all" "with its file and line"
[[ "$rc" == 1 ]] && ok "and the check stayed at enforce" \
    || bad "a misspelled value switched signature checking off (exit $rc)"

sec "the two escapes are separate, and --force does not skip the signature gate"
# §26's rollout stop and §27's signature gate answer different questions, and
# sharing one flag would mean anybody working around a machine-health stop
# silently stopped checking who signed their next image. Asserted on the
# source, because the behaviour lives in `apex update`, which this suite does
# not run — see the note on `gate` above.
OPSRS="$REPO/apexd/apex/src/ops.rs"
MAINRS="$REPO/apexd/apex/src/main.rs"
guard="$(sed -n '/^pub fn update(opts: UpdateOptions)/,/^}/p' "$OPSRS" \
    | grep -v '^[[:space:]]*//' | grep -B3 'trust_gate(')"
if printf '%s' "$guard" | grep -q 'opts.force'; then
    bad "the trust gate is guarded on opts.force — escaping §26's health stop would skip signature checking"
else
    ok "the trust gate is not guarded on --force"
fi
if printf '%s' "$guard" | grep -q 'trust_gate(opts.allow_unverified)'; then
    ok "it takes its own escape, --allow-unverified"
else
    bad "the trust gate does not take opts.allow_unverified"
fi
if grep -q 'allow_unverified: bool' "$OPSRS" && grep -q 'allow_unverified: args.allow_unverified' "$MAINRS"; then
    ok "and that flag is threaded from the CLI through to the gate"
else
    bad "--allow-unverified is not wired from the CLI through to UpdateOptions"
fi
# An escape hatch that also hides the reason is a way to not find out what was
# wrong with the image you just deployed.
if sed -n '/^fn trust_gate/,/^}/p' "$OPSRS" | grep -q 'proceeding anyway'; then
    ok "--allow-unverified still prints the refusal it overrode"
else
    bad "--allow-unverified suppresses the refusal instead of overriding it"
fi
R="$(crypto_fixture escapenamed 'signature=enforce' sig)"
gate "$R" >/dev/null; both
has '--allow-unverified' "$TMP/all" "the refusal names the flag that permits it"
hasnt 'apex update --force' "$TMP/all" "and does not offer --force, which would not work"

sec "the gate verifies what would be DEPLOYED, not what is booted"
# The load-bearing difference between this unit and the readout that preceded
# it. `apex`, `daily`, `gaming-mesa` and `gaming-nvidia` are four aliases for
# ONE digest that moves on every successful main build, so every APEX machine
# is effectively on edge: measured live, `:daily` served
# sha256:daf8c8eb… while the author's L16 was booted on sha256:308127d9…. A
# gate that verified the booted digest would wave an unsigned image through
# every time, while printing "verified".
R="$(crypto_fixture resolved 'signature=enforce')"
gate "$R" >/dev/null; both
has "$DIGEST" "$TMP/all" "the digest under judgement is the one the registry would serve"
hasnt 'f3f505fc' "$TMP/all" "and not the booted deployment's checksum"
# Prove it reads the resolver rather than guessing: break only the resolver.
R="$(crypto_fixture noresolve 'signature=enforce')"
printf 'ghcr.io/andrenijman/apex-os:daily: manifest unknown\n' > "$R/registry/resolve.error"
rm -f "$R/registry/resolve"
rc="$(gate "$R")"
both
has 'would not say which digest' "$TMP/all" "a tag it cannot resolve is reported as such"
hasnt 'has no signature' "$TMP/all" "and is not reported as an unsigned image"
[[ "$rc" == 1 ]] && ok "under enforce, an unresolvable tag is refused" || bad "exit $rc"

sec "this program never deploys on fixture facts"
# Not the suite being careful — an invariant of the program. Every trust fact
# under a fixture root is a file somebody wrote for a test, so `bootc` is not
# spawned on the strength of them in either direction, whatever the decision.
# It is a single condition, checked before anything else in the gate, and it
# is what makes a suite like this one safe to run on a daily-driver laptop.
tg="$(sed -n '/^fn trust_gate/,/^}/p' "$REPO/apexd/apex/src/ops.rs")"
if printf '%s' "$tg" | grep -q 'roots.fixture.is_some()'; then
    ok "trust_gate returns on a fixture root before any decision is acted on"
else
    bad "trust_gate does not special-case a fixture root — a test run could stage an image"
fi
# And it returns unconditionally from that branch: a fall-through would reach
# `bootc upgrade` on any fixture whose facts happened to verify.
fx="$(printf '%s\n' "$tg" | sed -n '/roots.fixture.is_some()/,/^    }/p')"
if printf '%s' "$fx" | grep -qE '^\s+return Some\('; then
    ok "and that branch returns unconditionally, so no fixture can fall through to bootc"
else
    bad "the fixture branch can fall through — a verifying fixture would reach bootc upgrade"
fi

sec "when the tag has moved, the gate judges the new image and --verify the old"
# The normal state of every APEX machine, not an edge case. The four tags moved
# to a new digest on the last main build and this machine is still running the
# previous one, so the two questions have two different answers — and only one
# of them can be enforced. Measured live while writing this: `:daily` served
# sha256:daf8c8eb… while the author's L16 was booted on sha256:308127d9….
R="$(crypto_fixture moved 'signature=enforce')"
BOOTED='sha256:308127d9cefeada90b1cb47b8f9c1cf6e8bd8f13ae5f3b0e2d7f4a6c8e1b3d5f'
printf '{"deployments":[{"booted":true,"base-commit-meta":{"ostree.manifest-digest":"%s"}}]}\n' \
    "$BOOTED" > "$R/rpm-ostree-status.json"
gate "$R" >/dev/null; both
has "$DIGEST" "$TMP/all" "the gate judges the digest the registry would serve"
hasnt "$BOOTED" "$TMP/all" "and never judges the digest this machine is booted on"
has 'verified — signed by' "$TMP/all" "which is signed, so the update is permitted"
# `apex trust --verify` asks the other question, and gets the other answer.
run "$R" trust --verify >/dev/null; both
has "$BOOTED" "$TMP/all" "--verify reports on the booted digest"
has 'none published' "$TMP/all" "for which this fixture's registry holds nothing"
hasnt 'verified — signed by' "$TMP/all" "so it does not claim the booted image was verified"

# The gate must precede everything in `update` that writes machine state. A
# refusal firing after `record_update` would have written a health record for
# an update that never happened — which is exactly what §26's rollout stop
# then reasons about — and one firing after `FsyncGuard::disable` would leave
# ostree's per-object fsync switched off on a machine that is not updating.
#
# Comments are stripped first. The block above the call names all three of
# these in prose, and a check that matched prose would keep passing on a file
# where the call itself had been moved.
body="$(sed -n '/^pub fn update(opts: UpdateOptions)/,/^}/p' "$REPO/apexd/apex/src/ops.rs" \
    | grep -v '^[[:space:]]*//')"
# Only the part after the `--check` branch returns: that branch legitimately
# runs `bootc upgrade --check` first, and it stages nothing.
body="$(printf '%s\n' "$body" | sed -n '/return worst;/,$p')"
g="$(printf '%s\n' "$body" | grep -n 'trust_gate(' | head -1 | cut -d: -f1)"
for after in 'record_update' 'FsyncGuard::disable' '"upgrade"'; do
    w="$(printf '%s\n' "$body" | grep -nF "$after" | head -1 | cut -d: -f1)"
    if [[ -n "$g" && -n "$w" && "$g" -lt "$w" ]]; then
        ok "the gate runs before $after (line $g vs $w)"
    else
        bad "the gate does not precede $after (line ${g:-none} vs ${w:-none}) — a refusal would fire after machine state was written"
    fi
done

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
