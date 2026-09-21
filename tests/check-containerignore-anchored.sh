#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-containerignore-anchored.sh — a build context that drops source.
#
#  `.containerignore` carried `**/target/`. That is the obvious way to keep
#  cargo build output out of the context, and it also matches
#  `apexd/apex-backup-core/src/target/`, which is SOURCE: the backup
#  destinations. `COPY apexd/` then delivered a crate whose lib.rs declares
#  `pub mod target;` with no such file, and every image build died in `base`:
#
#      error[E0583]: file not found for module `target`
#
#  Nothing else could have caught it. The file is committed, the gitignore is
#  correct, the build is green locally where the context is not used the same
#  way, and the compiler error names a module rather than a missing file.
#
#  .gitignore had the identical bug first with a bare `target/`. Two layers,
#  one mistake, so this gate covers the class rather than the instance:
#
#    1. no unanchored `target` pattern may appear in .containerignore;
#    2. every path it ignores must be a real cargo output directory, i.e. sit
#       next to a Cargo.toml;
#    3. and the real test — no file tracked by git under a crate's src/ may be
#       excluded by the context rules. That one would catch a future pattern
#       nobody predicted.
#
#  Both arms fail: a bad pattern reddens it, and a clean tree passes.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
F=.containerignore
rc=0
pass() { printf '  PASS  %s\n' "$1"; }
fail() { printf '  FAIL  %s — %s\n' "$1" "$2"; rc=1; }

[ -f "$F" ] || { echo "FATAL: no $F"; exit 2; }

# ── 1. no unanchored target pattern ─────────────────────────────────────────
bad="$(grep -nE '^\s*(\*\*/)?target/?\s*$|^\s*\*\*/target' "$F" || true)"
if [ -n "$bad" ]; then
    fail "no unanchored target pattern" "$(echo "$bad" | tr '\n' ' ')"
else
    pass "no unanchored target pattern"
fi

# ── 2. every ignored path is a real cargo output directory ──────────────────
unknown=""
while IFS= read -r p; do
    case "$p" in ''|\#*|.git/) continue ;; esac
    d="${p%/target/}"; d="${d%/target-clippy/}"
    [ "$d" = "$p" ] && continue          # not a target rule; leave it alone
    [ -f "$d/Cargo.toml" ] || unknown="$unknown $p"
done < "$F"
if [ -n "$unknown" ]; then
    fail "every ignored target sits next to a Cargo.toml" "no crate at:$unknown"
else
    pass "every ignored target sits next to a Cargo.toml"
fi

# ── 3. the real test: no tracked source is excluded from the context ────────
# Ask git what it tracks under any crate's src/, then ask whether the context
# rules would drop it. Anything dropped is a file the build cannot see.
command -v git >/dev/null 2>&1 || { echo "FATAL: git is needed to enumerate tracked source"; exit 2; }
dropped=""
while IFS= read -r f; do
    while IFS= read -r p; do
        case "$p" in ''|\#*) continue ;; esac
        case "$p" in
            '**/'*) pat="${p#\*\*/}"; case "/$f" in */"${pat%/}"/*) dropped="$dropped $f";; esac ;;
            */)     case "$f" in "${p}"*) dropped="$dropped $f";; esac ;;
        esac
    done < "$F"
done < <(git ls-files '*/src/*.rs' 2>/dev/null)
if [ -n "$dropped" ]; then
    fail "no tracked source is excluded from the build context" \
         "$(echo "$dropped" | tr ' ' '\n' | sort -u | head -5 | tr '\n' ' ')…"
else
    pass "no tracked source is excluded from the build context"
fi

[ "$rc" -eq 0 ] && echo "  the build context carries every tracked source file"
exit "$rc"
