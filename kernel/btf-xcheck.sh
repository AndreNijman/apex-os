#!/bin/sh
# btf-xcheck.sh — a SECOND, independent reading of the kfunc BTF defect.
#
# WHY A SECOND READER EXISTS AT ALL
#
# `apex-kernel-btf-gate` is the authority: it reuses apexd's kernelbtf.rs, the
# same reader `apex game status` answers from, and it asks the POST-strip
# question — does the published FUNC prototype still carry the implicit
# `struct bpf_prog_aux *`? That is the user-visible symptom.
#
# This script asks the ROOT-CAUSE question with completely different machinery
# (bpftool + sh, not Rust): did pahole emit a `bpf_kfunc` DECL_TAG for the
# function at all? No tag means resolve_btfids' collect_kfuncs() never saw it,
# so btf2btf() never stripped the argument.
#
# The point is that this build gate had never once passed on a real kernel
# before it was written. Every BTF available to test against carried the
# defect. If the authoritative gate ever fails on a kernel we built, "the
# kernel is genuinely broken" and "the reader is wrong about this BTF" look
# identical from one reader. Two readers that disagree is a diagnosis; one
# reader that fails is a mystery, and the mystery costs another 45 minutes.
#
# WHY `*_impl` NAMES ARE EXCLUDED — measured, not inherited
#
# Read on 2026-09-20 from the three blobs in ROADMAP/evidence/
# kernel-build-20260920.md's A/B:
#
#   pahole 1.30 output (pre-resolve_btfids) : 68 scx_bpf_* FUNCs,  0 `*_impl`
#   pahole 1.32 output (pre-resolve_btfids) : 68 scx_bpf_* FUNCs,  0 `*_impl`
#   the kernel's shipped .BTF (post-)       : 97 scx_bpf_* FUNCs, 29 `*_impl`
#
# The 29 `*_impl` twins do not come from pahole. resolve_btfids creates them,
# one per kfunc whose implicit argument it actually stripped. A vmlinux's .BTF
# section is always read after resolve_btfids has run, so those twins are
# always there and are untagged BY CONSTRUCTION. Counting them would report a
# perfectly good kernel as having ~29 untagged kfuncs.
#
# Input: an ELF vmlinux, or a raw .BTF blob. Exit 0 only if every scx_bpf_*
# name carries the tag.

set -eu

BLOB="${1:-}"
if [ -z "${BLOB}" ] || [ ! -s "${BLOB}" ]; then
    echo "usage: btf-xcheck.sh <vmlinux-or-raw-btf>" >&2
    exit 2
fi

command -v bpftool >/dev/null 2>&1 || { echo "btf-xcheck: no bpftool" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

# `bpftool btf dump file` reads a raw blob; for an ELF it needs the section cut
# out first. Try raw, fall back to objcopy, and say which worked.
if bpftool btf dump file "${BLOB}" format raw > "${WORK}/raw" 2>/dev/null \
   && [ -s "${WORK}/raw" ]; then
    echo "btf-xcheck: read ${BLOB} as a raw BTF blob"
else
    command -v objcopy >/dev/null 2>&1 \
        || { echo "btf-xcheck: ${BLOB} is not a raw BTF blob and there is no objcopy" >&2; exit 2; }
    objcopy --dump-section ".BTF=${WORK}/btf" "${BLOB}" /dev/null 2>/dev/null \
        || { echo "btf-xcheck: ${BLOB} has no .BTF section and is not a raw blob" >&2; exit 2; }
    bpftool btf dump file "${WORK}/btf" format raw > "${WORK}/raw" 2>/dev/null \
        || { echo "btf-xcheck: bpftool cannot read the .BTF cut out of ${BLOB}" >&2; exit 2; }
    echo "btf-xcheck: read the .BTF section out of ELF ${BLOB}"
fi

# "<type_id> <name>" for every scx_bpf_* FUNC that is not a resolve_btfids twin.
grep -o "^\[[0-9]*\] FUNC 'scx_bpf_[A-Za-z0-9_]*'" "${WORK}/raw" \
    | sed "s/^\[\([0-9]*\)\] FUNC '\(.*\)'\$/\1 \2/" \
    | grep -v '_impl$' \
    | sort -u > "${WORK}/scx" || true

# Every type id carrying a `bpf_kfunc` DECL_TAG on the function ITSELF.
# component_idx=-1 is the function; >=0 would be one of its parameters.
grep -o "DECL_TAG 'bpf_kfunc' type_id=[0-9]* component_idx=-1" "${WORK}/raw" \
    | grep -o 'type_id=[0-9]*' | cut -d= -f2 | sort -u > "${WORK}/tagged" || true

TWINS="$(grep -c "^\[[0-9]*\] FUNC 'scx_bpf_[A-Za-z0-9_]*_impl'" "${WORK}/raw" || true)"
NTAGS="$(wc -l < "${WORK}/tagged")"

# Dedup by NAME, and count a name as tagged if ANY of its type ids is tagged —
# a static function can appear as several FUNC ids. This is the generous
# reading on purpose: a name reported untagged here has no tag anywhere.
cut -d' ' -f2- "${WORK}/scx" | sort -u > "${WORK}/names"
: > "${WORK}/untagged"
while read -r name; do
    [ -n "${name}" ] || continue
    hit=0
    while read -r id n; do
        [ "${n}" = "${name}" ] || continue
        if grep -qx "${id}" "${WORK}/tagged"; then hit=1; break; fi
    done < "${WORK}/scx"
    [ "${hit}" = 1 ] || echo "${name}" >> "${WORK}/untagged"
done < "${WORK}/names"

NNAMES="$(wc -l < "${WORK}/names")"
NUNTAG="$(wc -l < "${WORK}/untagged")"

echo "btf-xcheck: ${NNAMES} scx_bpf_* names examined (${TWINS} *_impl resolve_btfids twins excluded)"
echo "btf-xcheck: ${NTAGS} bpf_kfunc DECL_TAGs in this BTF"
echo "btf-xcheck: ${NUNTAG} scx_bpf_* names carry NO bpf_kfunc DECL_TAG"

# A BTF with no scx_bpf_* FUNCs at all would otherwise report "0 untagged" and
# read as a pass. That is the "gate that inspects nothing" this repository
# keeps paying for.
if [ "${NNAMES}" -lt 1 ]; then
    echo "btf-xcheck: FAIL — no scx_bpf_* FUNC in this BTF at all." >&2
    echo "            Either sched-ext is not built in, or this reading found nothing" >&2
    echo "            to read. Zero untagged out of zero is not a passing kernel." >&2
    exit 1
fi

if [ "${NUNTAG}" -gt 0 ]; then
    echo "btf-xcheck: FAIL — these kfuncs lost their tag:" >&2
    sed 's/^/              /' "${WORK}/untagged" >&2
    exit 1
fi

echo "btf-xcheck: PASS — every scx_bpf_* name carries the tag"
