# trust-enforcement
items: P1-047 criterion 2 (enforcement half); touches P1-046's tag-alias fact
repo: apex-os
worktree: /var/tmp/apex-work/wt-trust
branch: task/trust-enforcement

## NEXT
Design the gate: trust::Verdict (Verified/Failed/CouldNotRun) x {signature, provenance},
a pure decision fn, and a real sigstore verifier built on skopeo+openssl (both already
in the image). Then wire it into ops::update before `bootc upgrade`.

## DONE
- Worktree /var/tmp/apex-work/wt-trust off origin/roadmap/v2.2 @ 1ab542c; branch
  task/trust-enforcement pushed before any work.
- Read trust.rs (1232 lines), ops::update, channel::halt_reason, tests/test-apex-trust.sh
  (242 lines), P1-045/046/047 evidence.
- Proved end-to-end verification is possible with ONLY skopeo + openssl (spike, read-only
  against ghcr.io, see FOUND).

## IN PROGRESS
- Nothing written yet.

## FOUND
- **cosign is not packaged for Fedora 43.** `dnf5 repoquery cosign` / `cosign*` /
  `sigstore*` is EMPTY across fedora, updates, updates-archive, rpmfusion-{free,nonfree}
  (control: `skopeo` returns 3 hits). trust.rs's cryptographic arm keys off
  `/usr/bin/cosign` existing, so on every APEX machine it is dead code and the report
  always says "not checked". Enforcement cannot be built on cosign without vendoring a
  ~100 MB Go binary into the image.
- **It does not need cosign.** Verified the real booted digest by hand with skopeo +
  openssl only, on the L16, read-only:
  - `skopeo inspect --raw docker://ghcr.io/andrenijman/apex-os:sha256-<hex>.sig` returns
    the cosign manifest; layers[0].annotations carry
    `dev.cosignproject.cosign/signature` (base64 DER ECDSA — note the
    **cosignproject** key, not `dev.sigstore.cosign/`; trust.rs only ever read the
    certificate key), `dev.sigstore.cosign/certificate` (leaf PEM),
    `dev.sigstore.cosign/chain`, `dev.sigstore.cosign/bundle` (rekor SET,
    integratedTime, logIndex).
  - `skopeo copy … dir:` yields the 243-byte simple-signing payload, which binds
    `docker-manifest-digest: sha256:308127d9…` == the L16's booted digest, and
    `docker-reference: ghcr.io/andrenijman/apex-os`.
  - `openssl dgst -sha256 -verify <leaf pubkey> -signature sig.der payload` -> **Verified
    OK**; one prepended byte -> `Verification failure`. Real crypto, no cosign.
  - The Fulcio leaf is **10 minutes long-lived** (notBefore 2026-09-05 13:43:25,
    notAfter 13:53:25) and is therefore already expired: chain verification MUST use
    `openssl verify -attime <bundle integratedTime>`; at `now` it fails with
    "certificate has expired". A verifier that forgets `-attime` refuses every good image.
  - Pinning the root is safe: the Fulcio root and intermediate fetched independently from
    sigstore/root-signing match the ones the signature carried, SHA-256 fingerprint for
    fingerprint (root 3B:A7:B6:CC:4E:95:46:9D:…:80:C1, intermediate 15:D7:95:34:…:FE:47).
    Taking the root out of the signature would be circular; shipping that pinned pair is not.
  - The identity is in the leaf's SAN URI and the OIDC issuer in extension
    1.3.6.1.4.1.57264.1.1 / .8 — both present and both equal to trust.rs's
    EXPECTED_SIGNER / EXPECTED_ISSUER.
- `/etc/containers/policy.json` on the L16 is still the Fedora default: one
  `insecureAcceptAnything`. Nothing on the machine requires a signature for anything.

## BLOCKED ON
- nothing
