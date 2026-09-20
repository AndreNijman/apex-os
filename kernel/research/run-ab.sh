#!/bin/bash
# A/B: does dwarves 1.32 emit the `bpf_kfunc` DECL_TAGs that 1.30 misses?
#
# Runs INSIDE a fedora:43 container. /work is the ab directory, bind-mounted.
#
# Substrate: the vmlinux from Fedora's kernel-debuginfo-7.2.6-100.fc43, which
# ROADMAP/evidence/kernel-btf-scx-20260920.md measured at 18 of 68 affected.
# CachyOS publishes no debuginfo at all (`%define debug_package %{nil}` in its
# spec), so this is the only ready-made DWARF vmlinux for a 7.2.6 kernel.
#
# Three readings, in this order, because the first two decide whether the third
# means anything:
#
#   R0  the vmlinux's OWN shipped .BTF, read for tag presence. Must agree with
#       the 18 the shipped reader found by the independent arg-presence route.
#       Proves this is the same kernel the evidence measured.
#   R1  pahole 1.30 regenerating BTF from that vmlinux's DWARF. Must reproduce
#       roughly R0's untagged set. If it gives 0 or 68 instead, the final
#       vmlinux's already-patched .BTF_ids has confounded the substrate and
#       nothing about 1.32 can be concluded from it.
#   R2  pahole 1.32, same DWARF, with 1.32's own flag set.
#
# The flag sets differ BY VERSION on purpose: scripts/Makefile.btf keys them on
# CONFIG_PAHOLE_VERSION, so 1.32 gets `--btf_features=layout` that 1.30 does
# not. Using one flag set for both would not be what either build does.
set -euo pipefail

KOJI=https://kojipkgs.fedoraproject.org/packages/dwarves
OUT=/work/out
VMLINUX=/work/vmlinux
mkdir -p "$OUT"

# Flags transcribed from scripts/Makefile.btf @ CachyOS/linux cachyos-7.2.6-1
# (saved beside this script as Makefile.btf.cachyos). CONFIG_PAHOLE_HAS_LANG_EXCLUDE=y
# on the shipped kernel, so --lang_exclude=rust is part of both.
COMMON_FEATURES="encode_force,var,float,enum64,decl_tag,type_tag,optimized_func,consistent_func,decl_tag_kfuncs"

flags_for() {
    # $1 = pahole numeric version (130 / 132)
    local v="$1"
    local f="-j12 --btf_features=${COMMON_FEATURES} --lang_exclude=rust"
    [ "$v" -ge 130 ] && f="$f --btf_features=attributes"
    [ "$v" -ge 131 ] && f="$f --btf_features=layout"
    printf '%s' "$f"
}

install_dwarves() {
    # $1 = version dir (1.30), $2 = release dir (2.fc43), $3 = nvr (1.30-2.fc43)
    local vd="$1" rd="$2" nvr="$3"
    echo "--- installing dwarves ${nvr} from koji (pinned NVR, permanent URL)"
    dnf5 -y --disablerepo='*' install \
        "${KOJI}/${vd}/${rd}/x86_64/dwarves-${nvr}.x86_64.rpm" \
        "${KOJI}/${vd}/${rd}/x86_64/libdwarves1-${nvr}.x86_64.rpm" >/dev/null
    pahole --version
}

run_pahole() {
    # $1 = label (130/132)
    local v="$1"
    local blob="${OUT}/btf-${v}.bin"
    local flags; flags="$(flags_for "$v")"
    echo "--- pahole ${v}: -J ${flags} --btf_encode_detached=${blob}"
    # shellcheck disable=SC2086
    /usr/bin/time -v pahole -J ${flags} --btf_encode_detached="${blob}" "$VMLINUX" \
        2> "${OUT}/pahole-${v}.time" || {
            echo "FATAL: pahole ${v} failed"; tail -30 "${OUT}/pahole-${v}.time"; exit 1; }
    grep -E 'Elapsed \(wall|Maximum resident' "${OUT}/pahole-${v}.time" || true
    ls -l "${blob}"
    bpftool btf dump file "${blob}" format raw > "${OUT}/dump-${v}.txt"
    python3 /work/kfunc-tags.py "${OUT}/dump-${v}.txt" > "${OUT}/tags-${v}.json"
    python3 - "${OUT}/tags-${v}.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(f"  scx_bpf_* examined={d['scx_bpf_examined']} "
      f"tagged={d['scx_bpf_tagged']} UNTAGGED={d['scx_bpf_untagged']}")
print(f"  bpf_kfunc DECL_TAGs total={d['decl_tag_values'].get('bpf_kfunc',0)} "
      f"_impl twins={d['impl_twins_skipped']}")
PY
}

echo "=============== R0: the vmlinux's own shipped .BTF ==============="
dnf5 -y install bpftool binutils python3 time >/dev/null
objcopy --dump-section .BTF="${OUT}/btf-shipped.bin" "$VMLINUX" /dev/null
ls -l "${OUT}/btf-shipped.bin"
bpftool btf dump file "${OUT}/btf-shipped.bin" format raw > "${OUT}/dump-shipped.txt"
python3 /work/kfunc-tags.py "${OUT}/dump-shipped.txt" > "${OUT}/tags-shipped.json"
python3 - "${OUT}/tags-shipped.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(f"  scx_bpf_* examined={d['scx_bpf_examined']} "
      f"tagged={d['scx_bpf_tagged']} UNTAGGED={d['scx_bpf_untagged']}")
print("  untagged:", ", ".join(d["untagged_names"]))
PY

echo "=============== R1: pahole 1.30 (the version that built it) ==============="
install_dwarves 1.30 2.fc43 1.30-2.fc43
run_pahole 130

echo "=============== R2: pahole 1.32 (the candidate fix) ==============="
dnf5 -y remove dwarves libdwarves1 >/dev/null 2>&1 || true
install_dwarves 1.32 1.fc43 1.32-1.fc43
run_pahole 132

echo "=============== VERDICT ==============="
python3 - "${OUT}/tags-shipped.json" "${OUT}/tags-130.json" "${OUT}/tags-132.json" <<'PY'
import json, sys
s, a, b = (json.load(open(p)) for p in sys.argv[1:4])
print(f"shipped .BTF   : {s['scx_bpf_untagged']:3d} untagged of {s['scx_bpf_examined']}")
print(f"pahole 1.30    : {a['scx_bpf_untagged']:3d} untagged of {a['scx_bpf_examined']}")
print(f"pahole 1.32    : {b['scx_bpf_untagged']:3d} untagged of {b['scx_bpf_examined']}")
print()
su, au, bu = set(s['untagged_names']), set(a['untagged_names']), set(b['untagged_names'])
print(f"1.30 vs shipped: same={len(au & su)} only-in-1.30={sorted(au - su)} only-in-shipped={sorted(su - au)}")
print(f"fixed by 1.32  : {sorted(au - bu)}")
print(f"still broken   : {sorted(bu)}")
print(f"NEW in 1.32    : {sorted(bu - au)}")
PY
echo "=============== done ==============="
