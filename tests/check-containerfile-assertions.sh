#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-containerfile-assertions.sh — run the image build's own grep
#  assertions against this repository, before a 50-minute build runs them.
#
#  Four separate assertions on roadmap/v2.2 could never pass or never run, and
#  each one was found the same way: by a `base` job failing at a later step than
#  the last, 50 minutes at a time.
#
#      706489ec  ! grep -q '_keep' … matched the policy file's OWN comment
#      e24cd244  nft -c could not reach netlink in a build container at all
#      43d98f10  ! grep 'mouse = true' matched the three comment lines that
#                explain why `{ mouse = true }` is wrong
#      2e33c1a6  grep 'ApexShellKeybinds.${_variant}' — a draft spelling of a
#                variable that appears nowhere in the file it greps
#
#  All four are the same defect: an assertion nobody has ever watched go red.
#  A Containerfile assertion is the one kind of test that cannot be run by
#  running the test suite, because it only exists inside a build — so this
#  resolves each one back through the Containerfile's own COPY lines to the
#  file in this repo that becomes it, and runs it here.
#
#  It is honest about what it cannot reach. Many stanzas grep a path that only
#  exists after a `sed`, or inside the vendored apex-shell clone, or through a
#  variable built at build time. Those are reported as UNRESOLVED and counted
#  separately — never as passes. "Could not check" is not "fine".
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

FILES=(Containerfile.base Containerfile.core)
[ "$#" -gt 0 ] && FILES=("$@")

python3 - "${FILES[@]}" <<'PY'
import re, subprocess, os, shlex, sys

failures = 0
checked = 0
unresolved = 0
unresolved_examples = []

for path in sys.argv[1:]:
    if not os.path.exists(path):
        print(f"SKIP  {path} does not exist")
        continue
    lines = open(path).read().split('\n')

    # The file's own COPY map, image path -> repo path.
    copies = []
    for l in lines:
        m = re.match(r'^COPY\s+(?:--\S+\s+)*(\S+)\s+(\S+)\s*$', l)
        if m:
            copies.append((m.group(1), m.group(2)))

    def to_repo(img):
        """Every repo path that becomes `img` in the built image.

        A grep target can be a directory ABOVE the COPY destinations that fill
        it -- `/usr/share/apex/hypr/` is filled by two separate COPY lines --
        and that is exactly the shape 43d98f10 had, so resolving only the
        exact and inside-a-COPY cases would miss the defect this file exists
        to catch."""
        best = None
        for src, dst in copies:
            if img == dst:
                return [src]
            if dst.endswith('/') and img.startswith(dst):
                cand = src.rstrip('/') + '/' + img[len(dst):]
                if best is None or len(dst) > len(best[0]):
                    best = (dst, cand)
        if best:
            return [best[1]]
        if img.endswith('/'):
            under = [src for src, dst in copies if dst.startswith(img)]
            if under:
                return under
        return None

    # Walk stanza by stanza so a `f=/usr/libexec/...` assignment is in scope for
    # the greps below it. That is not a nicety: the fourth defect was found in a
    # stanza that greps "$f", so a resolver that only understood literal paths
    # would have missed exactly the one this file exists to catch.
    i = 0
    while i < len(lines):
        if not lines[i].startswith('RUN '):
            i += 1
            continue
        # A comment line inside a continued RUN does NOT end with a backslash --
        # buildah strips comments before joining -- so a walker that ends the
        # stanza at the first line without one stops at the first comment and
        # silently drops every assertion after it. That is how the first draft
        # of this file passed while blind to two of the four defects it was
        # written for: both sit under an explanatory comment. Only a non-comment
        # line without a continuation ends a stanza.
        stanza = []
        j = i
        while True:
            stanza.append((j + 1, lines[j]))
            stripped = lines[j].strip()
            is_comment = stripped.startswith('#')
            if not is_comment and not lines[j].rstrip().endswith('\\'):
                break
            j += 1
            if j >= len(lines):
                break
            # A blank line, or the next instruction, really does end it.
            if not lines[j].strip() or re.match(r'^[A-Z]+\s', lines[j]):
                break
        i = j + 1

        env = {}
        # `for m in hyprland niri labwc; do ... grep "^apex(\"${m}\")" ...` --
        # a pattern built from a loop variable is not a literal and cannot be
        # checked from here. It is UNRESOLVED, not a failure: reporting it as a
        # defect would be this file making the same mistake it exists to catch.
        loopvars = set()
        for _, raw in stanza:
            for lm in re.finditer(r'\bfor\s+([A-Za-z_][A-Za-z0-9_]*)\s+in\s', raw):
                loopvars.add(lm.group(1))
        for lineno, raw in stanza:
            s = raw.strip()
            if s.startswith('RUN '):
                s = s[4:].strip()
            s = s.rstrip('\\').strip()
            if s.startswith('#') or not s:
                continue

            # VAR=/usr/... or VAR="/usr/..."   (one per physical line, which is
            # how this file writes them)
            m = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)=("?)(/(?:usr|etc)/[^"\s;]+)\2\s*;?\s*$', s)
            if m:
                env[m.group(1)] = m.group(3)
                continue

            m = re.match(r'^(!\s*)?grep\s+(.*)$', s)
            if not m:
                continue
            negated = bool(m.group(1))
            try:
                toks = shlex.split('grep ' + m.group(2))
            except ValueError:
                unresolved += 1
                unresolved_examples.append(f"{path}:{lineno} unparseable")
                continue
            toks = toks[1:]
            # Drop a trailing `; \` fragment shlex kept as its own token.
            toks = [t for t in toks if t not in (';', '\\')]

            flags = [t for t in toks if t.startswith('-') and len(t) > 1]
            rest = [t for t in toks if t not in flags]
            if len(rest) < 2:
                continue
            pattern, target = rest[0], rest[-1].rstrip(';')

            # Resolve the target: a literal image path, or a variable the
            # stanza set to one.
            if any(re.search(r'\$\{?' + re.escape(v) + r'\}?', pattern) for v in loopvars):
                unresolved += 1
                unresolved_examples.append(f"{path}:{lineno} pattern is built from a loop variable")
                continue

            img = None
            vm = re.match(r'^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$', target)
            if vm and vm.group(1) in env:
                img = env[vm.group(1)]
            elif target.startswith('/usr/') or target.startswith('/etc/'):
                img = target

            if img is None:
                unresolved += 1
                unresolved_examples.append(f"{path}:{lineno} target {target}")
                continue
            repos = to_repo(img if img.endswith('/') else img.rstrip('/'))
            repos = [r for r in (repos or []) if os.path.exists(r)]
            if not repos:
                unresolved += 1
                unresolved_examples.append(f"{path}:{lineno} {img} has no COPY source here")
                continue

            gflags = ['-q']
            if any(os.path.isdir(r) for r in repos):
                gflags.append('-r')
            for f in flags:
                if f.startswith('-') and not f.startswith('--'):
                    for ch in f[1:]:
                        if ch in 'EFiwxr':
                            gflags.append('-' + ch)
            r = subprocess.run(['grep'] + gflags + [pattern] + repos, capture_output=True)
            matched = (r.returncode == 0)
            checked += 1
            if os.environ.get('APEX_CF_VERBOSE'):
                print(f"  check {path}:{lineno} {'!' if negated else ' '}grep {pattern!r} -> "
                      f"{'match' if matched else 'no match'} in {' '.join(repos)}")
            want = (not negated)
            if matched != want:
                failures += 1
                if negated:
                    print(f"FAIL  {path}:{lineno}")
                    print(f"      ! grep {pattern!r} is supposed to find NOTHING, and it matches {' '.join(repos)}")
                    m2 = subprocess.run(['grep', '-n'] + [g for g in gflags if g != '-q'] + [pattern] + repos,
                                        capture_output=True, text=True)
                    for x in m2.stdout.strip().split('\n')[:3]:
                        print(f"          {x[:150]}")
                    print(f"      This assertion can only ever FAIL the build.")
                else:
                    print(f"FAIL  {path}:{lineno}")
                    print(f"      grep {pattern!r} finds nothing in {' '.join(repos)}")
                    print(f"      This assertion can only ever FAIL the build.")

print()
print(f"containerfile assertions: {checked} checked, {failures} failed, {unresolved} could not be resolved")
if unresolved:
    print("  UNRESOLVED (not checked, and not a pass — these run only inside the build):")
    for e in unresolved_examples[:12]:
        print(f"    {e}")
    if len(unresolved_examples) > 12:
        print(f"    … and {len(unresolved_examples) - 12} more")
sys.exit(1 if failures else 0)
PY
