# The signature gate: what APEX refuses to deploy, and why it might refuse yours

Roadmap §27, the enforcement half.

## What changed

CI has cosign-signed every APEX image it ever published, under a keyless GitHub
identity, and it verifies its own signature before it moves a tag. Until the
gate, none of that reached your machine. `apex trust` (the readout half) could
tell you nobody had checked, and `apex update` then deployed the image anyway.

Now `apex update` checks the signature of the image it is about to deploy, and
refuses to deploy one it cannot verify.

## Seeing what your machine will do, before it does it

```bash
apex trust --gate
```

It needs no root, writes nothing to the network and stages nothing. It resolves
the tag your machine follows, fetches the signature the registry holds for
whatever that tag points at *now*, verifies it, and prints the decision
`apex update` would reach. Both run the same code, so the two cannot drift. The
exit status is 1 if the update would be refused.

```
  Digest            sha256:daf8c8eb2928ab995a67ea9df43aa78116f638278bd0e7d32135a8b272e4ebec
  Signature         verified — signed by https://github.com/AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/main
                    (the transparency log was not checked)
  Provenance        none published — the registry holds no SBOM attestation for this digest
  Enforcement       signature enforce, provenance warn
  Deploying this    yes, with what follows unestablished
                    this image has no provenance: the registry holds no SBOM attestation for this digest
```

`apex trust --verify` answers a different question: is the image you are
already **running** signed? The two answers often disagree, because `apex`,
`daily`, `gaming-mesa`, `gaming-nvidia` and `edge` all name one digest, and CI
moves it on every successful build of `main` (see
[update-channels.md](update-channels.md)). Your machine runs the digest from
your last update, and the tag has moved since. Only the image you have not
deployed yet can be refused, so that is the one the gate checks.

`apex update --check` does **not** run the gate. It asks bootc whether an update
exists, which is a third question; `apex trust --gate` is the readout for the
gate.

## What "verify" means here

For the digest the registry serves now, all of these must hold:

1. the signature payload's SHA-256 matches the layer digest that named it;
2. the ECDSA signature verifies over that payload under the public key in the
   signing certificate;
3. that certificate chains to the **pinned** Sigstore root the image ships at
   `/usr/share/apex-os/trust/fulcio-root.pem`. Pinned, and not taken from the
   signature: a cosign signature carries Fulcio's intermediate *and* root, and
   checking a certificate against a root the certificate handed you proves
   nothing;
4. the certificate's identity is the one this machine expects (the release
   workflow), and its OIDC issuer is GitHub's token endpoint;
5. the signed payload names **this** digest and **this** repository. Without
   this check, a genuine signature over a *different* image from the same
   publisher would verify, and a moving tag makes that substitution easy.

The gate leaves out two checks on purpose.

**The transparency log is not checked.** Nothing above authenticates the
signature's Rekor bundle, so an attacker who can serve you a manifest can put
any timestamp in it. The gate therefore verifies the chain at the signing
certificate's own `notBefore`, which *is* authenticated. You give up proof that
the signature was ever published to a public log. This is the posture of
`cosign verify --insecure-ignore-tlog`, and every report says "the transparency
log was not checked" instead of an unqualified "verified".

**cosign is not used, and is not installed.** Fedora does not package it
(`dnf5 repoquery cosign 'cosign*' 'sigstore*'` returns nothing across every
configured repository), so a gate built on it would have been dead code on every
machine APEX ships to. The gate verifies with `skopeo` and `openssl`, which the
image already carries.

## What the SBOM attestation contains

The provenance half of the gate checks an SBOM that CI attaches to the image as
a signed attestation. "SBOM" names two different artefacts and APEX publishes
the smaller one, so this section spells out what the document covers.

**History:** the step is in `build-image.yml` on `main` and runs only when the
repository variable `APEX_ATTEST_SBOM` is `true`. The first attested image,
9690b65d (the merge of PR #42, 2026-09-23), was refused by every machine:
`apex update` before 0e48a1009 read the attestation's signature from a layer
annotation that cosign leaves empty on a DSSE attestation, and a provenance
check that is present and fails refuses even under `provenance=warn`. CI then
published unattested images until the fleet ran 0e48a1009 or later, which is
why `provenance` defaults to `warn` further down. On 2026-09-28, with the L16
and katana both on images that carry the fix, the variable was set to `true`,
and every build since attaches the attestation. A machine still on an image
older than 0e48a1009 (built before 2026-09-23) refuses these updates; it needs
`provenance=off` in `/etc/apex/trust.conf` for one update, which brings the
fixed verifier.

**What is in it.** Every package syft finds in the image (9,830 of them), with
its name, version, purl and CPEs. That covers the RPM set, the npm trees inside
the Claude and ChatGPT desktop apps, and the Go and Rust modules inside the
binaries. It is an SPDX document, attested with `--type spdxjson`, signed by the
same keyless GitHub identity as the image itself, and **published to the public
Sigstore transparency log**. The build fails if cosign reports no log entry, so
that last claim cannot stop being true unnoticed.

The CPEs are the key a vulnerability scanner matches a CVE advisory against.
They are also 27% of the document. They stayed in instead of being traded for
headroom, because an SBOM that fits only by matching fewer advisories is a
smaller answer to the wrong question.

**What is not in it: the file inventory, or the relationship graph.** What CI
publishes is a **flat list of packages**. syft's full output also lists every
file in the image with its digests, an edge from each file to the package that
owns it, and a 16,463-edge `DEPENDENCY_OF` graph saying which package pulled in
which. The attestation carries none of that.

The document answers *"which packages, at which versions, is this image built
from"*, the question a CVE advisory makes you ask. It does **not** answer
*"which files does this image contain, with what digests"* or *"why is this
package here"*.

**Why the reduction.** `rekor.sigstore.dev` refuses any request body of 24 MiB
or more at its fronting proxy, before reading a byte. Sixteen ascending POSTs on
2026-09-20 measured it: Rekor accepted and processed 25,000,092 bytes, and
answered 25,165,916 bytes with HTTP 502 in 0.43 seconds, having uploaded
nothing. `cosign attest` base64-encodes the predicate into a DSSE envelope, so
the ceiling on the document itself is about 18.8 MB.

syft's full output for this image is 166 MB. The complete package inventory with
its files and graph is 28.3 MB, still well over. Dropping the files and the
graph brings it to 14.1 MB, about 74% of the ceiling, and that is what CI
publishes. The headroom is thin, and the package count drives it: the Electron
bundles are the part that moves.

The choice was between a fuller SBOM that is signed but **not** in any public
log, and the package inventory that is. APEX takes the second. A transparency
log that covers only the artefacts small enough to fit guarantees little, and
the dropped parts are *structure*: anyone can reproduce them from the same
bytes, and nobody can reproduce identity that way. For the full document, the
image is public and its digest is in the signature:

```bash
syft "registry:ghcr.io/andrenijman/apex-os@sha256:..." -o spdx-json
```

That reproduces the full file-level document, with the dependency graph, from
the same bytes CI catalogued. It needs about 16 GB of memory and fifteen
minutes.

**Reading the attestation yourself:**

```bash
cosign verify-attestation \
  ghcr.io/andrenijman/apex-os@sha256:... \
  --type spdxjson \
  --certificate-identity-regexp '^https://github\.com/AndreNijman/apex-os/\.github/workflows/build-image\.yml@' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  | jq -r '.payload | @base64d | fromjson | .predicate.packages[] | "\(.name) \(.versionInfo)"'
```

Your machine does less than this, and says so (see the transparency-log note
above). APEX does not install `cosign` and Fedora does not package it; run the
command above on a workstation that has it.

## What is enforced, and how to change it

Two checks, each set to `enforce`, `warn` or `off`:

| | `enforce` | `warn` | `off` |
|---|---|---|---|
| verified | deploy | deploy | deploy |
| fails to verify | **refuse** | **refuse** | warn, deploy |
| none published | **refuse** | warn, deploy | warn, deploy |
| could not be checked | **refuse** | warn, deploy | warn, deploy |

The middle column refuses a *failed* check on purpose. A signature that
verifies **wrongly** is the signature of an attack, and a flaky network does not
explain it away. A signature that is missing, or that could not be fetched, is
a gap, and a warning is the proportionate answer to a gap.

The image's defaults are in `/usr/share/apex-os/trust/enforcement.conf`:

```
signature=enforce
provenance=warn
```

`signature=enforce` because every published APEX image is signed, so refusing an
unsigned one costs nothing. `provenance=warn` because the images CI publishes
now carry no SBOM attestation (the step is off; see *Tense* above), so `enforce`
would refuse every update on every machine until the publisher turns it back
on. That would be an outage posing as a security control.

To change it on your machine, write only the keys you want to change to
`/etc/apex/trust.conf`. APEX reads it after the image's file and it wins per
key, so a one-line file is enough:

```bash
# stop warning about the SBOM attestation nobody publishes yet
printf 'provenance=off\n' | sudo tee /etc/apex/trust.conf
```

A value that is not one of the three leaves the setting where it was, and the
note names the file and line. A typo must not turn a machine that checks
signatures into one that does not:

```
  note              /etc/apex/trust.conf:1: enfore is not one of enforce, warn, off — leaving signature as it was
```

A file that exists but cannot be read also produces a note, and the built-in
default applies. An unreadable policy file never turns into a permissive one.

## When it refuses

```
apex: this update is being held. The image it would deploy does not verify.
  the signature for sha256:daf8c8eb...
  the signature does not verify: the signature covers sha256:1234abcd..., not the sha256:daf8c8eb... being deployed

This machine enforces: signature enforce, provenance warn.

APEX publishes a cosign signature for every image, and this machine checks it before deploying.
A refusal means this machine could not establish that the image the registry is serving
is the one it was told to expect — see the line above for whether that is because the
signature was wrong or because the check could not be made. Either way the update
stops before anything is downloaded.

If you know why and want it anyway: `sudo apex update --allow-unverified`.
To change what is enforced permanently, edit /etc/apex/trust.conf — see docs/trust-enforcement.md.
```

Read the middle line. **"does not verify" and "could not be checked" are
different accusations.** The first says the image is not what it claims to be.
The second says your machine could not find out: an unreachable registry, a
missing pinned root, no `openssl`. The gate names which one, and which of the
two checks refused.

`--allow-unverified` deploys anyway, once, and still prints the whole refusal
first. An escape hatch that also hid the reason would leave you not knowing what
was wrong with the image you just deployed.

**`--force` is a different flag and does not skip this.** `--force` overrides
§26's rollout stop, for a machine that came back broken from its last update
(see [update-channels.md](update-channels.md)). The two gates answer unrelated
questions, and working around a health stop must not also switch off the check
on who signed your next image.

## Things worth knowing

**Offline, under `enforce`, the gate refuses the update.** With no registry to
reach, nothing can be verified, and `enforce` means what it says. `bootc` could
not have pulled anything either. If you update over a flaky link and would
rather have the update than the check, `provenance=warn` is already the default
and `signature=warn` does the same for the other half.

**`/etc/containers/policy.json` is a separate mechanism, and the gate leaves it
alone.** It ships as a single `insecureAcceptAnything` and is what `bootc`
itself consults; `apex trust` reports it under "next update". If you ever
tighten it, mind the interaction: the gate fetches signatures with `skopeo
copy`, which honours the same policy, so a `sigstoreSigned` scope covering this
repository would start requiring signatures on the `.sig` artifacts themselves,
and the gate would report "could not be checked".

**Running `bootc upgrade` yourself bypasses the gate.** `apex update` is the
only path in this project that consults it. That boundary is by design: the gate
is a policy APEX applies to its own update verb, and it does not restrain bootc
or the kernel.

**Rotating the pinned root** takes an image build. The build checks the root's
SHA-256 fingerprint, so a wrong or corrupt root fails the build instead of
refusing every machine's next update.

## The rename to Rime OS

APEX becomes Rime OS, and the repository moves from `AndreNijman/apex-os` to
`AndreNijman/rime-os`. A Sigstore identity names the repository, so every image
built after the rename carries a new signer. `apex` accepts both identities by
default (`EXPECTED_SIGNER` and `RENAMED_SIGNER` in `apexd/apex/src/trust.rs`),
and it learned the new one a release before the rename. A machine that updated
to that release keeps updating across the rename. An image signed before it,
and a rollback to one, still verifies.

The image name moves too, and GHCR does not redirect a renamed package. Every
published build goes out under both `ghcr.io/andrenijman/apex-os` and
`ghcr.io/andrenijman/rime-os`, with the same digest, a signature and attestation
made under each name, and the same tags. On `apex update`, a machine that tracks
a tag of the old name checks whether the new name serves that tag. If it does,
the update runs `bootc switch` to it after the gate verifies the new name. If
the switch fails, the update checks the old name and upgrades under it instead.
A digest pin or a fork's image never moves.

## A fork that publishes its own images

Three optional image-owned files, all under `/usr/share/apex-os/trust/`:

| file | replaces |
|---|---|
| `expected-signer` | the certificate identities to expect (one per line, any of them accepted) |
| `expected-issuer` | the OIDC issuer to expect |
| `fulcio-root.pem` | the root that identity's certificate must chain to |

The rest of the report stays true for a fork, which is why these three are
files and not a patch to the source.
