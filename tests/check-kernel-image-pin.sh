#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-kernel-image-pin.sh — is `core` actually pointed at a kernel that
#  exists, pinned to the exact one whose BTF was checked, with no way to build
#  without it?
#
#  WHY THIS FILE EXISTS. `Containerfile.core` consumes the kernel tier as
#  `FROM ${APEX_KERNEL_IMAGE}`. That one line has already failed in two
#  different ways, each of which stopped EVERY image build in the project and
#  neither of which any test in this repository could see:
#
#    1. The ARG defaulted to `localhost/apex-kernel:local` — an image that
#       exists only on a machine somebody has hand-built a kernel on. CI is not
#       such a machine. Run 35552604603 on roadmap/v2.2:
#       `Error: determining starting point for build: no FROM statement found`.
#    2. The ARG was declared INSIDE an earlier stage. Buildah and Docker make an
#       ARG visible to a stage's own FROM line only when it was declared before
#       the FIRST FROM in the whole file; a stage-local one is invisible to
#       every FROM, including its own. So the reference expanded EMPTY on every
#       machine, with or without a local kernel image, and `--build-arg` could
#       not fix it because the override had nothing to attach to. Same message.
#
#  Both are one `grep` to notice and forty-five minutes of build to discover.
#
#  It also asserts the two things that make the pin mean something:
#
#    · The default is a DIGEST, not a floating tag. The kernel is the most
#      security-sensitive thing in the image, and `core` refusing a kernel whose
#      manifest does not say `btf_scx=usable` is worth much less if the image
#      behind the name can be replaced after that check was made.
#    · There is NO FALLBACK. A `core` that quietly installed the COPR kernel
#      when the published one was unreachable would undo the entire reason the
#      kernel tier exists — and the BTF defect it fixes is invisible until a
#      sched-ext scheduler fails to load, months later, on a user's machine.
#
#  Static, offline, and takes milliseconds: it reads files, builds nothing and
#  talks to no registry. Whether the pinned digest is actually REACHABLE is a
#  different question, asked by build-image.yml's core job before it spends an
#  hour — see the "resolve the kernel tier" step there.
#
#      ./tests/check-kernel-image-pin.sh
# ─────────────────────────────────────────────────────────────────────────────
# NOT `set -e`: every check below should run, so one failure reports one line
# instead of hiding the other six.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

CF=Containerfile.core
BL=build-local.sh
WF=.github/workflows/kernel-build.yml

# The registry repository the kernel is published to. Every tier shares one
# GHCR repository distinguished by tag (see build-image.yml's `IMAGE`), which is
# also why publishing the kernel needed no new package and no new permission.
IMAGE_REPO='ghcr.io/andrenijman/apex-os'

# The name a LOCAL kernel build produces. This must keep working: it is how the
# kernel is developed, and build-local.sh passes it explicitly.
LOCAL_IMG='localhost/apex-kernel:local'

fail=0
# `err` counts a problem; `hint` explains one already counted. Without the
# split, a six-line explanation reported six failures.
err()  { echo "FAIL: $*" >&2; fail=$((fail + 1)); }
hint() { echo "      $*" >&2; }
ok()   { echo "ok: $*"; }

for f in "$CF" "$BL" "$WF"; do
    [ -f "$f" ] || { echo "FATAL: $f is missing — run this from the repo"; exit 1; }
done

# ── 1. the ARG exists exactly once, and before the first FROM ───────────────
# `grep -c` rather than a pipeline: `something | grep -q` under `pipefail`
# returns 141 on a match, because grep exits at the first hit and the writer
# takes SIGPIPE. That has mis-seeded suites in this repository before, so every
# grep here reads a FILE.
arg_count=$(grep -cE '^ARG[[:space:]]+APEX_KERNEL_IMAGE=' "$CF")
if [ "$arg_count" -ne 1 ]; then
    err "$CF has $arg_count \`ARG APEX_KERNEL_IMAGE=\` declarations, expected exactly 1"
else
    ok "$CF declares APEX_KERNEL_IMAGE once"
fi

arg_line=$(grep -nE '^ARG[[:space:]]+APEX_KERNEL_IMAGE=' "$CF" | head -1 | cut -d: -f1)
from_line=$(grep -nE '^[[:space:]]*FROM[[:space:]]' "$CF" | head -1 | cut -d: -f1)

if [ -z "$from_line" ]; then
    err "$CF has no FROM line at all"
elif [ -z "$arg_line" ]; then
    err "$CF never declares ARG APEX_KERNEL_IMAGE"
elif [ "$arg_line" -ge "$from_line" ]; then
    err "$CF declares APEX_KERNEL_IMAGE at line $arg_line, AFTER the first FROM at line $from_line."
    hint "A stage-local ARG is invisible to every FROM, including its own, so"
    hint "\`FROM \${APEX_KERNEL_IMAGE}\` would expand empty and the whole build would"
    hint "die with 'determining starting point for build: no FROM statement found'."
    hint "--build-arg does NOT fix this. Move the declaration above line $from_line."
else
    ok "APEX_KERNEL_IMAGE is declared at line $arg_line, before the first FROM at line $from_line"
fi

# ── 2. the default is a published digest, not a local name and not a tag ────
raw=$(sed -nE 's/^ARG[[:space:]]+APEX_KERNEL_IMAGE=(.*)$/\1/p' "$CF" | head -1)
# Strip an optional quoting, which the Dockerfile parser also accepts.
default=${raw%\"}; default=${default#\"}

# ...but THIS repository must not use it. build-image.yml's "Resolve the
# kernel tier" step recovers the reference with
#   sed -nE 's|^ARG APEX_KERNEL_IMAGE=(ghcr\.io/…@sha256:[0-9a-f]{64})$|\1|p'
# which is anchored and allows no quotes. A quoted pin therefore builds
# fine locally, passes a gate that strips quotes, and then dies in CI with
# "is not a ghcr.io/andrenijman/apex-os@sha256:<64 hex> digest reference"
# — a message that accuses the digest, which is correct, rather than the
# quotes, which are the actual fault. Found by mutating a passing tree:
# quoting the pin was the one mutant of ten that this gate let through.
# The two parsers must agree, so the stricter one wins.
if [ "$raw" != "$default" ]; then
    err "$CF quotes its APEX_KERNEL_IMAGE default: $raw"
    hint "The Dockerfile parser accepts that, but build-image.yml's resolve step"
    hint "parses this line with an anchored regex that does not. Write it bare:"
    hint "ARG APEX_KERNEL_IMAGE=$default"
fi

if [ -z "$default" ]; then
    err "$CF declares APEX_KERNEL_IMAGE with no default value. CI passes no"
    hint "--build-arg for it, so an empty default is an empty FROM."
elif [ "${default#localhost/}" != "$default" ]; then
    err "$CF defaults APEX_KERNEL_IMAGE to '$default'."
    hint "A localhost/ image exists only on a machine that has hand-built a"
    hint "kernel. CI is not one, and this is exactly the defect that stopped"
    hint "every image build in run 35552604603. The default is for CI and for"
    hint "anyone who has not built a kernel; build-local.sh passes the local"
    hint "name explicitly and is unaffected by what this default says."
elif [[ ! "$default" =~ ^"$IMAGE_REPO"@sha256:[0-9a-f]{64}$ ]]; then
    err "$CF defaults APEX_KERNEL_IMAGE to '$default', which is not a digest"
    hint "reference on $IMAGE_REPO. It must be exactly"
    hint "${IMAGE_REPO}@sha256:<64 hex>, so that the kernel core installs is the"
    hint "one whose BTF the kernel tier actually checked — a floating tag can be"
    hint "repointed after that check and core cannot tell."
    hint "kernel-build.yml prints the exact line to paste in its run summary."
else
    ok "APEX_KERNEL_IMAGE defaults to a digest on $IMAGE_REPO"
fi

# ── 3. the stage is a named FROM, and the RPMs come from it ─────────────────
if grep -qE '^FROM[[:space:]]+\$\{APEX_KERNEL_IMAGE\}[[:space:]]+AS[[:space:]]+kernel-rpms' "$CF"; then
    ok "$CF has \`FROM \${APEX_KERNEL_IMAGE} AS kernel-rpms\`"
else
    err "$CF no longer has \`FROM \${APEX_KERNEL_IMAGE} AS kernel-rpms\`."
    hint "A named stage, not \`COPY --from=\${ARG}\`: an ARG-substituted --from"
    hint "resolves inconsistently across builders and fails as a missing file."
fi

for want in '/rpms' '/manifest'; do
    if grep -qE "^COPY --from=kernel-rpms[[:space:]]+$want([[:space:]]|$)" "$CF"; then
        ok "$CF copies $want out of the kernel stage"
    else
        err "$CF does not \`COPY --from=kernel-rpms $want\` — core would install a"
        hint "kernel it did not get from the kernel tier, or check a manifest it"
        hint "did not get with the RPMs."
    fi
done

# ── 4. no fallback, of either kind ──────────────────────────────────────────
# A repository install of the kernel package is the fallback that must never
# exist: it is how core would silently ship the COPR kernel, with the BTF defect
# the kernel tier exists to fix, while every gate in this repository stayed
# green. The legitimate install is from the copied FILES under /tmp.
bad_install=$(grep -nE 'install[^|&;]*(^|[[:space:]])kernel-cachyos([[:space:]]|$)' "$CF" | grep -v '/tmp/apex-kernel-rpms')
if [ -n "$bad_install" ]; then
    err "$CF installs kernel-cachyos from a repository, not from the kernel tier:"
    printf '%s\n' "$bad_install" | sed 's/^/      /' >&2
    hint "That is a silent fallback to the COPR kernel. There must not be one."
else
    ok "$CF installs the kernel only from the files the kernel stage provided"
fi

# The guard that stops a build with no kernel RPMs must be fatal, not advisory.
if grep -qE 'test -d /tmp/apex-kernel-rpms' "$CF"; then
    ok "$CF asserts the kernel RPMs arrived"
else
    err "$CF no longer checks that /tmp/apex-kernel-rpms exists before installing."
fi
if grep -qE 'test "\$\{BTF\}" = usable' "$CF"; then
    ok "$CF still refuses a kernel whose manifest does not say btf_scx=usable"
else
    err "$CF no longer refuses a kernel whose manifest does not say btf_scx=usable."
    hint "That check is the whole cross-tier contract; without it the BTF gate"
    hint "is advisory, which is the defect the kernel tier exists to prevent."
fi

# ── 5. the LOCAL path still works ───────────────────────────────────────────
# The published default is for CI. Developing the kernel means building it
# locally, and build-local.sh must keep overriding the default with the local
# name — otherwise pinning a digest would quietly make every local core build
# install a kernel from the registry instead of the one just compiled.
if grep -qE "^KERNEL_IMG=$LOCAL_IMG\$" "$BL"; then
    ok "$BL still builds the kernel as $LOCAL_IMG"
else
    err "$BL no longer sets KERNEL_IMG=$LOCAL_IMG"
fi
if grep -qE '[-][-]build-arg APEX_KERNEL_IMAGE="\$KERNEL_IMG"' "$BL"; then
    ok "$BL still passes its locally built kernel to the core build"
else
    err "$BL no longer passes --build-arg APEX_KERNEL_IMAGE=\"\$KERNEL_IMG\"."
    hint "With a published digest as the default, dropping that override makes"
    hint "a local core build silently install the REGISTRY's kernel rather than"
    hint "the one the developer just compiled."
fi

# ── 6. something actually publishes to the place the default points at ──────
if grep -qE "^      IMAGE: $IMAGE_REPO\$" "$WF"; then
    ok "$WF publishes to $IMAGE_REPO"
else
    err "$WF does not publish to $IMAGE_REPO, which is where $CF's default points."
    hint "A default naming a registry nothing pushes to is a build that fails in"
    hint "FROM instead of here."
fi
# The push and the --digestfile are on separate continuation lines, so ask for
# each rather than for one line containing both.
if grep -qE 'podman push ' "$WF" && grep -qE -- '--digestfile' "$WF"; then
    ok "$WF captures the digest of what it pushed"
else
    err "$WF does not capture a --digestfile, so nothing can say which digest to pin."
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "check-kernel-image-pin: $fail problem(s)" >&2
    exit 1
fi
echo "check-kernel-image-pin: the kernel pin is sound"
