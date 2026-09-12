# The signature gate: what APEX refuses to deploy, and why it might refuse yours

Roadmap §27, the enforcement half.

## What changed

Every APEX image published since the beginning has been cosign-signed under a
keyless GitHub identity, and CI verifies its own signature before it moves a
tag. None of that reached your machine. `apex trust` could tell you nobody had
checked — that was the readout half — and then `apex update` deployed the image
anyway.

Now `apex update` checks the signature of the image it is about to deploy, and
refuses to deploy one it cannot verify.

## Seeing what your machine will do, before it does it

```bash
apex trust --gate
```

No root, no network writes, nothing staged. It resolves the tag your machine
follows, fetches the signature the registry holds for whatever that tag points
at *now*, verifies it, and prints the decision `apex update` would reach — from
the same code, so the two cannot drift. Exit status is 1 if the update would be
refused.

```
  Digest            sha256:daf8c8eb2928ab995a67ea9df43aa78116f638278bd0e7d32135a8b272e4ebec
  Signature         verified — signed by https://github.com/AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/main
                    (the transparency log was not checked)
  Provenance        none published — the registry holds no SBOM attestation for this digest
  Enforcement       signature enforce, provenance warn
  Deploying this    yes, with what follows unestablished
                    this image has no provenance: the registry holds no SBOM attestation for this digest
```

`apex trust --verify` answers a *different* question: is the image you are
already **running** signed. Both are worth knowing and they routinely disagree,
because `apex`, `daily`, `gaming-mesa` and `gaming-nvidia` are four names for
one digest that moves on every successful build of `main` (see
[update-channels.md](update-channels.md)). Your machine is on the digest from
whenever you last updated; the tag has moved since. Only the second one can be
refused, so only the second one is what the gate looks at.

`apex update --check` does **not** run the gate. It asks bootc whether an update
exists, which is a different question again; `apex trust --gate` is the readout
for this one.

## What "verify" means here, exactly

For the digest the registry currently serves, all of these must hold:

1. the signature payload's SHA-256 matches the layer digest that named it;
2. the ECDSA signature verifies over that payload under the public key in the
   signing certificate;
3. that certificate chains to the **pinned** Sigstore root the image ships at
   `/usr/share/apex-os/trust/fulcio-root.pem`. Pinned, and not taken from the
   signature: a cosign signature carries Fulcio's intermediate *and* root, and
   checking a certificate against a root the certificate handed you proves
   nothing;
4. the certificate's identity is the one this machine expects — the release
   workflow — and its OIDC issuer is GitHub's token endpoint;
5. the signed payload names **this** digest and **this** repository. Without
   this, a genuine signature over a *different* image from the same publisher
   would verify, which is exactly the substitution a moving tag makes easy.

Two things it deliberately does not do.

**The transparency log is not checked.** The signature's Rekor bundle is not
authenticated by anything above, so an attacker who can serve you a manifest can
put any timestamp in it. The chain is therefore verified at the signing
certificate's own `notBefore`, which *is* authenticated. What this gives up is
proof that the signature was ever published to a public log. It is the same
posture as `cosign verify --insecure-ignore-tlog`, and every report says "the
transparency log was not checked" rather than printing an unqualified
"verified".

**cosign is not used, and is not installed.** It is not packaged for Fedora at
all — `dnf5 repoquery cosign 'cosign*' 'sigstore*'` is empty across every
configured repository — so a gate built on it would have been dead code on every
machine APEX ships to. The verification above is done with `skopeo` and
`openssl`, both of which are already in the image.

## What is enforced, and how to change it

Two checks, each set to `enforce`, `warn` or `off`:

| | `enforce` | `warn` | `off` |
|---|---|---|---|
| verified | deploy | deploy | deploy |
| fails to verify | **refuse** | **refuse** | warn, deploy |
| none published | **refuse** | warn, deploy | warn, deploy |
| could not be checked | **refuse** | warn, deploy | warn, deploy |

`warn` refusing a *failed* check is the point of the middle column. A signature
that verifies **wrongly** is the signature of an attack, and no amount of "the
network was flaky" makes it not one. A signature that is missing, or that could
not be fetched, is a gap — and warning about a gap is proportionate.

The image's defaults are in `/usr/share/apex-os/trust/enforcement.conf`:

```
signature=enforce
provenance=warn
```

`signature=enforce` because every published APEX image is signed, so refusing an
unsigned one costs nothing. `provenance=warn` because **no** published APEX image
has an SBOM attestation yet: the `cosign attest` step exists on `roadmap/v2.2`
and has never run on `main`, so `enforce` would refuse every update on every
machine on the grounds that the publisher has not caught up. That is an outage
dressed as a security control.

To change it on your machine, write only the keys you want to change to
`/etc/apex/trust.conf`. It is read after the image's file and wins per key, so a
one-line file is enough:

```bash
# stop warning about the SBOM attestation nobody publishes yet
printf 'provenance=off\n' | sudo tee /etc/apex/trust.conf
```

A value that is not one of the three leaves the setting where it was and says
so, with the file and line. A typo must not be the difference between a machine
that checks signatures and one that does not:

```
  note              /etc/apex/trust.conf:1: enfore is not one of enforce, warn, off — leaving signature as it was
```

A file that exists but cannot be read is also a note, and the built-in default
applies. An unreadable policy file never becomes a permissive one.

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
The second says your machine could not find out — an unreachable registry, a
missing pinned root, no `openssl`. The gate names which one, and which of the two
checks refused.

`--allow-unverified` deploys anyway, once, and still prints the whole refusal
first — an escape hatch that also hid the reason would be a way to not find out
what was wrong with the image you just deployed.

**`--force` is a different flag and does not skip this.** That one is §26's
rollout stop, for a machine that came back from its last update broken (see
[update-channels.md](update-channels.md)). The two gates answer unrelated
questions, and working around a health stop must not silently stop checking who
signed your next image.

## Things worth knowing

**Offline, under `enforce`, the update is refused.** The registry could not be
reached, so nothing could be verified, and `enforce` means what it says. In
practice `bootc` could not have pulled anything either. If you update over a
flaky link and would rather have the update than the check, `provenance=warn`
is already the default and `signature=warn` is the equivalent for the other
half.

**`/etc/containers/policy.json` is a separate mechanism and is untouched.** It
ships as a single `insecureAcceptAnything` and is what `bootc` itself consults;
`apex trust` reports it under "next update". Note the interaction if you ever
tighten it: signature fetching here uses `skopeo copy`, which honours that same
policy, so a `sigstoreSigned` scope covering this repository would start
requiring signatures on the `.sig` artifacts themselves and the gate would
become "could not be checked".

**Running `bootc upgrade` yourself bypasses the gate entirely.** `apex update`
is the only path in this project that consults it. That is a real boundary, not
an oversight: the gate is a policy APEX applies to its own update verb, not a
kernel-level restraint.

**Rotating the pinned root** means an image build. It is verified at build time
by SHA-256 fingerprint, so a wrong or corrupt root fails the build rather than
refusing every machine's next update.

## A fork that publishes its own images

Three optional image-owned files, all under `/usr/share/apex-os/trust/`:

| file | replaces |
|---|---|
| `expected-signer` | the certificate identity to expect |
| `expected-issuer` | the OIDC issuer to expect |
| `fulcio-root.pem` | the root that identity's certificate must chain to |

Everything else in the report stays true, which is why these three are files
rather than a patch to the source.
