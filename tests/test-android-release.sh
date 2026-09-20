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
    {
        printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n'
        printf 'val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(1)\n'
    } > "$dir/repo/android/core/src/main/kotlin/com/apexos/remote/core/Client.kt"
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
printf '{"remoteProtocolPreferred": 4, "remoteProtocolSupported": [4, 3]}\n' \
    > "$notes_dir/apex-remote-$name.json"

out=$("$NOTES_SH" --code 7 --name "$name" --dir "$notes_dir" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && said "$out" "$digest"; then
    ok "the page prints the checksum of the file beside it"
else
    bad "the notes did not carry the real digest (rc=$rc)"
fi
# Read from the metadata, not hardcoded: the fixture's window is [4, 3], so a
# page saying "v3 to v4" can only have derived it, and a page saying v1 would
# be reciting a constant. Both halves are checked — the range in the prose and
# the preferred revision in the table, which is the number somebody compares
# against `apex remote status`.
if said "$out" "v3 to v4"; then
    ok "the page states the protocol window this build speaks, read from the build"
else
    bad "the page does not state the protocol window, or hardcodes it"
fi
if said "$out" "\`v4\`"; then
    ok "the page names the revision this build prefers, for comparing with the machine"
else
    bad "the page does not name the preferred revision"
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

section "the protocol window reaches the metadata, or the release stops"

# The compatibility promise on the Releases page is generated from the window
# this build actually ships. A build whose window cannot be read must refuse,
# because the alternative is a page that implies compatibility nobody can check
# — which is this repository's dominant defect class wearing prose.

FIXW="$WORK/fixw"
build_fixture "$FIXW" 3
CLIENT="$FIXW/repo/android/core/src/main/kotlin/com/apexos/remote/core/Client.kt"
ART="$FIXW/repo/android/tools/release-artifacts.sh"

# 1. A window that is no longer a literal list. `listOf(REMOTE_PROTOCOL_VERSION)`
#    is the obvious, tidy-looking edit, and it is the one that makes the window
#    unreadable — so it must stop the release rather than publish a blank.
{
    printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n'
    printf 'val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(REMOTE_PROTOCOL_VERSION)\n'
} > "$CLIENT"
out=$(APEX_KEYSTORE=/dev/null APEX_KEYSTORE_PASSWORD=x APEX_KEY_ALIAS=y APEX_KEY_PASSWORD=z \
      "$ART" --out "$WORK/outw" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "not a plain list of integers"; then
    ok "a window that is not a literal list refuses the release"
else
    bad "an unreadable protocol window did not stop the release (rc=$rc): $out"
fi

# 2. The window renamed out of the file entirely.
printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n' > "$CLIENT"
out=$(APEX_KEYSTORE=/dev/null APEX_KEYSTORE_PASSWORD=x APEX_KEY_ALIAS=y APEX_KEY_PASSWORD=z \
      "$ART" --out "$WORK/outw" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "SUPPORTED_REMOTE_PROTOCOL_VERSIONS"; then
    ok "a missing window is named, not silently treated as empty"
else
    bad "a missing window was not reported (rc=$rc): $out"
fi

# 3. A build that prefers a revision it does not list. The Kotlin suite asserts
#    this too; it is asserted again at the last point before the number reaches
#    a phone, because the two files can be edited apart.
{
    printf 'const val REMOTE_PROTOCOL_VERSION: Int = 3\n'
    printf 'val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(2, 1)\n'
} > "$CLIENT"
out=$(APEX_KEYSTORE=/dev/null APEX_KEYSTORE_PASSWORD=x APEX_KEY_ALIAS=y APEX_KEY_PASSWORD=z \
      "$ART" --out "$WORK/outw" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "cannot prefer a revision it does not list"; then
    ok "a build preferring a revision outside its own window is refused"
else
    bad "a preferred revision outside the window was accepted (rc=$rc): $out"
fi

# 4. And the check is not vacuous: a good file gets PAST it and fails later, on
#    the keystore. A refusal that fires for every input would pass all three
#    assertions above and prove nothing.
{
    printf 'const val REMOTE_PROTOCOL_VERSION: Int = 1\n'
    printf 'val SUPPORTED_REMOTE_PROTOCOL_VERSIONS: List<Int> = listOf(1)\n'
} > "$CLIENT"
out=$(env -u APEX_KEYSTORE -u APEX_KEYSTORE_PASSWORD -u APEX_KEY_ALIAS -u APEX_KEY_PASSWORD \
      "$ART" --out "$WORK/outw" --code 5 --name 0.1.0+5.gdeadbeef 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "APEX_KEYSTORE is not set"; then
    ok "a readable window is not refused; the run proceeds to the next gate"
else
    bad "a valid protocol window was refused, so the check fires for everything (rc=$rc): $out"
fi

section "the page states the compatibility promise this build can keep"

# The page is read by somebody whose machine will not connect. A single-revision
# build that told them "an app one revision behind still connects" would be
# describing a build that does not exist.
onew="$WORK/notes-one"
mkdir -p "$onew"
namew="0.1.0+9.gaaaaaaaa"
printf 'apk\n' > "$onew/apex-remote-$namew.apk"
dw=$(sha256sum "$onew/apex-remote-$namew.apk"); dw="${dw%% *}"
printf '%s  apex-remote-%s.apk\n' "$dw" "$namew" > "$onew/apex-remote-$namew.apk.sha256"
printf '{"remoteProtocolPreferred": 1, "remoteProtocolSupported": [1]}\n' \
    > "$onew/apex-remote-$namew.json"
out=$("$NOTES_SH" --code 9 --name "$namew" --dir "$onew" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && said "$out" "exactly one revision"; then
    ok "a single-revision build says so instead of promising a range"
else
    bad "a one-entry window claimed compatibility it does not have (rc=$rc)"
fi
if said "$out" "behind it by one"; then
    bad "the page still carries the old promise that this build cannot keep"
else
    ok "the page does not claim a tolerance this build does not have"
fi

# And the other arm. Both branches of a conditional sentence have to be
# exercised, or half of it is prose nobody has read.
printf '{"remoteProtocolPreferred": 3, "remoteProtocolSupported": [3, 2, 1]}\n' \
    > "$onew/apex-remote-$namew.json"
out=$("$NOTES_SH" --code 9 --name "$namew" --dir "$onew" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && said "$out" "v1 to v3"; then
    ok "a multi-revision build states the range it really speaks"
else
    bad "a three-entry window did not produce a range (rc=$rc): $out"
fi

# A metadata file with no window at all. Older than the field, or written by
# something that is not release-artifacts.sh; either way the page must not
# guess.
printf '{"remoteProtocolPreferred": 1}\n' > "$onew/apex-remote-$namew.json"
out=$("$NOTES_SH" --code 9 --name "$namew" --dir "$onew" 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && said "$out" "remoteProtocolSupported"; then
    ok "a page that cannot state the window is refused rather than implied"
else
    bad "a missing window produced a page anyway (rc=$rc): $out"
fi

section "the workflow and the app agree about where updates come from"

# The in-app updater reads the Releases page of one repository, and the release
# workflow writes to it. A metadata file the updater cannot parse would leave
# every installed phone quietly stuck, so the contract is asserted on both
# sides rather than kept in step by memory.
UPDATER="$ROOT/android/core/src/main/kotlin/com/apexos/remote/core/update/Updates.kt"
[ -f "$UPDATER" ] || { echo "FATAL: $UPDATER is missing; the updater this section is about does not exist" >&2; exit 2; }
updater_src=$(cat "$UPDATER")
artifacts_src=$(cat "$ARTIFACTS_SH")

for field in versionCode apk sha256 sizeBytes remoteProtocolSupported; do
    if said "$updater_src" "$field" && said "$artifacts_src" "$field"; then
        ok "both sides of the update metadata name $field"
    else
        bad "$field is written by one side and not read by the other"
    fi
done

# NOT /releases/latest. Measured 2026-09-20: this repository's Releases page
# carries OS netinstall ISOs, and that endpoint answers one of them — an
# updater built on it finds no APK and reports "up to date" forever.
# Comments STRIPPED before looking. `Updates.kt` explains at length why it does
# not use `/releases/latest`, and a bare substring search would read that
# explanation as the offence — the same shape as the forbid-check this
# repository once shipped that matched the comment describing what it forbade.
updater_code=$(grep -vE '^[[:space:]]*(\*|//|/\*)' "$UPDATER")
if said "$updater_code" "releases/latest"; then
    bad "the updater uses /releases/latest, which on this repository answers an OS ISO release"
else
    ok "the updater does not use /releases/latest, which here would never find an APK"
fi
# …and the strip is not so eager that it left nothing to search. A check that
# reads an empty string always passes.
if said "$updater_code" "newestAndroidRelease"; then
    ok "there is real code left after the comments are stripped"
else
    bad "stripping comments left nothing, so the check above inspected an empty string"
fi
if said "$updater_src" 'android-v'; then
    ok "the updater picks its release out by the android-v tag"
else
    bad "the updater has no way to tell an Android release from an OS one"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
[ "$pass" -gt 0 ] || { echo "FATAL: the suite asserted nothing"; exit 2; }
