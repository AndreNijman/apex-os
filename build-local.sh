#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  build-local.sh — build APEX-OS images locally, with the kernel signed.
#
#  WHY THIS EXISTS. Containerfile.core signs the CachyOS kernel with the APEX
#  MOK only when the key is mounted as a build secret:
#
#     podman build --secret id=apex_sb_key,src=… --secret id=apex_sb_crt,src=…
#
#  and when the secret is absent it stamps the image `unsigned` and carries on
#  by design, so local builds keep working for people without the key. The
#  consequence is that forgetting the flag produces an image that looks fine,
#  builds green, and cannot be used with Secure Boot — with nothing but one log
#  line saying so. Nothing in the repo passed the flag; it was done by hand
#  every time, which is a coin flip nobody should be asked to keep winning.
#
#  This script passes it, and REFUSES to produce an unsigned image unless you
#  explicitly ask for one with --allow-unsigned.
#
#  Usage:
#     ./build-local.sh                 core + base + apex, signed
#     ./build-local.sh base            just the base (reuses the existing core)
#     ./build-local.sh kernel          just the kernel tier (the ~45 min compile)
#     ./build-local.sh apex            just the image tier
#     ./build-local.sh --allow-unsigned base       no key needed
#     ./build-local.sh --force-core                rebuild core even if present
#
#  ONE IMAGE. `daily`, `gaming-mesa` and `gaming-nvidia` are gone as build
#  targets; there is a single image and the three names survive only as published
#  tags pointing at it. They are still accepted here and map to `apex`, so a
#  habit or a stale script does not fail with "unknown target".
#
#  CORE vs BASE. The image is built in three tiers (see Containerfile.core's
#  header): `core` is the slow-moving ~45 min foundation, `base` is the thin
#  per-commit tier on top of it, and the image tier comes last. Core is REUSED
#  when it already exists locally, because rebuilding it is both slow and — on a
#  published image — a multi-gigabyte download for every machine on the fleet.
#  Pass --force-core when you actually mean to move it.
#
#  --force-core IS NOT OPTIONAL FOR A GPU OR MODULE CHANGE. The NVIDIA and
#  controller akmods, their MOK signatures and the NVIDIA userspace all live in
#  core now. Reusing an older local core produces an image without them and every
#  check below still passes, which is exactly how a validation build in this
#  project reported green against an artifact that did not contain the change.
#
#  Key location: ~/.apex-signing/apex-mok.{key,crt}, overridable with
#  APEX_SIGNING_DIR. The key is never copied, never committed, and never enters
#  the image — --secret is a tmpfs mount that leaves no layer behind.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")"

SIGNDIR="${APEX_SIGNING_DIR:-$HOME/.apex-signing}"
KEY="$SIGNDIR/apex-mok.key"
CRT="$SIGNDIR/apex-mok.crt"
ALLOW_UNSIGNED=0
FORCE_CORE=0
TARGETS=()

for a in "$@"; do
    case "$a" in
        --allow-unsigned) ALLOW_UNSIGNED=1 ;;
        --force-core) FORCE_CORE=1 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown option: $a" >&2; exit 2 ;;
        *) TARGETS+=("$a") ;;
    esac
done
[ "${#TARGETS[@]}" -gt 0 ] || TARGETS=(core base apex)

# ── The signing key ──────────────────────────────────────────────────────────
SECRET_ARGS=()
if [ -s "$KEY" ] && [ -s "$CRT" ]; then
    # Validate before building rather than discovering at sbsign time, an hour
    # into a base build.
    openssl rsa  -in "$KEY" -noout -check >/dev/null 2>&1 \
        || { echo "FATAL: $KEY is not a valid RSA private key"; exit 1; }
    openssl x509 -in "$CRT" -noout >/dev/null 2>&1 \
        || { echo "FATAL: $CRT is not a valid certificate"; exit 1; }
    k=$(openssl rsa  -in "$KEY" -noout -modulus | sha256sum)
    c=$(openssl x509 -in "$CRT" -noout -modulus | sha256sum)
    [ "$k" = "$c" ] || { echo "FATAL: $KEY and $CRT are not a matching pair"; exit 1; }
    SECRET_ARGS=(--secret "id=apex_sb_key,src=$KEY" --secret "id=apex_sb_crt,src=$CRT")
    echo "signing key: $KEY (validated)"
elif [ "$ALLOW_UNSIGNED" = 1 ]; then
    echo "WARNING: building UNSIGNED — the result cannot be used with Secure Boot."
else
    cat >&2 <<EOF
FATAL: no signing key at $SIGNDIR

Containerfile.core would silently produce an image whose kernel is unsigned,
which cannot boot with Secure Boot on and gives users nothing to enrol. That is
too easy to ship by accident, so this script refuses instead.

Either put apex-mok.key and apex-mok.crt in $SIGNDIR (or set APEX_SIGNING_DIR),
or pass --allow-unsigned if you genuinely want an unsigned image.
EOF
    exit 1
fi

REV="$(git rev-parse HEAD 2>/dev/null || echo unknown)"

CORE_IMG=localhost/apex-os-core:latest
# The kernel is its own tier now (docs/update-cost.md, "The fourth tier").
# Containerfile.core consumes it by name and has NO COPR fallback, so a local
# core build needs this image to exist first.
#
# THIS IS NOT Containerfile.core's DEFAULT any more, and the difference matters.
# The default there is the digest of the kernel published to GHCR by
# kernel-build.yml, because CI has no hand-built kernel image and a default
# naming one stopped every image build in the project. build_core() below
# therefore has to keep passing `--build-arg APEX_KERNEL_IMAGE="$KERNEL_IMG"`:
# drop that override and a local core build would silently install the
# REGISTRY's kernel instead of the one just compiled here.
# tests/check-kernel-image-pin.sh asserts both halves.
KERNEL_IMG=localhost/apex-kernel:local

# ── The shell ref, resolved rather than named ────────────────────────────────
# Containerfile.base defaults APEX_SHELL_REF to `main`, and `git clone --branch
# main` is a cache hit forever: podman cannot know the remote moved, so a local
# build silently vendors whatever apex-shell was at the first build and keeps
# doing so. Observed directly — a base build begun minutes after apex-shell's
# main advanced printed `Using cache` for the clone layer and shipped the old
# shell.
#
# CI does not have this problem because build-image.yml resolves the SHA first
# and passes it, so the build-arg changes whenever the shell does. This does the
# same, which also makes a local build reproduce what CI produces instead of
# something subtly older.
#
# A failure to reach the remote is fatal rather than a fallback to `main`: a
# build that quietly vendors a stale shell is the thing this exists to prevent.
#
# ── and it resolves the MATCHING branch, not `main` ──────────────────────────
#
# This asked for `refs/heads/main` unconditionally, and that is the THIRD
# appearance of one defect; the other two callers had already fixed it and left
# their reasoning in place:
#
#   * build-image.yml `Pin apex-shell` — pinning main "vendored an apex-shell
#     months behind the apex-os being built", and `base` then died in
#     check-labwc-keybinds on W-A-s (screen reader) and W-A-v (voice), two
#     keybinds roadmap/v2.2's rc.xml has and old apex-shell defaults do not
#     generate.
#   * pr-validation.yml — its input-parity check compared apex-os roadmap/v2.2
#     against apex-shell main and reported drift while the two INTEGRATION
#     branches agreed perfectly.
#
# Measured here, 2026-09-19, four ways: apex-os roadmap/v2.2 + apex-shell
# roadmap/v2.2 passes check-labwc-keybinds (70 defaults, 4 skipped); apex-os
# roadmap/v2.2 + apex-shell main fails on exactly those two keybinds. So a local
# build of roadmap/v2.2 could never pass, and the answer is NOT to regenerate
# rc.xml against the older shell — that reverts the accessibility work.
#
# THE MIDDLE RUNG. The chain is want -> roadmap/v2.2 -> main, which is
# pr-validation.yml's three-rung chain rather than build-image.yml's two. That
# is deliberate and the difference matters HERE more than in either workflow:
# build-image.yml only ever runs on a branch that apex-shell also has, while
# this script is run by a human from whatever worktree they are standing in,
# and every `task/*` worktree in this program has no apex-shell twin. A
# two-rung chain would send all of them to `main` and reproduce the exact
# failure above. pr-validation.yml's comment records the same finding.
#
# The `roadmap/v2.2` rung is a PROGRAM-LIFETIME rung, not a permanent one. Once
# v2.2 lands in main and apex-shell deletes the branch, this degrades cleanly to
# want -> main. If apex-shell keeps the branch after the program ends, a feature
# branch cut from a post-program `main` would vendor a stale shell from it —
# delete the rung then. pr-validation.yml carries the identical hazard; this is
# not a new one.
#
# UNREACHABLE IS FATAL, AND IT NOW SAYS SO. The old code could not reach its own
# FATAL. `SHELL_REF="$(git ls-remote … 2>/dev/null | awk …)"` under `set -euo
# pipefail` gives the assignment ls-remote's status (128 for an unreachable
# remote) through pipefail, and errexit kills the script at the assignment —
# before the `[ -n … ] ||` line that carries the message, with git's own error
# thrown away by `2>/dev/null`. Measured: exit 128, not one word printed. So the
# stated property held only by accident of `main` always existing. It is now a
# RETURN CODE decision — 0 found, 2 the remote answered and has no such branch,
# anything else unreachable — and git's stderr is left alone so the user can
# read it.
#
# APEX_SHELL_REMOTE exists so tests/test-build-local-shell-ref.sh can point this
# at local fixture repositories and drive every rung offline. Nothing else
# should set it.
SHELL_REMOTE="${APEX_SHELL_REMOTE:-https://github.com/AndreNijman/apex-shell}"

# Resolve ONE branch on the apex-shell remote.
#   0 -> found; the sha is in SHELL_REF_OUT
#   2 -> the remote answered and has no such branch (try the next rung)
#   unreachable -> FATAL, here, rather than a silent fallback
SHELL_REF_OUT=""
resolve_shell_branch() {  # $1 = branch name
    local out rc=0 sha
    SHELL_REF_OUT=""
    out="$(git ls-remote --exit-code "$SHELL_REMOTE" "refs/heads/$1")" || rc=$?
    case "$rc" in
        0) ;;
        2) return 2 ;;
        *) echo "FATAL: cannot reach apex-shell at $SHELL_REMOTE — git ls-remote exited $rc." >&2
           echo "       Its error is above. A build that quietly vendors a stale shell is" >&2
           echo "       what this refuses to do. Set APEX_SHELL_REF=<sha> to build offline." >&2
           exit 1 ;;
    esac
    out="${out%%$'\n'*}"   # first line
    sha="${out%%$'\t'*}"   # first field
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || {
        echo "FATAL: apex-shell '$1' resolved to '$sha', which is not a 40-hex sha" >&2
        exit 1
    }
    SHELL_REF_OUT="$sha"
}

SHELL_REF="${APEX_SHELL_REF:-}"
if [ -n "$SHELL_REF" ]; then
    echo "== shell == pinned by APEX_SHELL_REF; the remote was not consulted"
else
    # A detached HEAD prints the literal string `HEAD`, which is not a branch
    # name and must not be asked for as one.
    want="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '')"
    [ "$want" = HEAD ] && want=""

    used=""
    tried=()
    for cand in "$want" roadmap/v2.2 main; do
        [ -n "$cand" ] || continue
        case " ${tried[*]-} " in *" $cand "*) continue ;; esac
        tried+=("$cand")
        if resolve_shell_branch "$cand"; then
            used="$cand"; SHELL_REF="$SHELL_REF_OUT"; break
        fi
    done

    [ -n "$used" ] || {
        echo "FATAL: apex-shell has none of the branches ${tried[*]-} — not even main." >&2
        echo "       Set APEX_SHELL_REF=<sha> to build offline." >&2
        exit 1
    }
    if [ "$used" = "$want" ]; then
        echo "== shell == pinned apex-shell branch '$used', which matches this apex-os branch"
    else
        echo "== shell == pinned apex-shell branch '$used' — apex-shell has no branch named '${want:-<detached HEAD>}'"
    fi
fi
echo "== shell == vendoring apex-shell $SHELL_REF"


# Everything the shipped kernel's signature can be checked against. Used after
# core (where signing happens) and after base (which only inherits it).
assert_signed() {  # $1 = image, $2 = label
    [ "${#SECRET_ARGS[@]}" -gt 0 ] || return 0
    local st
    st=$(sudo podman run --rm --entrypoint /bin/sh "$1" \
           -c 'cat /usr/share/apex-os/secureboot/kernel-signed 2>/dev/null || echo missing')
    [ "$st" = signed ] \
        || { echo "FATAL: $2 is stamped '$st' — refusing to continue with an unsigned kernel"; exit 1; }
    sudo podman run --rm --entrypoint /bin/sh "$1" -c \
        'dnf5 -y install -q sbsigntools >/dev/null 2>&1; sbverify --list /usr/lib/modules/$(cat /usr/lib/apex-kver)/vmlinuz' \
        | grep -qi 'signature certificates\|image signature issuers\|APEX' \
        || { echo "FATAL: sbverify found no signature on $2's vmlinuz"; exit 1; }
    sudo podman run --rm --entrypoint /bin/sh "$1" \
        -c 'test -s /usr/share/apex-os/secureboot/apex-mok.der' \
        || { echo "FATAL: $2 has no apex-mok.der — users would have nothing to enrol"; exit 1; }
    # The out-of-tree modules, checked the same way and for the same reason: a
    # marker file is a claim the build wrote about itself. `modinfo -F signer`
    # reads the PKCS#7 signature out of the module that will actually ship.
    #
    # Written so it cannot pass on an empty set — a loop over nothing succeeds,
    # and "no modules found" is precisely the failure this is here to catch.
    sudo podman run --rm --entrypoint /bin/sh "$1" -c '
        set -eu
        KVER=$(cat /usr/lib/apex-kver)
        MODDIR=/usr/lib/modules/$KVER
        [ "$(cat /usr/share/apex-os/secureboot/modules-signed 2>/dev/null || echo missing)" = signed ] || {
            echo "modules-signed is not \"signed\""; exit 1; }
        SIGNER=$(cat /usr/share/apex-os/secureboot/module-signer)
        OOT=""
        for d in extra updates; do [ -d "$MODDIR/$d" ] && OOT="$OOT $MODDIR/$d"; done
        [ -n "$OOT" ] || { echo "no out-of-tree module directory"; exit 1; }
        n=0
        for m in $(find $OOT -type f -name "*.ko*"); do
            got=$(modinfo -F signer "$m" 2>/dev/null || true)
            [ "$got" = "$SIGNER" ] || { echo "$m signed by \"$got\", expected \"$SIGNER\""; exit 1; }
            n=$((n + 1))
        done
        [ "$n" -gt 0 ] || { echo "zero out-of-tree modules — vacuous pass"; exit 1; }
        for pat in nvidia xone xpadneo; do
            [ "$(find $OOT -type f -name "*$pat*.ko*" | wc -l)" -gt 0 ] || { echo "no $pat module"; exit 1; }
        done
        echo "  $n out-of-tree modules, all signed by \"$SIGNER\""
    ' || { echo "FATAL: $2 has unsigned or missing out-of-tree kernel modules"; exit 1; }
    echo "$2: kernel signed, modules signed, apex-mok.der present"
}

# The kernel compile. Reused unless --force-core, on the same reasoning as the
# core reuse below: it is the slowest thing here and it only needs to move when
# kernel/kernel.pin does. Unlike core, there is no fallback if it is missing --
# Containerfile.core stops rather than quietly installing a kernel that never
# went through apex-kernel-btf-gate.
build_kernel() {
    if [ "$FORCE_CORE" = 0 ] && sudo podman image exists "$KERNEL_IMG"; then
        echo "== kernel == reusing existing $KERNEL_IMG (pass --force-core to rebuild)"
        return 0
    fi
    echo "== kernel == compiling the kernel (~45 min at -j12; longer on fewer cores)"
    sudo podman build --isolation=chroot \
        -f Containerfile.kernel -t "$KERNEL_IMG" .

    # Assert rather than trust. The build's own gate is what decides this, but
    # reading the verdict back out of the produced image is what proves the gate
    # ran at all -- a `podman build` that exits 0 is not evidence by itself.
    #
    # `podman create` + `podman cp`, NOT `podman run`. The kernel image is
    # `FROM scratch`: it has no /bin/sh, so `podman run --entrypoint /bin/sh`
    # cannot start, the `2>/dev/null || true` swallows it, and the read comes
    # back EMPTY -- which fails the test below and aborts with
    # "btf_scx='', expected 'usable'" AFTER a 45-minute compile that in fact
    # passed its gate. Measured against a scratch image carrying this exact
    # manifest: the run form yields '', the create+cp form yields 'usable'.
    kcid="$(sudo podman create "$KERNEL_IMG" /x)" \
        || { echo "FATAL: cannot create a container from $KERNEL_IMG"; exit 1; }
    ktmp="$(mktemp -d)"
    sudo podman cp "$kcid:/manifest/kernel-build.txt" "$ktmp/kernel-build.txt" \
        || { echo "FATAL: $KERNEL_IMG has no /manifest/kernel-build.txt -- it was not built by Containerfile.kernel"; \
             sudo podman rm "$kcid" >/dev/null 2>&1 || true; exit 1; }
    sudo podman rm "$kcid" >/dev/null
    got="$(sed -n 's/^btf_scx=//p' "$ktmp/kernel-build.txt")"
    rm -rf "$ktmp"
    [ "$got" = usable ] \
        || { echo "FATAL: kernel image reports btf_scx='$got', expected 'usable'"; exit 1; }
    echo "kernel: BTF verdict usable"
}

build_core() {
    if [ "$FORCE_CORE" = 0 ] && sudo podman image exists "$CORE_IMG"; then
        echo "== core == reusing existing $CORE_IMG (pass --force-core to rebuild)"
        return 0
    fi
    build_kernel
    echo "== core == (this is the slow one, ~45 min)"
    sudo podman build --isolation=chroot \
        "${SECRET_ARGS[@]}" \
        --build-arg APEX_REVISION="$REV" \
        --build-arg APEX_KERNEL_IMAGE="$KERNEL_IMG" \
        -f Containerfile.core -t "$CORE_IMG" .

    # Assert rather than trust. The Containerfile degrades to `unsigned` on any
    # signing failure, so a green build is not evidence the kernel is signed.
    assert_signed "$CORE_IMG" core
}

build_base() {
    # The base is FROM the core, so it cannot be built without one. Say so
    # clearly instead of letting podman fail on a missing image reference.
    sudo podman image exists "$CORE_IMG" \
        || { echo "FATAL: $CORE_IMG does not exist — run ./build-local.sh core first"; exit 1; }
    echo "== base =="
    sudo podman build --isolation=chroot \
        --build-arg CORE="$CORE_IMG" \
        --build-arg APEX_REVISION="$REV" \
        --build-arg APEX_SHELL_REF="$SHELL_REF" \
        -f Containerfile.base -t localhost/apex-os-base:latest .

    # Catches building on a stale unsigned core.
    assert_signed localhost/apex-os-base:latest base
}

build_image() {  # $1 = apex (or a legacy tag name, which maps to it)
    local f=$1
    case "$f" in
        apex|daily|gaming-mesa|gaming-nvidia) ;;
        *) echo "unknown target: $f" >&2; exit 2 ;;
    esac
    [ "$f" = apex ] || echo "note: '$f' is a published TAG, not a build target — building the one image"
    echo "== apex =="
    sudo podman build --isolation=chroot \
        --build-arg BASE=localhost/apex-os-base:latest \
        --build-arg APEX_REVISION="$REV" \
        -f Containerfile.apex -t localhost/apex-os:apex .
    # The three published names all resolve to this one image in the registry;
    # tag them locally too so a local `bootc switch` against any of them works.
    for t in daily gaming-mesa gaming-nvidia; do
        sudo podman tag localhost/apex-os:apex "localhost/apex-os:$t"
    done

    # Then read them back. `podman tag` cannot plausibly fail here — the point
    # is not to doubt it, it is to pin the INVARIANT. If this function is ever
    # changed to build per-name images again, the tags stop being one image and
    # every machine tracking a legacy name starts drifting onto different bytes.
    # That regression is silent, and this is the only local thing that would
    # notice it. The registry-side equivalent lives in build-image.yml and can
    # only run on a real publish.
    want="$(sudo podman image inspect --format '{{.Id}}' localhost/apex-os:apex)"
    [ -n "$want" ] || { echo "FATAL: localhost/apex-os:apex has no image ID" >&2; exit 1; }
    for t in apex daily gaming-mesa gaming-nvidia; do
        got="$(sudo podman image inspect --format '{{.Id}}' "localhost/apex-os:$t" 2>/dev/null || echo MISSING)"
        [ "$got" = "$want" ] || {
            echo "FATAL: localhost/apex-os:$t resolves to '$got', expected '$want'" >&2
            echo "       the four names must be ONE image; see docs/ci-release-tiers.md" >&2
            exit 1
        }
    done
    echo "tags: apex, daily, gaming-mesa, gaming-nvidia all resolve to $want"

    # The image inherits signing from core via the base, so this catches building
    # on top of a stale or unsigned tier — the same hole the CI job covers.
    assert_signed localhost/apex-os:apex apex
}

for t in "${TARGETS[@]}"; do
    case "$t" in
        kernel) build_kernel ;;
        core) build_core ;;
        base) build_base ;;
        *)    build_image "$t" ;;
    esac
done
echo "done: ${TARGETS[*]}"
