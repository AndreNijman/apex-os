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

- **`35455459788`** — `sbom-probe` on `task/sbom-attest`, dispatched
  2026-09-19T16:35Z by pushing `4f308f82`. ~40-60 min. ONE runner, not a build:
  it re-catalogues the digest the failing build already pushed
  (`sha256:86225e1b…`). Read it with
  `gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs` (`gh run view --log`
  refuses while a run is in progress). Grep the log for `RESULT ` and `REKOR `:
  every number the decision turns on is printed on one of those lines.
  It also re-confirms the 24 MiB ceiling **from a GitHub runner**, so the
  bisection above does not rest on one home connection.

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
