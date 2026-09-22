#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  release-version.sh — decide what versionCode this release gets, or refuse.
#
#  An Android `versionCode` must increase monotonically FOREVER. Android will
#  not install a code lower than the one already installed, and the only way a
#  user can get out of that is to uninstall — which, for this app, destroys the
#  paired device key and means walking back to the desktop to scan a new QR
#  code. A published regression cannot be fixed from the server side. So this
#  script's job is not to compute a number; it is to refuse to emit a bad one.
#
#  WHERE THE NUMBER COMES FROM
#
#  `git rev-list --count` on the release commit. It is deterministic (the same
#  commit always yields the same code, so a re-run cannot drift), it needs no
#  state file that could be lost or edited, and it only ever grows as long as
#  main is not rewritten.
#
#  THE RATCHETS, AND WHY THERE ARE TWO
#
#  A script is a thing people edit, so the count alone is not trusted:
#
#    1. The release commit must be an ANCESTOR of main. This is what makes the
#       count meaningful — a topic branch can easily have a lower count than
#       main, and releasing from one would hand out a code that goes backwards.
#    2. The new code must be strictly greater than every `android-v<N>` tag
#       that already exists, locally or on the remote. Tags are the durable
#       record: git refuses to move one, so the tag namespace itself is a
#       ratchet that survives this file being rewritten.
#
#  EQUAL IS ALSO REFUSED. Re-dispatching on a commit that has already been
#  released would produce a second, different APK wearing a code that is
#  already in the wild, and Android would refuse to install it over the first
#  as surely as it refuses a lower one. Bumping needs a new commit, which is
#  what "release the current main" already means.
#
#  A LOOKUP THAT FAILED IS NOT AN EMPTY ANSWER. Every git command here is
#  checked, because `git ls-remote` printing nothing because the network is
#  down looks exactly like "there are no tags yet" — and that mistake would
#  hand out a code below every one already published.
#
#  Usage:
#      android/tools/release-version.sh [--remote origin] [--main-ref origin/main]
#
#  Prints three `key=value` lines on success and nothing on refusal:
#      code=1306
#      name=0.1.0+1306.g303221d5
#      tag=android-v1306
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

remote=origin
main_ref=""
marketing=""

while [ $# -gt 0 ]; do
    case "$1" in
        --remote)    remote="${2:?--remote needs a value}"; shift 2 ;;
        --main-ref)  main_ref="${2:?--main-ref needs a value}"; shift 2 ;;
        --marketing) marketing="${2:?--marketing needs a value}"; shift 2 ;;
        -h|--help)   sed -n '2,50p' "$0"; exit 0 ;;
        *) echo "FATAL: unknown argument '$1'" >&2; exit 2 ;;
    esac
done
[ -n "$main_ref" ] || main_ref="$remote/main"

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
android=$(dirname "$here")
repo=$(dirname "$android")
cd "$repo" || { echo "FATAL: cannot enter $repo" >&2; exit 2; }

fatal() { echo "FATAL: $*" >&2; exit 1; }

# ── The marketing version, read from the build file rather than repeated ─────
#
# Two copies of a version string is one copy that goes stale. This reads the
# single declaration in build.gradle.kts, and refuses if it cannot find it
# rather than inventing a default — a release named after a version nobody
# chose is worse than a release that did not happen.
if [ -z "$marketing" ]; then
    gradle_file="$android/app/build.gradle.kts"
    [ -f "$gradle_file" ] || fatal "$gradle_file is missing, so the version cannot be read"
    marketing=$(sed -n 's/^val APEX_MARKETING_VERSION = "\([^"]*\)".*/\1/p' "$gradle_file" | head -1)
    [ -n "$marketing" ] \
        || fatal "no APEX_MARKETING_VERSION line in $gradle_file; it was renamed or removed"
fi
case "$marketing" in
    *[!0-9.]*|"") fatal "APEX_MARKETING_VERSION '$marketing' is not a dotted number" ;;
esac

# ── The release commit ───────────────────────────────────────────────────────
git rev-parse HEAD >/dev/null 2>&1 || fatal "not a git repository, or HEAD is unborn"
short_sha=$(git rev-parse --short=8 HEAD 2>/dev/null) || fatal "cannot shorten HEAD"

# ── Ratchet 1: the release commit must be on main ────────────────────────────
#
# `--is-ancestor` exits 0 for "yes", 1 for "no" and >1 for "could not tell" —
# and the three are kept apart deliberately. A missing main ref answers 128,
# which must be a refusal and NOT be folded into "no": one means the release is
# off main, the other means this script could not look, and reporting the
# second as the first would hide a broken checkout behind a sensible-looking
# error message.
git rev-parse --verify --quiet "$main_ref^{commit}" >/dev/null \
    || fatal "'$main_ref' does not resolve here. Fetch it first: a release cannot be checked against a branch this clone does not have."
git merge-base --is-ancestor HEAD "$main_ref"
case $? in
    0) : ;;
    1) fatal "HEAD ($short_sha) is not an ancestor of $main_ref. A versionCode derived from a branch can be LOWER than one already released; release from main." ;;
    *) fatal "git merge-base could not compare HEAD with $main_ref" ;;
esac

# ── The number ───────────────────────────────────────────────────────────────
code=$(git rev-list --count HEAD 2>/dev/null) || fatal "git rev-list --count failed"
case "$code" in
    ""|*[!0-9]*) fatal "git rev-list --count printed '$code', which is not a number" ;;
esac
[ "$code" -gt 0 ] || fatal "the commit count is $code; an Android versionCode must be positive"
# Android's own ceiling. Far away, but a silent wrap here would be catastrophic
# and unrecoverable, so it is a named refusal.
[ "$code" -lt 2100000000 ] || fatal "versionCode $code is at Android's 2,100,000,000 ceiling"

tag="android-v$code"

# ── Ratchet 2: every tag that already exists ─────────────────────────────────
#
# Local and remote, because either on its own can be stale: a fresh CI clone
# has no local tags, and a laptop that has not fetched has no remote ones.
local_tags=$(git tag --list 'android-v*' 2>/dev/null) \
    || fatal "git tag --list failed, so existing releases could not be read"

# Not in a pipeline: `git ls-remote | grep` would report the pipeline's exit
# status, and `set -o pipefail` with a `grep -q` match returns 141 on SIGPIPE.
# The whole answer is captured first and examined afterwards.
if ! remote_raw=$(git ls-remote --tags "$remote" 'refs/tags/android-v*' 2>/dev/null); then
    fatal "cannot read tags from '$remote'. A release must not be numbered against tags this script could not see — that is how a code lands below one already published."
fi
remote_tags=$(printf '%s\n' "$remote_raw" | sed -n 's|.*refs/tags/\(android-v[0-9]*\)$|\1|p')

highest=0
for existing in $local_tags $remote_tags; do
    n="${existing#android-v}"
    case "$n" in
        ""|*[!0-9]*) continue ;;
    esac
    [ "$n" -gt "$highest" ] && highest="$n"
done

if [ "$code" -le "$highest" ]; then
    if [ "$code" -eq "$highest" ]; then
        fatal "versionCode $code has already been released as $tag. Releasing this commit again would put a SECOND, different APK behind a code that is already installed on phones, and Android would refuse to install it over the first. Land a commit and release that."
    fi
    fatal "versionCode $code is BELOW the released high-water mark of $highest. An APK with a lower code cannot be installed over its predecessor, and the user's only escape is to uninstall — which destroys their paired device key."
fi

printf 'code=%s\n' "$code"
printf 'name=%s+%s.g%s\n' "$marketing" "$code" "$short_sha"
printf 'tag=%s\n' "$tag"
