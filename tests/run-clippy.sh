#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  run-clippy.sh — the only way to actually run clippy on these machines.
#
#  Neither the L16 nor katana has clippy, and neither has rustup: `cargo clippy`
#  exits with "no such command: `clippy`". That is easy to misread as a broken
#  invocation and easy to work around badly. The tempting substitute,
#  `RUSTFLAGS="-D warnings" cargo build --all-targets`, is strictly weaker — it
#  catches rustc's lints and none of clippy's — and reporting it as clippy is how
#  three status claims in this program came to say something nobody had checked.
#
#  So clippy runs in a container, against a clean checkout, and says so.
#
#  Usage: tests/run-clippy.sh [<git-ref>]      default: the current worktree
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

command -v podman >/dev/null 2>&1 || { echo "podman is not installed; cannot run clippy" >&2; exit 2; }

REF="${1:-}"
if [ -n "$REF" ]; then
    WORK="$(mktemp -d /var/tmp/apex-clippy.XXXXXX)"
    trap 'git worktree remove --force "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
    git worktree add --detach -q "$WORK" "$REF" || exit 2
    echo "clippy: $REF at $(git -C "$WORK" rev-parse --short HEAD), in a container"
else
    WORK="$PWD"
    echo "clippy: this worktree at $(git rev-parse --short HEAD), in a container"
fi

# --all-targets so tests and benches are linted too; -D warnings so a lint is a
# failure rather than a line nobody reads.
podman run --rm -v "$WORK:/w:z" -w /w/apexd \
    -e CARGO_TARGET_DIR=/w/target-clippy \
    docker.io/library/rust:1 \
    sh -c 'rustup component add clippy >/dev/null 2>&1 || { echo "could not add the clippy component" >&2; exit 2; }
           cargo clippy --locked --all-targets -- -D warnings'
rc=$?
[ "$rc" = 0 ] && echo "PASS  clippy is clean" || echo "FAIL  clippy exited $rc"
exit "$rc"
