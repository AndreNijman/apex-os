#!/usr/bin/env python3
"""Set a task's status and evidence in roadmap.yaml without reformatting the file.

Editing the yaml by hand kept breaking it: an unquoted colon inside an evidence
string is a mapping, not text. This writes evidence as a folded block scalar so
punctuation cannot reopen that wound, and it re-parses the file afterwards so a
bad edit fails here instead of in the next session.

    ./set-status.py P0-001 partial "probed both machines, see evidence/..."
    ./set-status.py BASE-007 done "cargo test 1075 passed; blueprint harness 140/0"

Evidence may also be passed as --evidence TEXT. It once could not: the flag was
read as the evidence itself, and on 2026-09-07 thirty-three tasks were found
holding the literal string "--evidence" where their evidence belonged. The text
after it had been silently dropped. Both spellings work now, and a lone flag
name is refused rather than stored.
"""
import re
import sys
import textwrap

VALID = {"verify_existing", "partial", "todo", "blocked", "done"}
PATH = "/var/home/andre/Projects/apex/ROADMAP/roadmap.yaml"


def block(text, indent="    "):
    """Render text as the body of a `>-` folded scalar."""
    # A folded scalar joins lines with spaces, so a stray colon is safe, but a
    # line starting with a list marker or ending in a colon still is not.
    flat = " ".join(text.split())
    return "\n".join(indent + line for line in textwrap.wrap(flat, 110))


def main():
    args = sys.argv[1:]
    evidence = None
    # --evidence TEXT and --evidence=TEXT, so the flag spelling cannot be
    # stored as the evidence. See the module docstring for why this is here.
    rest = []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--evidence":
            if i + 1 >= len(args):
                sys.exit("--evidence needs a value")
            evidence = args[i + 1]
            i += 2
            continue
        if a.startswith("--evidence="):
            evidence = a.split("=", 1)[1]
            i += 1
            continue
        rest.append(a)
        i += 1
    if len(rest) < 2:
        sys.exit(__doc__)
    tid, status = rest[0], rest[1]
    if evidence is None and len(rest) > 2:
        evidence = rest[2]
    if status not in VALID:
        sys.exit(f"status must be one of {sorted(VALID)}")
    if evidence is not None and evidence.strip().startswith("--"):
        sys.exit(f"refusing to store {evidence.strip()!r} as evidence")
    if evidence is not None and not evidence.strip():
        sys.exit("refusing to store empty evidence")

    src = open(PATH).read()
    m = re.search(rf"^- id: {re.escape(tid)}\n((?:(?!^- id: ).*\n)*)", src, re.M)
    if not m:
        sys.exit(f"{tid} not found")
    body = m.group(1)
    new = body

    if re.search(r"^  status: .*$", new, re.M):
        new = re.sub(r"^  status: .*$", f"  status: {status}", new, count=1, flags=re.M)
    else:
        sys.exit(f"{tid} has no status line")

    if evidence:
        rendered = f"  evidence: >-\n{block(evidence)}\n"
        # Drop an existing evidence key (scalar or block) before adding ours.
        new = re.sub(r"^  evidence:.*\n(?:    .*\n)*", "", new, flags=re.M)
        new = new.rstrip("\n") + "\n" + rendered

    out = src[: m.start(1)] + new + src[m.end(1) :]

    import yaml

    try:
        parsed = yaml.safe_load(out)
    except yaml.YAMLError as e:
        sys.exit(f"refusing to write, result does not parse: {e}")
    ids = [t["id"] for t in parsed["tasks"]]
    if len(ids) != len(set(ids)):
        sys.exit("refusing to write, duplicate task ids")

    open(PATH, "w").write(out)
    got = [t for t in parsed["tasks"] if t["id"] == tid][0]
    print(f"{tid} -> {got['status']}  ({len(ids)} tasks, parses clean)")


if __name__ == "__main__":
    main()
