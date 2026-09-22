#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-plugin.sh — assertions against the SHIPPED plugin CLI,
#  files/system/libexec/apex-plugin and its node shim. Nothing here
#  re-implements either: every case runs the script as a process, or sources it
#  and calls the function the image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  §16's plugin platform ships in apex-shell (PR #9, merged) and deliberately left the
#  OS-side CLI out. `apex plugin list|info|enable|disable` is that CLI, and it
#  has two properties that need proving rather than asserting in prose:
#
#   1. IT OWNS NO RULES. Every decision about a manifest comes from the shell's
#      own src/services/plugins/manifest.js, through node. So the interesting
#      assertion is not "does the CLI refuse a bad plugin" but "does it refuse
#      it with the SAME reason code manifest.js gives" — which is checked
#      differentially, by asking manifest.js directly and comparing.
#
#   2. `disable` MOVES A DIRECTORY. The shell has no enabled/disabled concept:
#      PluginService.qml scans exactly one directory and is the only reader of
#      that path in the whole shell, so there is no allowlist to edit and no
#      IPC to push a decision over. Moving the directory out of the tree the
#      shell scans is what actually takes effect. That makes the move the one
#      write path in the feature, and it is asserted like one.
#
#  ── This suite must never touch the developer's live shell ──────────────────
#  ~/.config/apex-shell/plugins is a real directory on the developer's machine
#  and `disable` moves things out of it. The repo's AGENTS.md gained a rule
#  about this the hard way: a one-line regex substitution deleted 217 of 256
#  lines from a live hyprland.conf, and the reason it was recoverable at all was
#  a backup.
#
#  So the plugin directories are pointed at a temp tree through
#  APEX_PLUGIN_DIR / APEX_PLUGIN_DISABLED_DIR, and this suite HARD-EXITS if
#  either resolves outside it — the same shape as test-apex-env.sh's refusal to
#  run when `command -v distrobox` is not the stub. A skip would not do: a
#  suite that quietly ran against the real directory is exactly the accident
#  the rule exists for.
#
#  ── What needs the shell tree, and what does not ────────────────────────────
#  The verdict half cannot run without apex-shell's manifest.js. That file was
#  once only on apex-shell **PR #9**, and this section used to say so; PR #9 has
#  since merged, so a missing validator no longer means "the shell has not
#  landed yet". It now means the tree that answered is older than that merge —
#  in practice a stale /usr/share/apex-shell on a development machine, which is
#  the last-resort fallback when no checkout is found. The suite prints which
#  tree it used for exactly this reason; `APEX_SHELL_TREE` overrides it.
#
#  It is deliberately ONE labelled failure rather than a refusal at the top of
#  the file. Everything that does not need manifest.js — the path-safety rules,
#  the enumeration, the whole move mechanism and all of its shape assertions,
#  and the refusal when the validator is absent — runs regardless. A suite that
#  hard-exited would report `passed=0` with a red tick, which is the same
#  vacuous shape this repository has been bitten by, only inverted.
#
#  PASS = every case prints exactly what it should, with a non-zero exit where
#         one is expected.
#
#  Run from anywhere: ./tests/test-apex-plugin.sh
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`: this suite COUNTS failures rather than aborting, and many cases run
# commands that exit non-zero on purpose. GitHub Actions invokes a script as
# `bash -e {0}`, and under `-e` the first such command would end the run
# silently instead of reporting anything.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-plugin
VERDICT=../files/system/libexec/apex-plugin-verdict.js
for f in "$ENGINE" "$VERDICT"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done
ENGINE=$(cd "$(dirname "$ENGINE")" && pwd)/$(basename "$ENGINE")
VERDICT=$(cd "$(dirname "$VERDICT")" && pwd)/$(basename "$VERDICT")

WORK=$(mktemp -d /tmp/apex-plugin-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %-56s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-56s %s\n' "$1" "${2:-}"; fail=$((fail+1)); }
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}
has() {
    local name=$1 want=$2 hay=$3
    if grep -qF -- "$want" <<<"$hay"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want") in: $(head -3 <<<"$hay" | tr '\n' ' ')"; fi
}
hasnt() {
    local name=$1 unwanted=$2 hay=$3
    if grep -qF -- "$unwanted" <<<"$hay"; then
        bad "$name" "found $(printf '%q' "$unwanted") and should not have"
    else ok "$name"; fi
}

# ── the sandbox ─────────────────────────────────────────────────────────────
PLUGINS="$WORK/plugins"
DISABLED="$WORK/plugins-disabled"
FAKEHOME="$WORK/home"
mkdir -p "$PLUGINS" "$DISABLED" "$FAKEHOME"

# The apex-shell tree that owns the plugin rules. `../apex-shell` is what CI
# vendors and what every other cross-repo suite here uses; the image path is
# the fallback for a run on a real APEX machine.
SHELL_TREE="${APEX_SHELL_TREE:-}"
if [ -z "$SHELL_TREE" ]; then
    for candidate in ../../apex-shell /usr/share/apex-shell; do
        [ -d "$candidate" ] && { SHELL_TREE="$candidate"; break; }
    done
fi
MANIFEST_JS=""
[ -n "$SHELL_TREE" ] && MANIFEST_JS="${SHELL_TREE}/src/services/plugins/manifest.js"
PLUGIN_SERVICE=""
[ -n "$SHELL_TREE" ] && PLUGIN_SERVICE="${SHELL_TREE}/src/services/plugins/PluginService.qml"

# Say which tree answered. Without this the suite's own failure message cannot
# be acted on: "the validator is missing" reads as a code defect, when the
# usual cause is that a stale /usr/share/apex-shell on a development machine
# answered instead of a source checkout.
if [ -n "$SHELL_TREE" ]; then
    printf 'using apex-shell tree: %s\n' "$(cd "$SHELL_TREE" 2>/dev/null && pwd || echo "$SHELL_TREE")"
else
    printf 'no apex-shell tree found (looked for $APEX_SHELL_TREE, ../../apex-shell, /usr/share/apex-shell)\n'
fi

echo "── the suite must not be able to reach the real plugin directory ──────"
# This is the guard, not a formality. `disable` MOVES a directory; if these
# resolved to the developer's own ~/.config/apex-shell, the cases below would
# move their live plugins out of the shell's reach. A hard exit, never a skip.
case "$PLUGINS" in
    "$WORK"/*) ok "the plugin directory under test is inside the temp tree" ;;
    *) echo "FATAL: PLUGINS=$PLUGINS is outside $WORK" >&2; exit 2 ;;
esac
case "$DISABLED" in
    "$WORK"/*) ok "the disabled directory under test is inside the temp tree" ;;
    *) echo "FATAL: DISABLED=$DISABLED is outside $WORK" >&2; exit 2 ;;
esac
# node is REQUIRED, not optional. The plugin rules live in the shell's own
# JavaScript, so a runner without node cannot verify anything — and the failure
# must be a refusal rather than a section that quietly does not run.
#
# ── Resolved here and THREADED into every invocation ────────────────────────
# The cases below reduce PATH so nothing on the developer's shell can change a
# result. That is a trap for this particular dependency: `node` is at
# /usr/bin/node on Fedora, but on GitHub's ubuntu-24.04 runner it comes from the
# toolcache and `command -v node` returns /usr/local/bin/node — /usr/bin/node
# need not exist at all. A suite whose reduced PATH happened to contain node
# would pass here and produce fifty "node is not available" failures on CI that
# read as a broken feature rather than a wrong assumption.
#
# So the real path is found once, with the ambient PATH, and passed in as
# APEX_PLUGIN_NODE — and the reduced PATH every case runs on contains NO node
# at all. That makes the threading load-bearing in every invocation rather than
# only in the one case written to check it: remove it anywhere and the suite
# fails here, not on the runner.
#
# The engine's own default stays a plain `node` off PATH, which is right in the
# image, where node is a package.
node_path="$(command -v node 2>/dev/null || true)"
if [ -z "$node_path" ]; then
    echo "FATAL: node is not available. The plugin rules live in apex-shell's own" >&2
    echo "manifest.js and this suite asks THAT file; there is nothing to verify" >&2
    echo "without a JavaScript runtime. Refusing to report a pass." >&2
    exit 2
fi
ok "node is available at $node_path"

# Prove the engine really honours the directory overrides rather than deriving
# them from $HOME anyway — the single mistake that would put every case below
# on the developer's live configuration.
resolved="$(env -i PATH=/usr/bin:/bin HOME="$FAKEHOME" \
    APEX_PLUGIN_DIR="$PLUGINS" APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
    APEX_PLUGIN_MANIFEST_JS="$WORK/absent.js" APEX_PLUGIN_VERDICT_JS="$VERDICT" \
    APEX_PLUGIN_NODE="$node_path" \
    bash "$ENGINE" list 2>&1)"
hasnt "…and the engine never mentions a path outside it" "$FAKEHOME/.config" "$resolved"

# And prove APEX_PLUGIN_NODE is really what makes the verdict path work, by
# running the engine with a PATH that deliberately does NOT contain node.
#
# This is the CI-runner situation reproduced rather than reasoned about: on
# ubuntu-24.04 node comes from the toolcache, so a suite that assumed
# /usr/bin/node would fail every verdict assertion. Building a PATH with the
# handful of programs the engine needs — and no node — is the only control that
# distinguishes "the threading works" from "node happens to be in /usr/bin".
NODELESS="$WORK/nodeless"
mkdir -p "$NODELESS"
for t in bash jq find awk grep sed cat mkdir mv rm wc sort dirname sha256sum ln cp; do
    src="$(command -v "$t" 2>/dev/null || true)"
    [ -n "$src" ] && ln -sf "$src" "$NODELESS/$t"
done
if [ -e "$NODELESS/node" ]; then
    bad "the nodeless PATH really has no node" "a node symlink got in"
else
    ok "the nodeless PATH really has no node"
fi
if "$node_path" --check "$VERDICT" 2>/dev/null; then
    ok "the shipped node shim parses"
else
    bad "the shipped node shim parses" "node --check failed"
fi

# ── running the engine ──────────────────────────────────────────────────────
# `env -i` clears the environment so nothing on the developer's shell can
# change the result. HOME is a throwaway too: the engine derives its default
# paths from it, and a case that forgot an override must land there and not in
# the real one.
MJS="$MANIFEST_JS"
run_plugin() {
    env -i \
        PATH="$NODELESS" \
        HOME="$FAKEHOME" \
        APEX_PLUGIN_DIR="$PLUGINS" \
        APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
        APEX_PLUGIN_MANIFEST_JS="$MJS" \
        APEX_PLUGIN_VERDICT_JS="$VERDICT" \
        APEX_PLUGIN_NODE="$node_path" \
        bash "$ENGINE" "$@" 2>&1 </dev/null
}

# Source the shipped engine and call one of its functions. Sourcing runs main()
# with no arguments, which prints usage and returns 0, leaving every function
# and constant defined exactly as the image has them.
call() {
    env -i PATH="$NODELESS" HOME="$FAKEHOME" \
        APEX_PLUGIN_DIR="$PLUGINS" APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
        APEX_PLUGIN_MANIFEST_JS="$MJS" APEX_PLUGIN_VERDICT_JS="$VERDICT" \
        APEX_PLUGIN_NODE="$node_path" \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        "$f" "${a[@]}"
    ' _ "$ENGINE" "$@"
}
predicate() {
    env -i PATH="$NODELESS" HOME="$FAKEHOME" \
        APEX_PLUGIN_DIR="$PLUGINS" APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
        APEX_PLUGIN_MANIFEST_JS="$MJS" APEX_PLUGIN_VERDICT_JS="$VERDICT" \
        APEX_PLUGIN_NODE="$node_path" \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        if "$f" "${a[@]}"; then echo true; else echo false; fi
    ' _ "$ENGINE" "$@"
}

# ── fixture plugins ─────────────────────────────────────────────────────────
GOOD_QML='import QtQuick

Item {
    property var api: null
    implicitWidth: 40
    implicitHeight: 20
    Text { anchors.fill: parent; text: "hi" }
}
'

# make_plugin <root> <id> <manifest-json> [qml] [entryname]
#
# `${4-…}`, NOT `${4:-…}`. An explicitly empty fourth argument means "no .qml
# at all", which is one of the four structural refusals under test; with the
# colon form an empty argument falls back to the default and the plugin gets a
# perfectly good Widget.qml, so `entry-missing` would never be reachable and
# two assertions would silently be testing the wrong thing.
make_plugin() {
    local root="$1" id="$2" manifest="$3" qml="${4-$GOOD_QML}" entry="${5:-Widget.qml}"
    mkdir -p "${root}/${id}"
    printf '%s' "$manifest" > "${root}/${id}/plugin.json"
    if [ -n "$qml" ]; then
        printf '%s' "$qml" > "${root}/${id}/${entry}"
    fi
}

manifest_for() {
    local id="$1" extra="${2:-}"
    printf '{"id":"%s","name":"Test %s","version":"1.0.0","apiVersion":"1.0",' "$id" "$id"
    printf '"entry":"Widget.qml","extensionPoint":"bar-widget"%s}' "${extra:+,$extra}"
}

echo "── a plugin id is about to become a path, and is checked as one ───────"
# NOT a second opinion about what an id is — manifest.js's validId() is the
# rule, and it is asked separately. This is the path-safety lock that must hold
# whatever any validator thinks, because the id is interpolated into a
# filesystem path before anything has read a manifest.
is "an ordinary id"        true  "$(predicate valid_id_shape apex-worldclock)"
is "digits"                true  "$(predicate valid_id_shape clock2)"
is "a traversal"           false "$(predicate valid_id_shape ../../etc/passwd)"
is "a bare .."             false "$(predicate valid_id_shape ..)"
is "an embedded .."        false "$(predicate valid_id_shape 'a..b')"
is "an embedded slash"     false "$(predicate valid_id_shape a/b)"
is "a backslash"           false "$(predicate valid_id_shape 'a\b')"
is "a leading dot"         false "$(predicate valid_id_shape .hidden)"
is "a leading dash"        false "$(predicate valid_id_shape -rf)"
is "empty"                 false "$(predicate valid_id_shape '')"
is "65 characters"         false "$(predicate valid_id_shape "$(printf 'a%.0s' {1..65})")"
is "64 characters"         true  "$(predicate valid_id_shape "$(printf 'a%.0s' {1..64})")"

echo "── enumeration reports the four facts manifest.js cannot see ──────────"
# The same four fields, in the same order, PluginService.qml's own scan emits:
# id, .qml count, symlink count, the one .qml name. They are facts about the
# DIRECTORY, which is why they are measured here and not in the manifest.
ENUM="$WORK/enum"
mkdir -p "$ENUM"
make_plugin "$ENUM" one "$(manifest_for one)"
mkdir -p "$ENUM/notaplugin"                        # no plugin.json
make_plugin "$ENUM" two "$(manifest_for two)"
printf '%s' "$GOOD_QML" > "$ENUM/two/Second.qml"   # a second .qml
make_plugin "$ENUM" bare "$(manifest_for bare)" '' # no .qml at all
make_plugin "$ENUM" linked "$(manifest_for linked)"
ln -s /etc "$ENUM/linked/escape"

enum_out="$(call enumerate "$ENUM")"
is  "a directory with no plugin.json is not a candidate" "" \
    "$(awk -F'\t' '$1=="notaplugin"' <<<"$enum_out")"
is  "one .qml is counted as one" "one	1	0	Widget.qml" \
    "$(awk -F'\t' '$1=="one"' <<<"$enum_out")"
is  "two .qml files are counted as two" 2 \
    "$(awk -F'\t' '$1=="two"{print $2}' <<<"$enum_out")"
is  "no .qml is counted as zero" 0 \
    "$(awk -F'\t' '$1=="bare"{print $2}' <<<"$enum_out")"
# A security fact, not bookkeeping. `files` is read-only inside the plugin's own
# directory and enforced textually, so a plugin shipping a symlinked
# subdirectory turns an approved relative path into a read of the user's
# documents. Only the filesystem can see that.
is  "a symlink anywhere is counted" 1 \
    "$(awk -F'\t' '$1=="linked"{print $3}' <<<"$enum_out")"
is  "every line carries exactly four fields" 4 \
    "$(awk -F'\t' '$1=="one"{print NF}' <<<"$enum_out")"

echo "── the validator is required, and its absence is a refusal ────────────"
# The one thing this CLI must never do is guess. If manifest.js is not there
# then APEX has no opinion about a plugin, and saying so is the only honest
# answer — a fallback implementation here would be the second, drifting answer
# the whole design exists to prevent.
make_plugin "$PLUGINS" present "$(manifest_for present)"
MJS="$WORK/definitely-absent.js"
for verb in "list" "info present"; do
    # shellcheck disable=SC2086  # a fixed, space-separated verb + argument
    out=$(run_plugin $verb); rc=$?
    is  "'$verb' refuses without the validator" 1 "$rc"
    has "…and says APEX does not reimplement the rules" \
        "does not reimplement the plugin rules" "$out"
done
# And the refusal happened BEFORE anything moved. This is the assertion that
# matters most: a refusal that had already renamed a directory would be a
# refusal that changed the machine.
if [ -d "$PLUGINS/present" ] && [ ! -e "$DISABLED/present" ]; then
    ok "…having moved nothing"
else
    bad "…having moved nothing" "the plugin is no longer where it was"
fi

# `enable` and `disable`, by contrast, need NO validator, and that is
# deliberate rather than an oversight: taking a plugin out of the tree the
# shell scans is a filesystem operation, and needing a JavaScript runtime for
# it would mean a machine whose shell install is broken cannot disable the
# plugin that is breaking it — which is exactly when a user needs to.
out=$(run_plugin disable present); rc=$?
is  "disable works with no validator at all" 0 "$rc"
if [ -d "$DISABLED/present" ] && [ ! -e "$PLUGINS/present" ]; then
    ok "…and really moved the directory"
else
    bad "…and really moved the directory"
fi
out=$(run_plugin enable present); rc=$?
is  "enable works with no validator either" 0 "$rc"
# But it must not then claim the shell will accept it. This file has no opinion
# of its own about a manifest, so the honest report is "unknown".
has "…while refusing to guess the shell's verdict" \
    "whether the shell will accept it is unknown here" "$out"
hasnt "…and never claims the shell will load it" "will load it at the next start" "$out"
rm -rf "$PLUGINS/present" "$DISABLED/present"

echo "── the help text states the two things a user has to know ─────────────"
MJS="$WORK/definitely-absent.js"
help="$(run_plugin --help)"
for want in 'list [--json]' 'info <id>' 'enable <id>' 'disable <id>'; do
    has "--help mentions: $want" "$want" "$help"
done
# Claims the feature would be dishonest without. Matched against the help text
# with newlines squashed, because these sentences wrap and a line-oriented
# `grep -F` would only ever see half of each one.
flat="$(tr '\n' ' ' <<<"$help" | tr -s ' ')"
has "--help says why disabling is a move" "moving it is what actually takes effect" "$flat"
has "--help says nothing is deleted or rewritten" \
    "Nothing is ever deleted and no file is ever rewritten." "$flat"
has "--help says APEX owns no plugin rules" \
    "APEX does not reimplement the plugin rules" "$flat"
has "--help names the shell's validator" "manifest.js" "$help"
# The permission model must never read as stronger than it is.
has "--help says the model is not a sandbox" "neither one confines hostile code" "$help"
out=$(run_plugin frobnicate); rc=$?
is "an unknown verb is an error" 1 "$rc"

echo "── enable and disable move a directory and rewrite no file ────────────"
# The one write path in the feature. It is a rename: the plugin's manifest and
# .qml are carried across byte-for-byte and are never opened for writing, so
# there is no substitution to get wrong. The assertions are the shape checks
# the repo's AGENTS.md now demands of anything that touches a live config.
MJS="$WORK/definitely-absent.js"   # the moves need no validator to be checked
make_plugin "$PLUGINS" mover "$(manifest_for mover)"
printf 'extra\n' > "$PLUGINS/mover/README"
before_count=$(find "$PLUGINS/mover" -mindepth 1 | wc -l)
before_sum=$(cd "$PLUGINS/mover" && find . -type f -exec sha256sum {} + | sort)
is "count_files sees every file, not just the .qml" 3 "$(call count_files "$PLUGINS/mover")"

n=$(call move_plugin mover "$PLUGINS/mover" "$DISABLED" disable); rc=$?
is  "the move reports the file count it carried" "$before_count" "$n"
is  "…and succeeds" 0 "$rc"
if [ -d "$DISABLED/mover" ] && [ ! -e "$PLUGINS/mover" ]; then
    ok "the directory is in the new tree and gone from the old"
else
    bad "the directory is in the new tree and gone from the old"
fi
after_sum=$(cd "$DISABLED/mover" && find . -type f -exec sha256sum {} + | sort)
is "every file crossed byte-for-byte" "$before_sum" "$after_sum"

# A destination that already exists is a refusal, never an overwrite.
make_plugin "$PLUGINS" mover "$(manifest_for mover)"
out=$(call move_plugin mover "$PLUGINS/mover" "$DISABLED" disable 2>&1); rc=$?
is  "a move onto an existing directory is refused" 1 "$rc"
has "…and says so" "already exists" "$out"
if [ -d "$PLUGINS/mover" ] && [ -d "$DISABLED/mover" ]; then
    ok "…leaving both copies exactly where they were"
else
    bad "…leaving both copies exactly where they were"
fi

# A plugin in BOTH trees is ambiguous. Guessing would be the failure mode:
# whichever one APEX picked, the other is the user's and it would look deleted.
out=$(run_plugin disable mover 2>&1); rc=$?
is  "a plugin in both trees is refused, not merged" 1 "$rc"
has "…and refuses to guess"  "will not guess which one is real" "$out"
out=$(run_plugin enable mover 2>&1); rc=$?
is  "…in both directions" 1 "$rc"
rm -rf "$DISABLED/mover"

# An empty directory is refused: there is nothing to move, and moving it would
# report a success that carried no files.
mkdir -p "$PLUGINS/hollow"
out=$(call move_plugin hollow "$PLUGINS/hollow" "$DISABLED" disable 2>&1); rc=$?
is  "an empty directory is not moved" 1 "$rc"
has "…and says why"  "refusing to disable" "$out"
rmdir "$PLUGINS/hollow" 2>/dev/null

out=$(run_plugin disable nosuch 2>&1); rc=$?
is  "disabling something that is not there fails" 1 "$rc"
out=$(run_plugin enable nosuch 2>&1); rc=$?
is  "enabling something that is not there fails" 1 "$rc"
out=$(run_plugin disable ../../etc 2>&1); rc=$?
is  "a traversal is refused before any path is built" 1 "$rc"
has "…saying it would leave the plugin directory" "would not stay inside" "$out"

# Nothing in this file deletes. Asserted over the whole run at the end too, but
# stated here because it is the property that makes the move safe to offer.
if [ -d "$PLUGINS/mover" ]; then ok "the plugin survived every refusal above"
else bad "the plugin survived every refusal above" "it is gone"; fi
rm -rf "$PLUGINS/mover"

# ═════════════════════════════════════════════════════════════════════════════
echo "── the shell's own validator: verdicts, and the structural tripwire ───"
# ═════════════════════════════════════════════════════════════════════════════
if [ -z "$MANIFEST_JS" ] || [ ! -f "$MANIFEST_JS" ]; then
    bad "apex-shell's plugin validator is present" \
        "not at ${MANIFEST_JS:-<no apex-shell tree found>}. This file HAS been on apex-shell main since PR #9 merged, so the usual cause is not a missing merge: this run resolved an apex-shell tree that predates it — most often a stale /usr/share/apex-shell installed on a development machine, which is the fallback when no checkout is found. The tree that answered is printed at the top of this run. Point the suite at a current one with APEX_SHELL_TREE=/path/to/apex-shell."
    printf '\n'
    printf 'apex-plugin: %d passed, %d failed\n' "$pass" "$fail"
    printf 'The verdict and tripwire sections did not run: no apex-shell tree carrying the validator.\n'
    exit 1
fi
ok "apex-shell's plugin validator is present"
MJS="$MANIFEST_JS"

# The control for the PATH threading set up at the top of this file: a real
# verdict, produced with a PATH that contains no node at all. If the engine
# resolved node off PATH rather than honouring APEX_PLUGIN_NODE, this is the
# assertion that fails — and it is what stands between "passes on Fedora" and
# "passes on a runner where node is in the toolcache".
make_plugin "$PLUGINS" pathcheck "$(manifest_for pathcheck)"
nodeless_out="$(env -i PATH="$NODELESS" HOME="$FAKEHOME" \
    APEX_PLUGIN_DIR="$PLUGINS" APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
    APEX_PLUGIN_MANIFEST_JS="$MJS" APEX_PLUGIN_VERDICT_JS="$VERDICT" \
    APEX_PLUGIN_NODE="$node_path" \
    bash "$ENGINE" list 2>&1)"
has "a verdict is produced with no node on PATH at all" "pathcheck" "$nodeless_out"
hasnt "…and it is not the node-is-missing refusal" "node is not available" "$nodeless_out"
# The other half: without the override, the same PATH must produce the refusal
# rather than a wrong answer. Otherwise the assertion above proves nothing —
# node could simply be reachable some other way.
nodeless_bare="$(env -i PATH="$NODELESS" HOME="$FAKEHOME" \
    APEX_PLUGIN_DIR="$PLUGINS" APEX_PLUGIN_DISABLED_DIR="$DISABLED" \
    APEX_PLUGIN_MANIFEST_JS="$MJS" APEX_PLUGIN_VERDICT_JS="$VERDICT" \
    bash "$ENGINE" list 2>&1)"
has "…and without the override it refuses, naming node" "node is not available" "$nodeless_bare"
rm -rf "$PLUGINS/pathcheck"

# ── the tripwire on the duplication this feature could not avoid ────────────
# Four refusals live in PluginService.qml rather than manifest.js, because they
# are facts about the directory: a symlink present, no .qml, more than one
# .qml, and an `entry` that is not the one .qml there. `apex-plugin` measures
# those facts and the node shim applies them in the same order.
#
# That is the ONLY duplication in this feature, and it is duplicated because it
# lives in QML that bash cannot execute. So it gets a tripwire: if the shell
# grows a fifth structural refusal, or changes the fields its own scan emits,
# these assertions fail and somebody has to come and update apex-plugin. Better
# than hoping.
if [ -f "$PLUGIN_SERVICE" ]; then
    ok "PluginService.qml is present to check against"
    # Refusals with a LITERAL reason string are the ones NOT delegated to
    # manifest.js. There are five: the four structural ones and `load-error`,
    # which needs a running QML engine and is therefore not reachable from a
    # CLI at all. A sixth means new duplication to carry.
    literal="$(grep -oE '_refuse\("[a-z-]+"' "$PLUGIN_SERVICE" | sort | uniq -c | sort -rn)"
    count="$(grep -cE '_refuse\("[a-z-]+"' "$PLUGIN_SERVICE")"
    is "PluginService.qml still has exactly 5 literal-reason refusals" 5 "$count"
    for reason in entry-outside-plugin entry-missing extra-qml load-error; do
        has "…including $reason" "$reason" "$literal"
    done
    # The four fields its scan emits, in order. `apex-plugin`'s `enumerate`
    # reproduces this exactly; a fifth field or a reorder silently changes what
    # the CLI is measuring.
    has "PluginService.qml's scan still emits four tab-separated fields" \
        "%s\\\\t%s\\\\t%s\\\\t%s\\\\n" "$(cat "$PLUGIN_SERVICE")"
    has "…and still counts symlinks with find -type l" \
        'find "$p" -type l' "$(cat "$PLUGIN_SERVICE")"
    # And the delegation itself: if PluginService stopped calling manifest.js,
    # this CLI would be asking a file the shell no longer consults.
    has "PluginService.qml still gets its manifest verdict from manifest.js" \
        "Manifest.validateManifest" "$(cat "$PLUGIN_SERVICE")"
    has "…and its source scan too" "Manifest.scanSource" "$(cat "$PLUGIN_SERVICE")"
else
    bad "PluginService.qml is present to check against" \
        "not at ${PLUGIN_SERVICE:-<none>}; the structural-duplication tripwire cannot run"
fi

# ── the fixture corpus ──────────────────────────────────────────────────────
CORPUS="$WORK/corpus"
mkdir -p "$CORPUS"

make_plugin "$CORPUS" good "$(manifest_for good '"permissions":["files"]')"
make_plugin "$CORPUS" badjson '{"id":"badjson", oops'
make_plugin "$CORPUS" mismatch "$(manifest_for somethingelse)"
make_plugin "$CORPUS" sysperm "$(manifest_for sysperm '"permissions":["system"]')"
make_plugin "$CORPUS" nethosts "$(manifest_for nethosts '"permissions":["network"]')"
make_plugin "$CORPUS" futureapi \
    '{"id":"futureapi","name":"F","version":"1.0.0","apiVersion":"1.9","entry":"Widget.qml","extensionPoint":"bar-widget"}'
make_plugin "$CORPUS" badpoint \
    '{"id":"badpoint","name":"B","version":"1.0.0","apiVersion":"1.0","entry":"Widget.qml","extensionPoint":"sidebar"}'
make_plugin "$CORPUS" badimport "$(manifest_for badimport)" \
    'import QtQuick
import Quickshell.Io

Item { Process { command: ["id"] } }
'
make_plugin "$CORPUS" dynamic "$(manifest_for dynamic)" \
    'import QtQuick

Item {
    Component.onCompleted: Qt.createQmlObject("import QtQuick; Item {}", this)
}
'
# The four structural cases — the duplicated ones.
make_plugin "$CORPUS" twoqml "$(manifest_for twoqml)"
printf '%s' "$GOOD_QML" > "$CORPUS/twoqml/Second.qml"
make_plugin "$CORPUS" noqml "$(manifest_for noqml)" ''
make_plugin "$CORPUS" symlinked "$(manifest_for symlinked)"
ln -s /etc "$CORPUS/symlinked/escape"
make_plugin "$CORPUS" entrywrong \
    '{"id":"entrywrong","name":"E","version":"1.0.0","apiVersion":"1.0","entry":"Other.qml","extensionPoint":"bar-widget"}'

# The oracle: manifest.js itself, asked directly. This is what makes "the CLI
# owns no rules" a checked fact rather than a design note — if the CLI invented
# a reason code, remapped one, or summarised a refusal into a different one,
# these comparisons fail.
oracle_reason() {
    "$node_path" -e '
const M = require(process.argv[1]);
const fs = require("fs");
let raw = "";
try { raw = fs.readFileSync(process.argv[2] + "/plugin.json", "utf8"); } catch (e) {}
const g = M.validateManifest(raw, process.argv[3], M.API_VERSION);
process.stdout.write(g.ok ? "ok" : g.reason);
' "$MANIFEST_JS" "$1" "$2"
}

PLUGINS_SAVE="$PLUGINS"
PLUGINS="$CORPUS"
corpus_json="$(run_plugin list --json)"
PLUGINS="$PLUGINS_SAVE"

cli_reason() {
    jq -r --arg id "$1" '.[] | select(.id == $id) | if .ok then "ok" else .reason end' \
        <<<"$corpus_json"
}

echo "── the CLI's verdict IS manifest.js's verdict, compared case by case ──"
for id in good badjson mismatch sysperm nethosts futureapi badpoint; do
    want="$(oracle_reason "$CORPUS/$id" "$id")"
    got="$(cli_reason "$id")"
    if [ -n "$want" ] && [ "$want" = "$got" ]; then
        ok "manifest.js and the CLI agree on '$id' ($want)"
    else
        bad "manifest.js and the CLI agree on '$id'" \
            "manifest.js says $(printf '%q' "$want"), the CLI says $(printf '%q' "$got")"
    fi
done
# Two the oracle above cannot reach, because they are decided by scanSource()
# rather than validateManifest(). Asserted against the reason codes manifest.js
# defines, which is still its vocabulary and not one invented here.
is "an import outside the allowlist is refused" forbidden-import "$(cli_reason badimport)"
is "dynamic QML construction is refused"        forbidden-construct "$(cli_reason dynamic)"

echo "── the four structural refusals, the only duplicated decisions ────────"
is "a symlink in the directory refuses the plugin" entry-outside-plugin "$(cli_reason symlinked)"
is "no .qml is entry-missing"                      entry-missing "$(cli_reason noqml)"
is "two .qml files are extra-qml"                  extra-qml     "$(cli_reason twoqml)"
is "an entry that is not the one .qml there"       entry-missing "$(cli_reason entrywrong)"

echo "── list and info report the grant, not a summary of it ────────────────"
PLUGINS="$CORPUS"
out="$(run_plugin list)"
has "list has a VALID column, not a LOADED one" "VALID" "$out"
has "list names a valid plugin"      "good" "$out"
has "…and explains an invalid one"   "plugin.json is not valid JSON" "$out"

info="$(run_plugin info good)"
has "info reports the entry point"   "bar-widget" "$info"
has "info reports the granted permission" "files" "$info"
# The distinction the shell makes by refusing at load rather than granting
# silently. A permission field that grants nothing still reads as a capability
# somebody reviewed, which is why these three are refused and not accepted.
for p in system secrets location; do
    has "info names $p as unenforceable" "$p" "$(grep unenforceable <<<"$info")"
done
has "info states the platform is not a sandbox" "neither one confines" "$info"
has "info reports the host's API version" "this shell implements" "$info"

info="$(run_plugin info badjson)"
has "info on a refused plugin says which reason code" "manifest-unparseable" "$info"
has "…and the shell's own English for it" "not valid JSON" "$info"
PLUGINS="$PLUGINS_SAVE"

echo "── end to end: disable, list, enable, against the real validator ──────"
make_plugin "$PLUGINS" e2e "$(manifest_for e2e '"permissions":["files"]')"
out=$(run_plugin disable e2e); rc=$?
is  "disable succeeds" 0 "$rc"
has "…and says the running shell keeps it until restart" \
    "nothing here can unload a plugin from a live shell" "$out"
if [ -d "$DISABLED/e2e" ] && [ ! -e "$PLUGINS/e2e" ]; then
    ok "…and the directory is out of the tree the shell scans"
else
    bad "…and the directory is out of the tree the shell scans"
fi
has "list reports it as disabled" "disabled" "$(run_plugin list)"
is  "…and still valid, because disabled is not broken" "ok" "$(
    corpus_json="$(run_plugin list --json)"; cli_reason e2e)"

out=$(run_plugin disable e2e); rc=$?
is  "disabling twice is a no-op, not an error" 0 "$rc"
has "…and says it is already disabled" "already disabled" "$out"

out=$(run_plugin enable e2e); rc=$?
is  "enable succeeds" 0 "$rc"
has "…and says the shell will load it" "will load it at the next start" "$out"
out=$(run_plugin enable e2e); rc=$?
is  "enabling twice is a no-op, not an error" 0 "$rc"

# Enabling a plugin the shell will refuse is not an error — the user may be
# about to fix it — but reporting it as done without saying so would be the
# "reports success having changed nothing useful" case apexd/AGENTS.md forbids.
make_plugin "$DISABLED" broken "$(manifest_for broken '"permissions":["system"]')"
out=$(run_plugin enable broken); rc=$?
is  "enabling a plugin the shell refuses still succeeds" 0 "$rc"
has "…and says the shell will REFUSE it"  "will still REFUSE it" "$out"
has "…with the shell's own reason"        "cannot enforce that permission" "$out"

echo "── nothing was deleted and nothing outside the temp tree was touched ──"
# The engine ran dozens of times with HOME pointed at a directory it should
# never write to. Anything there means a path was derived from $HOME instead of
# the overrides — which on a real machine is the developer's live shell
# configuration.
stray=$(find "$FAKEHOME" -mindepth 1 2>/dev/null | head -5)
if [ -z "$stray" ]; then ok "the fake HOME is still empty"
else bad "the fake HOME is still empty" "found: $(tr '\n' ' ' <<<"$stray")"; fi
# Every fixture that was created still exists somewhere. No verb in this file
# deletes, so a missing one is a bug and not a cleanup.
for id in e2e broken; do
    if [ -d "$PLUGINS/$id" ] || [ -d "$DISABLED/$id" ]; then
        ok "'$id' still exists somewhere; nothing was deleted"
    else
        bad "'$id' still exists somewhere; nothing was deleted" "it is gone"
    fi
done

echo
printf 'apex-plugin: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
