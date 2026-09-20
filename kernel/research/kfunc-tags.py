#!/usr/bin/env python3
"""Count scx_bpf_* kfuncs that carry a `bpf_kfunc` DECL_TAG in a raw BTF blob.

This is DELIBERATELY a different reading from apexd's kernelbtf.rs.

kernelbtf.rs asks the POST-resolve_btfids question: does the published FUNC
prototype still carry the implicit `struct bpf_prog_aux *`? That is the
user-visible symptom.

This script asks the PRE-resolve_btfids question: did pahole emit a
`bpf_kfunc` DECL_TAG for this FUNC at all? That is the ROOT CAUSE -- no tag
means resolve_btfids' collect_kfuncs() never sees the function, so btf2btf()
never strips the argument.

Before resolve_btfids has run there is nothing to strip yet, so the shipped
reader cannot be used on a freshly-pahole'd blob. Two readings, two stages of
one pipeline; they are not two implementations of the same check.

Input is `bpftool btf dump file <blob> format raw`.
"""
import re
import sys
import json

FUNC_RE = re.compile(r"^\[(\d+)\]\s+FUNC\s+'([^']+)'")
TAG_RE = re.compile(
    r"^\[(\d+)\]\s+DECL_TAG\s+'([^']+)'\s+type_id=(\d+)\s+component_idx=(-?\d+)"
)

# Same exclusion the shipped reader makes, and for the same reason:
# `scx_bpf_dsq_insert_impl` is the pre-strip original that resolve_btfids
# renames. It is not a kfunc a BPF program references.
IMPL_SUFFIX = "_impl"


def main(path: str) -> int:
    funcs = {}          # type_id -> name
    tagged = set()      # type_id carrying a bpf_kfunc DECL_TAG
    tag_values = {}     # tag value -> count

    with open(path, "r", errors="replace") as fh:
        for line in fh:
            m = FUNC_RE.match(line)
            if m:
                funcs[int(m.group(1))] = m.group(2)
                continue
            m = TAG_RE.match(line)
            if m:
                value, tid, comp = m.group(2), int(m.group(3)), int(m.group(4))
                tag_values[value] = tag_values.get(value, 0) + 1
                # component_idx == -1 means the tag applies to the function
                # itself rather than to one of its parameters.
                if value == "bpf_kfunc" and comp == -1:
                    tagged.add(tid)

    scx = {
        tid: name
        for tid, name in funcs.items()
        if name.startswith("scx_bpf_") and not name.endswith(IMPL_SUFFIX)
    }
    # De-duplicate by NAME: a name can appear as more than one FUNC type id
    # (static duplicates across translation units). A name counts as tagged if
    # ANY of its type ids carries the tag -- this is the generous reading, so a
    # name reported as untagged really has no tag anywhere.
    by_name = {}
    for tid, name in scx.items():
        by_name.setdefault(name, []).append(tid)

    untagged = sorted(n for n, tids in by_name.items()
                      if not any(t in tagged for t in tids))
    ok = sorted(n for n, tids in by_name.items()
                if any(t in tagged for t in tids))

    impl = sorted({n for n in funcs.values()
                   if n.startswith("scx_bpf_") and n.endswith(IMPL_SUFFIX)})

    out = {
        "btf_dump": path,
        "total_funcs": len(funcs),
        "decl_tag_values": tag_values,
        "scx_bpf_examined": len(by_name),
        "scx_bpf_tagged": len(ok),
        "scx_bpf_untagged": len(untagged),
        "untagged_names": untagged,
        "impl_twins_skipped": len(impl),
    }
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: kfunc-tags.py <bpftool-raw-dump>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
