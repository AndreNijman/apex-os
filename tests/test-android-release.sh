#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  The Android release flow, exercised rather than described.
#
#  What this suite is about is one irreversible mistake: publishing an APK
#  whose `versionCode` is not strictly greater than every code already
#  published. Android refuses to install a lower code over a higher one, and
#  the only escape a user has is to uninstall — which destroys this app's
#  paired device key and means walking back to the desktop to scan a new QR
#  code. There is no server-side fix. So the refusals below are the product,
#  and the happy path is almost incidental.
#
#  NOTHING HERE IS STUBBED WHERE IT COULD BE REAL. The version gate reads git
#  tags, so the fixture is a real git repository with real tags and a real
#  remote — a fake `git` on PATH would only prove the fake agrees with itself.
#  The two places a real thing is out of reach are named where they occur.
#
#      ./tests/test-android-release.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Counted, not aborted: most assertions below run a command that is SUPPOSED to
# exit non-zero, and under `-e` the first refusal would end the suite and
# report every later assertion as a failure. GitHub Actions invokes a script as
# `bash -e {0}`, so this matters here and not only at a prompt.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

for tool in git python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FATAL: $tool is required; this suite cannot test anything without it" >&2
        exit 2
    }
done

VERSION_SH="$ROOT/android/tools/release-version.sh"
ARTIFACTS_SH="$ROOT/android/tools/release-artifacts.sh"
NOTES_SH="$ROOT/android/tools/release-notes.sh"
WORKFLOW="$ROOT/.github/workflows/release-android.yml"

for f in "$VERSION_SH" "$ARTIFACTS_SH" "$NOTES_SH" "$WORKFLOW"; do
    [ -f "$f" ] || { echo "FATAL: $f is missing; the flow this suite tests does not exist" >&2; exit 2; }
done
for f in "$VERSION_SH" "$ARTIFACTS_SH" "$NOTES_SH"; do
    [ -x "$f" ] || { echo "FATAL: $f is not executable, so CI could not run it" >&2; exit 2; }
done

# `said <text> <needle>` — substring match without a pipeline, because
# `printf … | grep -q X` returns 141 on a match under `pipefail` (SIGPIPE on
# the writer) and this repository has been mis-seeded by exactly that before.
said() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

# ── The fixture: a real repository with a real remote ────────────────────────
#
# `release-version.sh` finds the repository from its OWN location, so the
# script is copied into a fixture tree shaped like the real one rather than
# being run against this checkout — which has thousands of commits and whose
# tags are the real ones.
build_fixture() {
    local dir="$1" commits="${2:-3}"
    rm -rf "$dir"
    mkdir -p "$dir/upstream"
    git init --quiet --bare "$dir/upstream"
    mkdir -p "$dir/repo/android/tools" "$dir/repo/android/app" \
             "$dir/repo/android/core/src/main/kotlin/com/apexos/remote/core"
    cp "$VERSION_SH" "$dir/repo/android/tools/release-version.sh"
    cp "$ARTIFACTS_SH" "$dir/repo/android/tools/release-artifacts.sh"
    cp "$NOTES_SH" "$dir/repo/android/tools/release-notes.sh"
    printf 'val APEX_MARKETING_VERSION = "0.1.0"\n' > "$dir/repo/android/app/build.gradle.kts"
    printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n' \
        > "$dir/repo/android/core/src/main/kotlin/com/apexos/remote/core/Client.kt"
    (
        cd "$dir/repo" || exit 1
        git init --quiet -b main .
        git config user.email fixture@example.invalid
        git config user.name fixture
        git config commit.gpgsign false
        local i=1
        while [ "$i" -le "$commits" ]; do
            printf '%s\n' "$i" > commit.txt
            git add -A >/dev/null
            git commit --quiet -m "commit $i"
            i=$((i + 1))
        done
        git remote add origin "$dir/upstream"
        git push --quiet origin main
        git fetch --quiet origin
    )
}

FIX="$WORK/fix"

section "the version gate: the happy path, and that it is not vacuous"

build_fixture "$FIX" 3
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && said "$out" "code=3" && said "$out" "tag=android-v3"; then
    ok "three commits on main produce versionCode 3"
else
    bad "a clean first release was refused (rc=$rc): $out"
fi
# Non-vacuous: the name really carries the version and the commit, so a later
# assertion about a refusal is not passing because everything fails.
short=$(git -C "$FIX/repo" rev-parse --short=8 HEAD)
if said "$out" "name=0.1.0+3.g$short"; then
    ok "the versionName carries the marketing version, the code and the commit"
else
    bad "versionName is not what it should be: $out"
fi

section "the version gate: every way a code could go wrong"

# 1. The same commit released twice.
build_fixture "$FIX" 3
git -C "$FIX/repo" tag android-v3 >/dev/null 2>&1
git -C "$FIX/repo" push --quiet origin android-v3 >/dev/null 2>&1
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "already been released"; then
    ok "re-releasing a commit that already has a tag is refused"
else
    bad "an already-released code was accepted (rc=$rc): $out"
fi

# 2. A code below the high-water mark. This is the unrecoverable one.
build_fixture "$FIX" 3
git -C "$FIX/repo" tag android-v99 >/dev/null 2>&1
git -C "$FIX/repo" push --quiet origin android-v99 >/dev/null 2>&1
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "BELOW the released high-water mark"; then
    ok "a versionCode below one already published is refused"
else
    bad "a REGRESSING versionCode was accepted (rc=$rc): $out"
fi

# 3. The tag exists only on the REMOTE. A fresh CI clone has no local tags, so
#    a gate that consulted only `git tag` would wave this through.
build_fixture "$FIX" 3
(
    cd "$FIX/repo" || exit 1
    git tag android-v50 >/dev/null 2>&1
    git push --quiet origin android-v50 >/dev/null 2>&1
    git tag -d android-v50 >/dev/null 2>&1
)
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "BELOW the released high-water mark"; then
    ok "a tag that exists only on the remote still blocks a lower code"
else
    bad "a remote-only tag was not consulted (rc=$rc): $out"
fi

# 4. Releasing off main. A branch can have a LOWER count than main.
build_fixture "$FIX" 3
(
    cd "$FIX/repo" || exit 1
    git checkout --quiet -b side HEAD~2
    printf 'side\n' > commit.txt
    git add -A >/dev/null
    git commit --quiet -m "off main"
)
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "not an ancestor"; then
    ok "releasing from a branch that is not on main is refused"
else
    bad "a release off main was accepted (rc=$rc): $out"
fi

# 5. A remote that cannot be read. THE important one: `git ls-remote` printing
#    nothing because the network is down looks exactly like "no tags yet", and
#    treating the two the same is how a code lands below one already published.
build_fixture "$FIX" 3
git -C "$FIX/repo" remote set-url origin "$WORK/there-is-no-repository-here"
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "cannot read tags"; then
    ok "a remote that cannot be read is a refusal, not an empty tag list"
else
    bad "an unreadable remote was treated as 'no releases yet' (rc=$rc): $out"
fi

# 6. A main ref this clone does not have. Must be told apart from "off main".
build_fixture "$FIX" 3
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/nonexistent 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "does not resolve here"; then
    ok "a missing main ref says so instead of reporting the release as off-main"
else
    bad "a missing main ref was misreported (rc=$rc): $out"
fi

# 7. The marketing version renamed out of the build file.
build_fixture "$FIX" 3
printf '// nothing here any more\n' > "$FIX/repo/android/app/build.gradle.kts"
out=$("$FIX/repo/android/tools/release-version.sh" --main-ref origin/main 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "APEX_MARKETING_VERSION"; then
    ok "a renamed marketing version is a refusal, not a made-up default"
else
    bad "a missing marketing version did not refuse (rc=$rc): $out"
fi

section "the artefact build refuses to produce something nobody can install"

# Unlike pr-validation.yml, where an absent key is a notice and the unsigned
# artefacts are built on purpose, this script's output is what a person
# installs. Absent means stop.
out=$(env -u APEX_KEYSTORE -u APEX_KEYSTORE_PASSWORD -u APEX_KEY_ALIAS -u APEX_KEY_PASSWORD \
      "$ARTIFACTS_SH" --out "$WORK/out" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "APEX_KEYSTORE is not set"; then
    ok "a release build with no signing key is refused, and names the variable"
else
    bad "an unsigned release build was not refused (rc=$rc): $out"
fi

out=$(APEX_KEYSTORE="$WORK/no-such-keystore.jks" APEX_KEYSTORE_PASSWORD=x \
      APEX_KEY_ALIAS=y APEX_KEY_PASSWORD=z \
      "$ARTIFACTS_SH" --out "$WORK/out" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "which is not a file"; then
    ok "a keystore path that is not a file is refused before the build starts"
else
    bad "a missing keystore file was not refused (rc=$rc): $out"
fi

# The refusal must come BEFORE gradle runs, or CI burns minutes to discover it.
if said "$out" "Gradle" || said "$out" "gradlew"; then
    bad "the keystore check ran after the build started"
else
    ok "the keystore is checked before anything is compiled"
fi

section "the release page describes the artefact that was actually built"

notes_dir="$WORK/notes"
mkdir -p "$notes_dir"
name="0.1.0+7.gcafef00d"
printf 'not really an apk, but a real file with a real digest\n' > "$notes_dir/apex-remote-$name.apk"
digest=$(sha256sum "$notes_dir/apex-remote-$name.apk")
digest="${digest%% *}"
printf '%s  apex-remote-%s.apk\n' "$digest" "$name" > "$notes_dir/apex-remote-$name.apk.sha256"
printf '{"remoteProtocolPreferred": 4}\n' > "$notes_dir/apex-remote-$name.json"

out=$("$NOTES_SH" --code 7 --name "$name" --dir "$notes_dir" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && said "$out" "$digest"; then
    ok "the page prints the checksum of the file beside it"
else
    bad "the notes did not carry the real digest (rc=$rc)"
fi
# Read from the metadata, not hardcoded: the fixture says 4, so a page that
# printed v1 would be reciting a constant.
if said "$out" "protocol v4"; then
    ok "the page states the protocol revision this build speaks, read from the build"
else
    bad "the page does not state the protocol revision, or hardcodes it"
fi
if said "$out" "Allow from this source"; then
    ok "the page tells a first-time sideloader what Android will ask them"
else
    bad "the page has no sideloading instructions"
fi
if said "$out" "Do not uninstall"; then
    ok "the page warns that uninstalling destroys the pairing"
else
    bad "the page does not warn about uninstalling"
fi

# A page that described an artefact nobody built would be the worst of the
# lot: it is the page people check their download against.
out=$("$NOTES_SH" --code 7 --name "0.1.0+8.gnothere" --dir "$notes_dir" 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "is missing"; then
    ok "notes for an artefact that was not built are refused"
else
    bad "the notes described a non-existent APK (rc=$rc): $out"
fi

section "the workflow cannot quietly do the wrong thing"

wf=$(cat "$WORKFLOW")

if said "$wf" 'refs/heads/main'; then
    ok "the workflow checks it is running on main"
else
    bad "the workflow has no main-only guard"
fi
# Default true, so a dispatch by someone exploring the Actions tab builds and
# verifies but publishes nothing.
if said "$wf" 'dry_run' && said "$wf" 'default: true'; then
    ok "dry_run exists and defaults to true"
else
    bad "dry_run is missing or does not default to true"
fi
if said "$wf" 'fetch-depth: 0'; then
    ok "the checkout takes the whole history, which the commit count needs"
else
    bad "a shallow checkout would produce a versionCode below every released one"
fi
if said "$wf" 'contents: write'; then
    ok "the release permission is declared"
else
    bad "the workflow cannot create a release without contents: write"
fi
# The scripts are invoked by name; a rename that missed the workflow would
# leave a green-looking file that runs nothing.
for script in release-version.sh release-artifacts.sh release-notes.sh; do
    if said "$wf" "$script"; then
        ok "the workflow runs $script"
    else
        bad "$script is not invoked by the workflow"
    fi
done
if said "$wf" 'shred'; then
    ok "the keystore is shredded from the runner"
else
    bad "the keystore is left on the runner"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
[ "$pass" -gt 0 ] || { echo "FATAL: the suite asserted nothing"; exit 2; }
