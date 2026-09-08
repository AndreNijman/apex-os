# trust-enforcement
items: P1-047 criterion 2 (enforcement half); touches P1-046's tag-alias fact
repo: apex-os
worktree: /var/tmp/apex-work/wt-trust
branch: task/trust-enforcement

## NEXT
**The unit is done.** Nothing is outstanding on this card; see the report below
for which criteria this closes and which stay open. Five commits, all pushed to
`origin/task/trust-enforcement`. Base is `1ab542c`; **not rebased** — the
integration tip has moved to `678bb5d` and the orchestrator integrates.

If anything else is picked up here, the two things deliberately NOT done:
- `provenance` stays `warn` in the shipped default until an image build actually
  publishes a `.att`. Flipping it to `enforce` before then refuses every update
  on every machine.
- `/etc/containers/policy.json` is untouched. Tightening it to `sigstoreSigned`
  would make `skopeo copy` require signatures on the `.sig` artifacts
  themselves, and the gate would go `CouldNotRun` — noted in
  `docs/trust-enforcement.md`.

## DONE
- Worktree /var/tmp/apex-work/wt-trust off origin/roadmap/v2.2 @ 1ab542c; branch
  task/trust-enforcement pushed before any work.
- Read trust.rs (1232 lines), ops::update, channel::halt_reason, tests/test-apex-trust.sh
  (242 lines), P1-045/046/047 evidence.
- Proved end-to-end verification is possible with ONLY skopeo + openssl (spike, read-only
  against ghcr.io, see FOUND).
- **4572efb — the verifier.** `apexd/apex/src/verify.rs`, 1300 lines: a real
  sigstore verification on skopeo + openssl, no cosign. Five checks, all of
  which must hold (blob hash vs layer digest; ECDSA over the blob under the
  leaf's key; leaf chained to a PINNED Fulcio root with the signature's own
  `chain` supplied only as untrusted intermediates; SAN URI + OIDC-issuer
  extension against what the machine expects; payload's
  `docker-manifest-digest`/`docker-reference` against what is being deployed).
  `Verdict` = Verified/Absent/Failed/CouldNotRun; `Enforcement` =
  {signature,provenance} x {enforce,warn,off} from
  `/usr/share/apex-os/trust/enforcement.conf` then `/etc/apex/trust.conf`;
  `decide()` is the one pure decision function. trust.rs's registry half
  rewritten onto it: `Artifact::Present`, `claimed_signer` and `checked` are
  gone — one field for the answer, none for the claim — and `render` prints a
  signer name ONLY where one was verified.
- **180c01c — the gate, wired into `ops::update`.** Before `record_update` and
  before `FsyncGuard::disable` (both write machine state; a refusal after them
  would have recorded a health record for an update that never happened, which
  is what §26's rollout stop then reasons about, and left ostree's fsync off on
  a machine that is not updating) — asserted structurally, comment-stripped, at
  lines 12 vs 19 vs 26 vs 28. Verifies the digest `verify::resolve` gets for the
  ORIGIN REFERENCE, not `booted_digest`. Escape `--allow-unverified`, separate
  from `--force`, and it still prints the refusal it overrode. New read-only
  surface `apex trust --gate [--json]`: the same gate as a readout, calling the
  same functions in the same order, no root, stages nothing. Under
  `APEX_TRUST_ROOT` the gate returns before acting on any decision — stated as
  a program invariant, one condition checked first, which is what makes the new
  suite safe on a daily-driver laptop.
- **`tests/test-apex-trust-enforcement.sh` — 59 assertions, real cryptography.**
  Each fixture mints a P-256 root + intermediate + leaf, gives the leaf a
  Sigstore SAN and OIDC-issuer extension, and signs a genuine cosign
  simple-signing payload; the binary does the whole verification. No network,
  no bootc, no root.
- **0591e63 — two gate defects the advisor caught, both the forbidden class.**
  (1) `image_error: Some(_)` proceeded UNGATED: both callers unwrapped the
  origin reference with an early return, so an unreadable `/proc/cmdline` or
  origin file printed one line and DEPLOYED under `signature=enforce` — the
  EACCES class this repo swept fourteen readers for, one layer up, and worse
  because the consequence was deploying rather than mis-reporting. It was also
  inconsistent with the arm beside it: the same uncertainty was refused when it
  came from the network and waved through when it came from a file.
  `verify::gate` now takes `Result<Option<&str>, &str>` and every "nothing
  could be checked" path goes through `decide` instead of returning past it;
  only `Ok(None)` (genuinely not a container deployment) skips, with its own
  sentence. (2) `refusal()`'s body paragraph was still unconditional — it said
  in every refusal that "the image the registry is serving is not the one this
  machine was told to expect", contradicting its own headline three lines above
  whenever the verdict was `CouldNotRun`, and telling a user with a broken
  network they had been attacked.
- **7b6a205 — the shipped files and the docs.**
  `files/system/trust/fulcio-root.pem` + `enforcement.conf`, installed by
  **`Containerfile.base`** (measured: that is where the explicit `COPY
  files/… /usr/…` lines live; `Containerfile.core` creates
  `/usr/share/apex-os/secureboot` inline instead, and neither `.core` nor
  `.apex` has a generic `files/system/share` mapping). The build now FAILS if
  the pinned root's SHA-256 fingerprint changes, is not self-signed, or if the
  defaults do not parse — because under `signature=enforce` a root that is
  missing or wrong is `CouldNotRun`, which REFUSES, so a bad file here refuses
  every update on every machine. `docs/trust-enforcement.md` written and
  clean under `check-doc-verbs.sh` (2 valid, 0 not-a-command; the 4 BAD in the
  whole-docs run are pre-existing, in `p1-/p3-progress.md`, which that
  checker's own header excludes as historical records).
- **c14da15 — 41 verify tests + the 12-cell decision table, and two mutations.**
  Full suite **451 green** (445 unit + 6 mcp_sidecar_live), up from 404.

## COUNTS (real runs, at the final tip)
- `cargo test`: **455** — 449 unit + 6 `mcp_sidecar_live`. Was 404 at the
  branch point, so **+51**.
- `tests/test-apex-trust-enforcement.sh`: **77 passed, 0 failed** (new file),
  and run five times in a row at 53 to confirm it is deterministic across
  freshly minted keys.
- `tests/test-apex-trust.sh --with-binary`: **25 passed, 0 failed** — unchanged
  by the rewrite, which is the point.
- `tests/test-apex-verbs.sh`: 44 passed, 0 failed.
- `tests/test-apex-channel.sh`: 22 passed, 0 failed (§26's gate, unchanged).
- `tests/check-no-conflict-markers.sh`: PASS.
- `tests/check-doc-verbs.sh` on the new doc: 2 valid, 0 not a command.
- `tests/run-clippy.sh`: **PASS**, in the container. Three real lints were
  fixed to get there (`manual_pattern_char_comparison`, `collapsible_match`,
  and `too_many_arguments` on `verify_signed_bytes`, which became the
  `Candidate` + `Expect` structs).

## MUTATIONS PROVEN (round 6) — four, each restored and re-verified green
| # | mutation | where | what went red | restored |
|---|---|---|---|---|
| 1 | `(Verdict::Failed(_), _) => Err(...)` → `Ok(Some(...))` — a signature that verifies WRONGLY stops refusing | verify.rs `arm()` | **6 Rust tests**: `a_failed_signature_refuses_even_under_warn`, `a_failed_signature_under_enforce_refuses`, `a_refusal_carries_the_other_check_s_warning_too`, `a_refusal_names_which_of_the_two_checks_refused`, `a_refusal_that_could_not_run_is_never_worded_as_a_failure`, `the_json_gate_answer_matches_the_rendered_one` (439 pass / 6 fail) | yes |
| 2 | `Artifact::Unavailable(why) => CouldNotRun(why)` → `Absent(why)` — an unreachable registry reads as unsigned | verify.rs `from_artifact()` | **1 Rust test**: `an_unreachable_registry_is_never_refused_as_unsigned` (444 pass / 1 fail) | yes |
| 3 | `match resolve(roots, reference)` → `match crate::trust::booted_digest(roots)` — the gate judges what is RUNNING instead of what would be DEPLOYED | verify.rs `gate()` | **5 shell assertions** (54 pass / 5 fail): the two in the unresolvable-tag section and the three in the moved-tag section. Caught only 2 before the `moved` fixture was added, which is why that fixture exists — booted and resolved were equal in every other fixture, so the mutation was invisible. | yes |
| 4 | `openssl_verify_reason` back to `lines().next()` | verify.rs | **1 Rust test**: `a_chain_failure_reports_the_reason_and_not_a_distinguished_name` (445 pass / 1 fail) | yes |
| 5 | restore the early return on an unreadable origin — an origin file nobody could read is deployed ungated | verify.rs `gate()` | **2 Rust tests** (`an_origin_file_nobody_could_read_is_never_deployed_ungated`, `a_deployment_with_no_image_at_all_is_told_apart_from_one_nobody_could_read`; 447 pass / 2 fail) **+ 3 shell assertions**, decisively "an unreadable origin deployed ungated (exit 0)" (65 pass / 3 fail) | yes |

## FOUND
- **cosign is not packaged for Fedora 43.** `dnf5 repoquery cosign` / `cosign*` /
  `sigstore*` is EMPTY across fedora, updates, updates-archive, rpmfusion-{free,nonfree}
  (control: `skopeo` returns 3 hits). trust.rs's cryptographic arm keyed off
  `/usr/bin/cosign` existing, so on every APEX machine it was dead code and the report
  always said "not checked". Enforcement cannot be built on cosign without vendoring a
  ~100 MB Go binary into the image. **Fixed in 4572efb: it does not need cosign.**
- Verified the real booted digest by hand with skopeo + openssl only, on the L16,
  read-only:
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
    `openssl verify -attime <instant>`; at `now` it fails with "certificate has
    expired". A verifier that forgets `-attime` refuses every good image.
    `unix_from_iso("2026-09-05 13:43:25Z") == 1788615805`, cross-checked against
    `date -u -d … +%s`, and asserted in
    `a_certificate_start_date_becomes_the_instant_the_chain_is_checked_at`.
  - **Which instant is a security decision, and the bundle is the wrong answer.**
    `dev.sigstore.cosign/bundle`'s rekor `integratedTime` is the honest instant but
    NOTHING in this verification authenticates that annotation, so an attacker who
    can serve a manifest can put any number in it — which would let a signature made
    with a leaked ephemeral key long after its window verify anyway. The leaf's own
    `notBefore` is inside the certificate and the certificate is authenticated by the
    chain, so that is what 4572efb uses. The cost is transparency-log inclusion, which
    is NOT checked; the report says so in those words (same posture as `cosign verify
    --insecure-ignore-tlog`) rather than printing an unqualified "verified".
  - Pinning the root is safe: the Fulcio root and intermediate fetched independently from
    sigstore/root-signing match the ones the signature carried, SHA-256 fingerprint for
    fingerprint (root 3B:A7:B6:CC:4E:95:46:9D:…:80:C1, intermediate 15:D7:95:34:…:FE:47).
    Taking the root out of the signature would be circular; shipping that pinned pair is not.
    **Commit 4 must record the FULL fingerprint — an elided one is not a check anyone
    can repeat.**
  - The identity is in the leaf's SAN URI and the OIDC issuer in extension
    1.3.6.1.4.1.57264.1.1 / .8 — both present and both equal to trust.rs's
    EXPECTED_SIGNER / EXPECTED_ISSUER.
- **openssl's rendering of the issuer extension, measured not assumed.** Minted two
  certificates to capture it: `openssl x509 -noout -text` prints
  `            1.3.6.1.4.1.57264.1.8: ` **with a trailing space**, value indented on
  the NEXT line, and the `.1.8` form wraps the string in a DER UTF8String which
  renders as `.+https://token…` — two bytes of tag and length in front. The `.1.1`
  form is the bare string. Both are parsed; the wrapper is peeled by taking the tail,
  never by searching the line for the expected value, or
  `https://evil.example/?x=<expected>` would match. Both renderings are in
  `the_oidc_issuer_is_read_out_of_either_sigstore_extension`, and the attack in
  `an_issuer_that_merely_contains_the_expected_one_is_not_the_expected_one`.
- **No published APEX image has an SBOM attestation.** The `cosign attest` step exists
  only on `roadmap/v2.2`; `origin/main`'s `build-image.yml` has no `syft` and no
  `attest`, and `.att` is `manifest unknown` for the booted digest. So the shipped
  default is `provenance=warn`: `enforce` would refuse every update on every machine
  because the publisher has not caught up, which is an outage dressed as a security
  control. It also means the attestation path has never met a real artifact, so an
  envelope the code cannot PARSE is `CouldNotRun`, never `Failed` — a format that
  differs from the one assumed must not refuse the first genuinely signed image.
- `/etc/containers/policy.json` on the L16 is still the Fedora default: one
  `insecureAcceptAnything`. Nothing on the machine requires a signature for anything.
- **`refusal()` had two defects, both fixed.** Its headline said "does not verify" for
  every refusal including `CouldNotRun` — printing an accusation of tampering when the
  machine merely could not reach a registry, which is exactly the four-valued
  verdict's whole point discarded at the one place a user reads it. And it pointed at
  `man apex-trust`, which does not exist: **this repository has no man pages at all**
  (`find . -name '*.1'` is empty), so that pointer is dropped and
  `docs/trust-enforcement.md` gets written in commit 4.
- No consumer outside Rust reads the renamed JSON keys: `claimedSigner`,
  `signatureChecked`, `signatureCheckedNote`, `sbomAttestation` appear in no `.sh`,
  `.qml`, `.md`, `.py` or `.js` in the repo, so the rename breaks nothing.

## BLOCKED ON
- nothing
