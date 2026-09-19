# sbom-attest — the SBOM is generated, and cannot be transparency-logged as it stands

items: none (a guard, like `sbom-oom` was) — it gates unit `final`
repo: apex-os
worktree: `/var/tmp/apex-work/wt-sbom-attest`, branch `task/sbom-attest` off
`roadmap/v2.2` (`b7e28d87`)
scratch: `/tmp/claude-1000/-var-home-andre-Projects-apex/5717b375-fcad-4ae2-b07f-9e8456f318ee/scratchpad/sbom-attest/` (per-agent)

## THE ANSWER, in one line

**Rekor's fronting proxy refuses any request body at or above 24 MiB with a
bare HTTP 502 before a single body byte is uploaded** — not a 413 — and the
predicate cosign posts for this image is roughly **222 MB**. No retry can ever
succeed. The lever is the predicate, not the transport.

## FOUND — measured 2026-09-20, not inherited

### 1. What Rekor actually returns (16 direct probes, deterministic)

`POST https://rekor.sigstore.dev/api/v1/log/entries`, well-shaped but
deliberately invalid proposed intoto entries (an invalid entry can never be
logged, so this pollutes nothing), `Expect: 100-continue`, ascending sizes:

| request body | result |
|---|---|
| 1 MiB … 22 MiB | **400** `error processing entry: invalid public key` — body fully uploaded, Rekor *processed* it |
| 23,999,092 B | **400** — uploaded, processed |
| 24,117,340 B (23 MiB) | **400** — uploaded, processed |
| 25,000,000 B | **400** — uploaded, processed |
| 25,000,092 B | **400** — uploaded, processed |
| **25,165,916 B (24 MiB + 92)** | **502**, `size_upload=0`, 0.44 s |
| 26,000,092 / 30,000,092 B | **502**, `size_upload=0`, ~0.43 s |
| 33,554,524 B (32 MiB) | **502**, `size_upload=0`, ~0.43 s |
| 67,108,956 B (64 MiB) | **502**, `size_upload=0`, ~0.43 s |
| 134 MB / 268 MB | **502**, `size_upload=0`, ~0.43 s |

The boundary is between **25,000,092 B (passes)** and **25,165,916 B (refused)**,
which brackets **25,165,824 B = exactly 24 MiB**. Every 502 was re-tested and
is deterministic. Response headers on a refusal carry no `server:` and no
`via:` — just `content-length: 136`, `text/html`, `alt-svc: h3` — so this is the
fronting proxy, not Rekor's application, and it rejects on `Content-Length` at
header time.

### 2. THE PREDECESSOR'S DIAGNOSIS WAS RIGHT IN SHAPE AND WRONG IN MECHANISM

The brief carried "the public Rekor instance rejects oversized entries
(sigstore/rekor#2808); GitHub's own documented cap is 16 MB", i.e. an expected
**413**. It is **not a 413**, and the difference is load-bearing three ways:

1. **502 is in `go-retryablehttp`'s retryable class** (`>= 500`). A 413 is not.
   That is exactly why cosign burned four attempts and why its error message —
   `Post "https://rekor.sigstore.dev/api/v1/log/entries": giving up after 4
   attempt(s)` — **cannot structurally name the cause**. It is the generic
   retries-exhausted string. Three rounds read this as a network fault because
   a network fault is precisely what it looks like.
2. **The cap is 24 MiB, not 16 MB.** 16 MiB and 22 MiB bodies are accepted and
   processed. A design budgeted against 16 MB would leave a third of the real
   allowance unused.
3. **No body is ever uploaded.** `size_upload=0` and ~0.43 s per attempt. The
   failing CI step spent 16:20:02.95 → 16:20:06.81 = **3.86 s** on all four
   attempts; posting a ~222 MB body four times over would take minutes. The
   timing in the real log independently confirms the header-time refusal.

### 3. The predicate is ~222 MB on the wire, ~9x the ceiling

syft emits **166 MB** of spdx-json (9,830 packages). `cosign attest` wraps it in
an in-toto statement and DSSE-signs it, so the posted body is base64 of the
statement — **×1.334, ≈ 222 MB**. To fit under 24 MiB the *predicate* must come
in under roughly **17.5 MB**, and ≤ 15 MB to leave real headroom.

### 3b. The expansion factor is exactly 4/3, measured offline, not reasoned about

`cosign attest` was run locally (v2.5.2, the same version `cosign-installer@v3`
puts on the runner) against a synthetic 20,000-package SPDX predicate of
4,824,532 B compact, with a static key and `--tlog-upload=false` — no OIDC, no
keyring prompt, no network:

```
predicate compact bytes  : 4824532
statement bytes          : 4824753   (predicate + 221 B of in-toto wrapper)
DSSE payload (base64)    : 6433004   factor 1.3334
DSSE envelope            : 6433196   factor 1.3334
predicateType            : https://spdx.dev/Document
packages carried through : 20000
```

Three things this settles rather than assumes:

- **cosign embeds the predicate whole.** All 20,000 packages came back out of
  the envelope. It does not hash, trim or reference it.
- **The factor is 4/3, not 16/9.** The bundle *file* base64s the envelope a
  second time (1.7778), which is easy to mistake for the wire size. The body
  Rekor sees is the envelope, so the size guard multiplies by 4/3. Getting this
  wrong by 33% in either direction is the difference between a guard that fires
  early and one that never fires.
- **`--type spdxjson` produces `predicateType: https://spdx.dev/Document`**,
  which contains `spdx` and therefore satisfies `verify.rs:540`. The machine
  side needs no change for option B. (This is the check that rules option C out
  of a single landing.)

Measure the **compact** document: cosign marshals with Go's `json.Marshal`, so
syft's pretty-printed output is not what goes on the wire.

### 4. This is not only a CI problem — the 166 MB attestation would land on users

`apexd/apex/src/verify.rs::verify_provenance` fetches the `.att` layer with
`blob()` **entirely into memory**, base64-decodes the DSSE payload, and then
does `String::from_utf8_lossy(&payload).into_owned()` — a third full copy. A
166 MB SBOM means **every `apex trust`, every `apex trust --gate` and every
`apex update`** downloads ~222 MB and holds ~550 MB of it in RAM on a user's
laptop. Even with an unlimited Rekor, a 166 MB attestation is the wrong
artefact. The shrink is right on its own merits.

### 5. The constraint that eliminates option C

`verify.rs:540` requires the statement's `predicateType` to **contain `spdx`**;
anything else is `Verdict::Failed`, and by the enforcement table a *failed*
provenance check is **refused even at `provenance=warn`**. So "attest a small
custom predicate naming the SBOM's digest" cannot ship without a coordinated
`verify.rs` change, and shipping it out of order would refuse updates on every
machine in the field. **Keeping `--type spdxjson` is worth real money.**

### 6. Signing itself is fine

The same run logged `tlog entry created with index: 2892864107` for
`cosign sign` 13 minutes before the attest failed. Rekor was up, the runner's
network was fine, the OIDC identity worked. Only the size is wrong.

### 7. THE OBVIOUS KNOB IS NOT THE LEVER — measured on alpine, not on the probe

`SYFT_FILE_METADATA_SELECTION=none` reads like the fix and is not. Run against
`docker.io/library/alpine:3.20` with the same pinned syft 1.52.0, because a
small image answers this in seconds instead of twelve minutes:

| configuration | packages | files | relationships | compact |
|---|---|---|---|---|
| default | 15 | 77 | 125 | 87,692 B |
| `SYFT_FILE_METADATA_SELECTION=none` | 15 | **77** | 125 | 76,227 B (−13%) |
| `SYFT_RELATIONSHIPS_PACKAGE_FILE_OWNERSHIP=false` | 15 | **2** | 48 | 31,767 B (−64%) |
| both | 15 | 1 | 48 | 31,130 B |

`FILE_METADATA_SELECTION=none` drops each file's **digests**, not the file
rows. The rows and their ownership edges go with the RELATIONSHIPS switch. The
first commit of this unit set only the former and would have shipped a 13%
reduction against a problem that needs 90%.

**The package sets are identical** between the default run and the stripped
one — the sorted `name@version` list of all fifteen `diff`s clean. This drops
files, not packages.

**The third switch errors, and must not be forced.**
`SYFT_RELATIONSHIPS_PACKAGE_FILE_OWNERSHIP_OVERLAP=false` makes syft 1.52.0
exit 1 before it catalogues anything:

```
invalid application config: cannot enable exclude-binary-overlap-by-ownership
without enabling package-file-ownership-overlap
```

The knob it demands in exchange decides whether a binary syft found and the RPM
that owns it are one package or two — i.e. it changes the PACKAGE set. Left
alone deliberately. **The probe's `norel` arm will therefore fail with rc=1 and
no output; that is explained, not a defect.**

A value syft does not recognise also exits 1 (checked with `=nonsense`), so a
typo cannot silently produce a smaller document.

### 8. HOW TO READ THE PROBE'S `jq-strip` NUMBER — it is ~9% LOW

The probe was written before finding 7, so **no arm runs the configuration the
build actually uses**. `nofiles` is the weak knob, `norel` cannot run, `full` is
the baseline. Only `jq-strip` approximates package-level — and it is not
equivalent, in a direction that matters:

| on alpine:3.20 | packages | files | rels | compact |
|---|---|---|---|---|
| probe's `jq-strip` of the default doc | 15 | 0 | 34 | 28,552 B |
| the build's real config (ownership+metadata off) | 15 | 1 | 48 | **31,130 B** |

**`jq-strip` under-reports by about 9%** (1.090x). The difference is 14 `OTHER`
relationships — the package-file-ownership-**overlap** edges, which finding 7
says must stay. jq-strip deletes every file-anchored edge; `ownership=false`
keeps the overlap ones.

So: multiply the probe's `RESULT jq-strip: compact=` by ~1.1 before comparing it
with the budget, and treat `REKOR jq-strip: ACCEPTED` as *"Rekor accepts a
package-level document of roughly this size"* — **not** as "the fix is
verified". Only a real build verifies the real document. The 1.09 comes from a
15-package image and is indicative, not exact.

## CHOSEN OPTION — shrink the predicate, keep the transparency log

Ranked, with what each costs:

- **B. Shrink to a package-level SPDX document, keep `--type spdxjson`, keep the
  tlog. CHOSEN.** Costs the *file-level* inventory (per-file paths, digests and
  their relationship edges). Keeps all 9,830 packages, which is what answers a
  CVE question. Gives up nothing in the signature or transparency claim, needs
  no `verify.rs` change, and fixes finding 4 as a side effect.
- **C. Small digest-committing predicate + full SBOM as an OCI artefact.**
  Transparency guarantee fully intact and keeps the file level too, but breaks
  `verify.rs:540` on every machine in the field unless landed in lockstep.
  Fallback only if B does not fit.
- **A. `--tlog-upload=false` (+ Sigstore TSA).** Signed and attached but **not
  publicly logged**: an attacker with the OIDC identity leaves no artefact-
  signature record. The Fulcio certificate is still in CT (the run verifies the
  SCT), so what is lost is specifically the signature record, not all trace.
  Last resort, and `docs/` would have to stop claiming a transparency log.
- **D. A private Rekor.** Out of scope, stated rather than left unconsidered: it
  needs hosting, its own key management, and every APEX machine taught to trust
  a second log — a larger surface than the problem.

## RUNS DISPATCHED — pick these up if I am cut short

- **`35456273175`** — **`build-image` on `task/sbom-attest`, THE VERIFICATION
  BUILD**, dispatched 2026-09-19T16:51Z. `core` is skipped (the paths filter
  deliberately excludes `build-image.yml`), so this is `base` ~16 min then
  `image` ~24 min, not the ~1 h a full build costs. **What confirms the fix:**
  step `Generate and attest the SBOM` = success *and not skipped* (a skipped
  job counts as success in this repo — six documented instances), the line
  `predicate …B compact -> …B DSSE body; ceiling 25165824B, at N%`, and
  `tlog entry created with index: N` in that step's log. Then
  `Verify the image and its SBOM are both retrievable` must also be success.

- **`35456401012`** — `sbom-probe` **round 4**, pushed 2026-09-19T16:55Z.
  ONE arm: the exact two switches `build-image.yml` sets, against the same
  already-pushed digest. ~15 min, one runner. Grep its log for `RESULT ` and
  `REKOR `. It is an independent cross-check of the build, not a substitute:
  if the build's guard fires, this run's numbers say which lever to reach for
  next.

- **`35455459788`** — `sbom-probe` round 3, COMPLETE, **partially failed, and the
  failure was mine**. What it did establish:
  - **The 24 MiB ceiling is confirmed from a GitHub runner**, so the bisection
    does not rest on one home connection: `body=25,000,000 -> http=400
    sent=25000000` and `body=25,166,008 -> http=502 sent=0 t=0.26s`.
  - The `full` arm: rc=0, **915 s**, peak RSS **15,449,996 KB** — consistent
    with build 35450175806's 742 s / 15,303,652 KB.
  - Then the step died on **my own harness bug**:
    `line 39: name: unbound variable`. `local name="$1" f="…${name}…"` expands
    every word BEFORE it assigns any of them, so under `set -u` the second
    initialiser reads `name` while it is still unbound. Every size measurement
    and every Rekor upload was lost with it. **915 s of syft, thrown away by a
    one-line bash rule.** Fixed in the next commit.

## NEXT

1. Read run `35455459788`. `RESULT <arm>: FITS` / `OVER` is the verdict line;
   `REKOR <arm>: ACCEPTED` with a logIndex is the proof.
2. If `nofiles` fits: that is the landing. `SYFT_FILE_METADATA_SELECTION=none`
   in the SBOM step, keep `--type spdxjson`, keep the tlog, add the size guard.
3. If it does not: `norel` next, then `jq-strip`, then fall back to option C —
   and option C needs `verify.rs:540` changed **in the same landing**.
4. Then dispatch `gh workflow run build-image.yml --ref task/sbom-attest` and
   put the run id in the section above BEFORE reading it.

## BLOCKED ON

- nothing.
