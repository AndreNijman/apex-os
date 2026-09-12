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
#  Measured rather than argued: `fn f(v: &Vec<String>) -> usize { v.len() }` in
#  apex-agent-core fails this script with clippy::ptr_arg (exit 101) and builds
#  clean under `RUSTFLAGS="-D warnings" cargo build --locked` (exit 0).
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
#
# --network=host because `docker.io/library/rust:1` does NOT ship clippy: it
# downloads it, and on katana a container without this gets no name resolution.
# The image pull works either way, since that goes through the host, so the
# failure only appears at `rustup component add` — which used to have its stderr
# discarded, turning "Temporary failure in name resolution" into "could not add
# the clippy component" and reading like a broken image. Both halves of that are
# fixed here: the network, and saying what actually went wrong.
podman run --rm --network=host -v "$WORK:/w:z" -w /w/apexd \
    -e CARGO_TARGET_DIR=/w/target-clippy \
    docker.io/library/rust:1 \
    sh -c 'rustup component add clippy || { echo "could not add the clippy component" >&2; exit 2; }
           cargo clippy --locked --all-targets -- -D warnings'
rc=$?
[ "$rc" = 0 ] && echo "PASS  clippy is clean" || echo "FAIL  clippy exited $rc"
exit "$rc"
