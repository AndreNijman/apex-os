# sbom-attest — DONE. Rekor caps a request body at 24 MiB; the SBOM now fits, and is logged

items: none (a guard, like `sbom-oom` was) — it gates unit `final`
repo: apex-os
worktree: `/var/tmp/apex-work/wt-sbom-attest`, branch `task/sbom-attest` off
`roadmap/v2.2` (`b7e28d87`)
scratch: `/tmp/claude-1000/-var-home-andre-Projects-apex/5717b375-fcad-4ae2-b07f-9e8456f318ee/scratchpad/sbom-attest/` (per-agent)

## VERIFIED — build `35457162588`, every step of `image` green

```
syft catalogued the image in 314s
Maximum resident set size (kbytes): 14913824
SBOM packages: 9830   file rows: 7049
after the trim: packages 9830, cpe refs 55652, DESCRIBES 1
predicate 14087745B compact -> ~18783660B DSSE body; ceiling 25165824B, at 74%
tlog entry created with index: 2893110210
signature and SBOM attestation both verify against
  https://github.com/AndreNijman/apex-os/.github/workflows/build-image.yml@refs/heads/task/sbom-attest
```

`tlog entry created with index: 2893110210` is the line this unit existed to
produce, and `cosign verify-attestation` passing in the same job is the proof
that it is retrievable and binds to this digest.

**Confirmed independently of the build**, with `skopeo` against the registry
rather than by trusting the job's own verify step:

```
skopeo inspect --raw docker://ghcr.io/andrenijman/apex-os:sha256-9e532b06….att
  layers[0].mediaType = application/vnd.dsse.envelope.v1+json
  layers[0].size      = 18,811,140
```

That is the exact media type `verify.rs::verify_provenance` looks for
(`MEDIA_DSSE`), and the size matches the predicted 18,783,660 B body to within
the envelope's own signature block. The artefact exists, on the digest, in the
shape the machine-side gate expects.

Two things beyond the step itself:

- **`Promote to every published tag` and `Assert every published tag resolves to
  this digest` are green.** They had been **skipped** in every failing run since
  the SBOM step was added — unproved, not passing, and this repo counts a
  skipped job as success. This is the first run that actually exercised them.
- The predicate came out **byte-identical** to the document measured locally
  from probe round 4's artefact — 14,087,745 B both times — so the candidate
  table in section 10 describes this exact document, not an approximation.

## THE ANSWER, in one line

**Rekor's fronting proxy refuses any request body at or above 24 MiB with a
bare HTTP 502 before a single body byte is uploaded** — not a 413, and 502 is
retryable, which is why cosign said "giving up after 4 attempt(s)" and named
nothing. The predicate was ~222 MB. No retry could ever have worked.

**The fix keeps everything that matters.** The published SBOM is the complete
package inventory — all 9,830 packages, every purl, all 55,652 CPEs, both
Electron trees — with the file rows and the relationship graph dropped:
14,087,745 B, **74% of the ceiling**, still signed, still `spdxjson`, still in
the public transparency log. Nothing about the guarantee is weakened; what is
lost is structure, which anyone can regenerate from the published digest.

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

### 8. SUPERSEDED — kept only so the number is not re-derived

Round 3's probe had no arm running the build's real configuration, so this
section used to explain how to correct its `jq-strip` figure upward by ~9%
(measured on alpine: `jq-strip` 28,552 B against the real config's 31,130 B, the
difference being the `OTHER` overlap edges `jq-strip` deletes and
`ownership=false` keeps). Round 4 ran the real configuration against the real
image, so the correction factor is no longer needed for anything. It is left
here because it is a measured fact about the two transformations, and because
the final trim deletes `OTHER` too — which is where 5,883,495 B of the saving
comes from.

### 9. MEASURED, 2026-09-20: THE PACKAGE-LEVEL DOCUMENT IS STILL OVER

Probe round 4 (`35456401012`) ran the exact two switches with syft 1.52.0
against the pushed digest. **335 s** (down from 915 s), peak RSS
**15,547,064 KB**, and:

```
RESULT packages=9830 files=7049 relationships=43801
RESULT pretty=28259405B compact=28259405B dsse=37679208B ceiling=25165824B
RESULT OVER by 12513384B
RESULT electron trees: @anthropic-ai/claude-code, @anthropic-ai/claude-code-linux-x64, chatgpt, electron
RESULT relationship types: {"CONTAINS":9829,"DEPENDENCY_OF":16463,"DESCRIBES":1,"OTHER":17508}
──── cosign rc=1 after 3s      REKOR: REFUSED
```

**1.5x over the ceiling, and Rekor refused it for real** — the same 3-second,
four-attempt refusal the build saw. The two syft switches are necessary and not
sufficient.

Three corrections to earlier claims on this card and in the workflow comment:

- **`pretty` == `compact`.** syft's `spdx-json` is ALREADY compact, so there was
  never a third of the size to win by minifying, and the original document's
  166 MB was a compact 166 MB — the ~222 MB DSSE figure stands.
- **THE FILE PASS IS NOT WHAT DRIVES THE RSS.** Peak RSS with the file inventory
  off is **15,547,064 KB** against **15,449,996 KB** with it on. Identical. The
  workflow comment claimed this as a side benefit; it is false and is corrected.
  What the switches do buy is **time**: 915 s → 335 s, a 63% cut.
- `files` is 7,049, not 0: `ownership=false` leaves the rows the binary and
  executable cataloguers produce.

### 10. WHERE THE 28,259,405 BYTES ACTUALLY ARE

Measured off the probe's own artefact (`sbom-size-probe`, 14-day retention on
run `35456401012`), so this is the real document and not an estimate:

| | bytes | share |
|---|---:|---:|
| `packages` (9,830) | 14,076,102 | 49% |
| — of which `externalRefs` | 9,108,268 | 32% |
| — — `cpe23Type` (55,652 refs) | **7,634,382** | 27% |
| — — `purl` (9,830 refs) | 1,241,294 | 4% |
| — of which `sourceInfo` | 1,357,597 | 5% |
| `relationships` (43,801) | 10,872,960 | 38% |
| — `OTHER` (17,508) | **5,883,495** | 21% |
| — `DEPENDENCY_OF` (16,463) | 3,085,431 | 11% |
| — `CONTAINS` (9,829) | 1,860,087 | 7% |
| — `DESCRIBES` (1) | 145 | 0% |
| `files` (7,049) | 3,298,838 | 12% |

Candidate documents, each built from that artefact and measured, not estimated
(ceiling 25,165,824 B on the DSSE body):

| | compact | DSSE | vs ceiling |
|---|---:|---:|---|
| as probed | 28,260,388 | 37,680,520 | 149% OVER |
| A: drop `files` + file-anchored edges | 22,142,348 | 29,523,132 | 117% OVER |
| B: A + drop `OTHER` | 19,060,538 | 25,414,052 | **100% OVER by 248 KB** |
| C: B + drop `sourceInfo` | 17,702,941 | 23,603,924 | 93% fits |
| D: B + drop `cpe23Type` | 11,370,504 | 15,160,672 | 60% fits |
| **E: drop `files` + every edge but `DESCRIBES`** | **14,088,728** | **18,784,972** | **74% fits** |
| F: E + drop `cpe23Type` | 6,398,694 | 8,531,592 | 33% fits |

## CHOSEN OPTION — publish the package inventory, keep the transparency log

**What ships: the complete package inventory, with no file rows and no
relationship graph.** 14,087,745 B compact, an 18,783,660 B DSSE body, **74% of
the ceiling**. Produced by the two syft switches plus a `jq` trim that deletes
`files` and every relationship except the single `DESCRIBES`.

**What it keeps, in full:** all 9,830 packages, every version, all 9,830 purls,
all 55,652 CPEs, both Electron trees. The signature, the keyless identity, the
`spdxjson` predicate type and the public Rekor entry are all unchanged. Nothing
about the *guarantee* is weakened — `cosign verify-attestation` answers exactly
what it did before.

**What it costs, stated plainly:** the published SBOM is a **flat list**. It
does not say which files are in the image or what each one hashes to, and it
does not carry the 16,463-edge `DEPENDENCY_OF` graph saying why a package is
there. Both are *structure*, and structure is reproducible by anyone from the
same bytes — `syft registry:<digest> -o spdx-json` regenerates all of it. Identity
is not reproducible from anything smaller, so identity is what gets signed.

**Two cheaper options were measured and refused:**

- **Keeping `DEPENDENCY_OF` and paying for it by dropping `sourceInfo`** lands
  at 93% of the ceiling. Seven points of headroom on an image that gains
  packages every release is not headroom; the failure mode is the next Electron
  bump turning the build red.
- **Dropping `cpe23Type`** is the cheapest 27% in the document and is refused.
  CPEs are the key a vulnerability scanner matches an advisory against;
  shrinking the artefact by making it worse at its only question is not a saving.

**The three other options from the brief, and why not:**

- **`--tlog-upload=false`** (signed, attached, not publicly logged) — NOT taken.
  It was never necessary, because the predicate could be made to fit. It would
  have cost the one thing the docs claim, and the Fulcio certificate's CT entry
  is not a substitute: what would be lost is the artefact-signature record.
- **A small digest-committing predicate + the SBOM as a separate OCI artefact** —
  NOT taken. `verify.rs:540` requires `predicateType` to contain `spdx`, and a
  failed provenance check is refused even at `provenance=warn`, so shipping it
  out of lockstep would refuse updates on every machine in the field.
- **A private Rekor** — out of scope, and said rather than left unconsidered: it
  needs hosting, its own key management, and every APEX machine taught to trust
  a second log. A larger surface than the problem.

**The trim is checked, not trusted.** The `packages` array is canonically
sorted and SHA-256'd before and after and must be byte-identical; `files` must
be absent; exactly one `DESCRIBES` must remain. Run against the real 28 MB
document (green, 74%) and against two mutants — one that eats half the
packages, one that forgets to drop `files` — both rc=1.

**And the guard has two thresholds.** At the 24 MiB ceiling the build fails and
names the cause. At 80% it warns and stays green, so growth is visible before it
is fatal. All three branches exercised against real files of the right size.

## RUNS DISPATCHED — pick these up if I am cut short

- **`35457162588`** — **`build-image` on `task/sbom-attest`, THE VERIFICATION
  BUILD THAT MATTERS**, dispatched 2026-09-19T17:05Z on `0b14e851`+, i.e. the
  first commit that actually fits under the ceiling. ~85 min. **What confirms
  the fix**, in the `image` job's `Generate and attest the SBOM` step (which
  must be `success` AND not skipped):
  - `SBOM packages: 9830   file rows: 7049`
  - `after the trim: packages 9830, cpe refs 55652, DESCRIBES 1`
  - `predicate 14087745B compact -> ~18783660B DSSE body; … at 74%`
  - `tlog entry created with index: N`
  and then `Verify the image and its SBOM are both retrievable` = success.

- **`35457052441`** — NOT a build. Zero jobs, `completed/failure`: GitHub's
  record of the workflow **failing to parse**. The SBOM step's `run:` block had
  grown to 22,901 characters and a run block is capped at **21,000**
  (`Exceeded max expression length 21000`). A dispatch returns HTTP 422 and a
  push leaves a jobless failed run, neither of which looks like a workflow file
  that never parsed. Fixed by moving every prose block of six lines or more
  above the step as a YAML comment, where the cap does not apply: 22,901 → 6,236.

- **`35456273175`** — the FIRST verification build, **cancelled deliberately**
  once probe round 4 proved its configuration could not fit. Dispatched
  2026-09-19T16:51Z. `core` is skipped (the paths filter
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

**This unit is finished and verified. Unit `final` is unblocked.** What is left
belongs to whoever lands `roadmap/v2.2` on `main`, and none of it is this
branch's to do:

1. **Merge `task/sbom-attest` into `roadmap/v2.2` — the orchestrator's job, not
   this agent's.** The brief forbade committing to `roadmap/v2.2`, and
   `sbom-oom` landed the same way: the unit pushed its branch and the
   orchestrator merged it (`b7e28d87`). The branch is ready. Ten commits off
   `b7e28d87`,
   touching only `.github/workflows/build-image.yml` and
   `docs/trust-enforcement.md` (`.github/workflows/sbom-probe.yml` was added and
   retired on the branch). No Rust, no `files/`, no Containerfile — nothing that
   can conflict with another unit.
2. **Do NOT tighten `provenance=warn` yet.** `files/system/usr/share/apex-os/
   trust/enforcement.conf` must stay `warn` until a build **on `main`** has
   published an image carrying an `.att`. Build `35457162588` is a task branch
   and the `PUBLISH` guard stopped it moving any tracked tag, so no machine in
   the field can fetch an attestation yet. Setting `enforce` before then refuses
   every update on every machine.
3. **Do NOT edit the "no published APEX image has an SBOM attestation yet"
   sentences** in `docs/trust-enforcement.md` or `apexd/apex/src/verify.rs`.
   Still true for the same reason. The first green `main` build is what makes
   them false, and that build's author should be the one to change them.
4. **When that `main` build lands**, `apexd/apex/src/verify.rs::verify_provenance`
   meets a real attestation for the first time. It has never been exercised
   against one — its own comments say so — and it will now download ~18.8 MB and
   hold roughly three copies in memory (see section 4). Worth a look then; it is
   not a defect today.

### Watch item, not a defect

74% of the ceiling is not generous, and growth is package-count driven — the
Electron bundles are the volatile part. The guard warns at 80% and fails at
100%, so this announces itself. When it warns, section 10 has the measured byte
breakdown and the two refused options with their costs.

### One thing NOT built, deliberately

No gate on the 21,000-character run-block cap that cost one dispatch. The
failure is loud and immediate — a dispatch returns HTTP 422 and a push leaves a
jobless `completed/failure` run — and the repo has no natural home for a
workflow lint. Recorded so the choice is visible rather than an oversight.

## BLOCKED ON

- nothing. Done.
