#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-resolve.sh — assertions against the SHIPPED universal resolver,
#  the §9 half of files/system/libexec/apex-pkg. Nothing here re-implements it:
#  every case either runs the script as a process or sources it and calls the
#  function the image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  §9's promise is that a user does not have to understand RPM vs Flatpak vs a
#  containerised distro package. Delivering that means the engine now makes a
#  CHOICE on the user's behalf, and a choice is a policy — the kind that is
#  invisible until someone reads the source.
#
#  The two sharp edges:
#
#    1. `apex install <name>` already ships. It runs from a user's terminal and
#       its behaviour for a bare name is covered by 54 existing assertions. A
#       resolver that silently re-routed a name which installs an RPM today
#       would be a behaviour change on a shipped command, so "an exact-name
#       repository package still wins" is pinned here rather than assumed.
#    2. The one thing that CAN move a name off that route is the curated table,
#       and a re-route the user did not ask for has to justify itself on the
#       spot and stay overridable. Both are asserted, including that the
#       override actually overrides.
#
#  ── How it runs with no network ─────────────────────────────────────────────
#  `dnf5` and `flatpak` are faked on $PATH, and the suite HARD-EXITS if they do
#  not resolve to the fakes — the failure mode is a refusal, never a run that
#  quietly queries Fedora's mirrors or the developer's Flatpak remotes. The
#  fakes answer from fixture files, so both "this exists" and "this does not"
#  are reachable, which is what makes the ranking testable at all.
#
#  The capsule probe is switched off with APEX_PKG_NO_CAPSULE=1 so the answer
#  does not depend on whether the person running the suite happens to have made
#  a capsule; the capsule leg of the ranking is asserted directly instead,
#  through rank_sources, for every combination.
#
#  No root, no network, no writes outside a temp directory, nothing installed
#  or built. There are no skips: every case runs against the fakes.
#
#  PASS = every case prints exactly what it should, with a non-zero exit where
#         one is expected.
#
#  Run from anywhere: ./tests/test-apex-resolve.sh
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`, as in every suite here: this one COUNTS failures instead of
# aborting, and many assertions run commands that exit non-zero on purpose.
# GitHub Actions invokes a script as `bash -e {0}`, and under `-e` the first
# such command would end the run silently rather than report anything.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }
ENGINE=$(cd "$(dirname "$ENGINE")" && pwd)/$(basename "$ENGINE")

WORK=$(mktemp -d /tmp/apex-resolve-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0

ok()  { printf 'PASS  %-54s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-54s %s\n' "$1" "$2"; fail=$((fail+1)); }

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

# ── the fake package sources ────────────────────────────────────────────────
# Two fixture files decide what exists. Each fake matches the name it was asked
# for and prints in the format the real tool prints, which is the format the
# engine parses — so a change to either parser is caught here.
BIN="$WORK/bin"
FAKEHOME="$WORK/home"
mkdir -p "$BIN" "$FAKEHOME"

# `name|evr|repo|summary`, one per line, as dnf5 repoquery --queryformat emits.
cat > "$WORK/repo-packages" <<'EOF'
htop|3.3.0-4.fc43|fedora|Interactive process viewer
neovim|0.10.2-1.fc43|fedora|Vim-fork focused on extensibility
discord|0.0.75-1.fc43|rpmfusion-nonfree|All-in-one voice and text chat for gamers
android-tools|35.0.2-2.fc43|fedora|Android platform tools
EOF

# `appid<TAB>name<TAB>remotes`, as flatpak search --columns emits.
printf 'com.discordapp.Discord\tDiscord\tflathub\n'          >  "$WORK/flatpak-apps"
printf 'io.neovim.nvim\tNeovim\tflathub\n'                   >> "$WORK/flatpak-apps"
printf 'org.mozilla.firefox\tFirefox\tflathub\n'             >> "$WORK/flatpak-apps"
# A description-only match: `flatpak search` matches descriptions too, so this
# is the row that must NOT count as a candidate for "htop".
printf 'org.example.Monitor\tSystem Monitor\tflathub\n'      >> "$WORK/flatpak-apps"

cat > "$BIN/dnf5" <<EOF
#!/usr/bin/env bash
# repoquery answers exact names (what the resolver asks); search matches
# summaries too (what \`apex search\` asks). Anything else is a test bug and
# must be loud rather than quietly returning nothing.
case "\${1:-}" in
    repoquery)
        name="\${@: -1}"
        grep -E "^\${name}\|" "$WORK/repo-packages" || true
        exit 0
        ;;
    search)
        term="\${@: -1}"
        grep -iF -- "\${term}" "$WORK/repo-packages" || true
        exit 0
        ;;
esac
echo "fake dnf5: unexpected subcommand '\${1:-}'" >&2
exit 3
EOF

cat > "$BIN/flatpak" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
    search)
        term="\${@: -1}"
        # The real thing matches ids, names AND descriptions. Matching loosely
        # here is the point: the engine's own filter is what must be strict.
        grep -iF -- "\${term}" "$WORK/flatpak-apps" || true
        exit 0
        ;;
    remotes) echo flathub; exit 0 ;;
esac
exit 0
EOF
chmod +x "$BIN/dnf5" "$BIN/flatpak"

SAFE_PATH="$BIN:/usr/bin:/bin"

# Run the shipped engine as a process, against the fakes only.
run_pkg() {
    env -i \
        PATH="$SAFE_PATH" \
        HOME="$FAKEHOME" \
        USER=tester \
        APEX_PKG_NO_CAPSULE=1 \
        bash "$ENGINE" "$@" 2>&1 </dev/null
}

call() {
    env -i PATH="$SAFE_PATH" HOME="$FAKEHOME" USER=tester APEX_PKG_NO_CAPSULE=1 \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        "$f" "${a[@]}"
    ' _ "$ENGINE" "$@"
}
predicate() {
    env -i PATH="$SAFE_PATH" HOME="$FAKEHOME" USER=tester APEX_PKG_NO_CAPSULE=1 \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        if "$f" "${a[@]}"; then echo true; else echo false; fi
    ' _ "$ENGINE" "$@"
}

echo "── the fakes must be the only package tooling reachable ───────────────"
# The guard, not a formality. Without it every case below would query Fedora's
# mirrors and the developer's real Flatpak remotes.
for tool in dnf5 flatpak; do
    resolved=$(env -i PATH="$SAFE_PATH" bash -c "command -v $tool")
    if [ "$resolved" != "$BIN/$tool" ]; then
        echo "FATAL: $tool resolves to '$resolved', not the fake at $BIN/$tool" >&2
        echo "Refusing to run: this suite would query the real package sources." >&2
        exit 2
    fi
    ok "$tool resolves to the fake"
done

echo "── the ranking is the policy, so it is asserted directly ──────────────"
# Every combination. The order is the claim: a repository package first
# because it puts the command on $PATH and rolls back with the extension, a
# Flatpak next because sandboxed beats nothing, a capsule last because the
# software then lives in a container rather than on the host.
is "all three available"      "rpm flatpak capsule" "$(call rank_sources 1 1 1 | tr '\n' ' ' | sed 's/ $//')"
is "rpm and flatpak"          "rpm flatpak"         "$(call rank_sources 1 1 0 | tr '\n' ' ' | sed 's/ $//')"
is "rpm and capsule"          "rpm capsule"         "$(call rank_sources 1 0 1 | tr '\n' ' ' | sed 's/ $//')"
is "flatpak and capsule"      "flatpak capsule"     "$(call rank_sources 0 1 1 | tr '\n' ' ' | sed 's/ $//')"
is "rpm only"                 "rpm"                 "$(call rank_sources 1 0 0 | tr '\n' ' ' | sed 's/ $//')"
is "flatpak only"             "flatpak"             "$(call rank_sources 0 1 0 | tr '\n' ' ' | sed 's/ $//')"
is "capsule only"             "capsule"             "$(call rank_sources 0 0 1 | tr '\n' ' ' | sed 's/ $//')"
is "nothing available"        ""                    "$(call rank_sources 0 0 0)"

echo "── a Flathub row must answer the question, not merely contain the word ─"
# `flatpak search` matches descriptions, so without this filter searching for
# "code" would report fifty candidates that merely mention it.
is "id last segment matches"  true  "$(predicate flatpak_match nvim io.neovim.nvim Neovim)"
is "application name matches" true  "$(predicate flatpak_match neovim io.neovim.nvim Neovim)"
is "case is ignored"          true  "$(predicate flatpak_match NEOVIM io.neovim.nvim Neovim)"
is "the full id matches"      true  "$(predicate flatpak_match org.mozilla.firefox org.mozilla.firefox Firefox)"
is "a description hit is not a match" false \
   "$(predicate flatpak_match htop org.example.Monitor 'System Monitor')"
is "a prefix is not a match"  false "$(predicate flatpak_match neo io.neovim.nvim Neovim)"
is "an empty term matches nothing" false "$(predicate flatpak_match '' io.neovim.nvim Neovim)"

echo "── every source states who vouched for what it installs ───────────────"
for s in rpm flatpak capsule; do
    text="$(call provenance "$s")"
    if [ -n "$text" ]; then ok "provenance for $s is stated"
    else bad "provenance for $s is stated" "empty"; fi
done
has "rpm provenance names the keys"   "signature checked" "$(call provenance rpm)"
has "rpm provenance names rollback"   "rolls back"        "$(call provenance rpm)"
has "flatpak provenance names the sandbox" "sandboxed"    "$(call provenance flatpak)"
has "capsule provenance says it is not on the host" "invisible to the host" \
    "$(call provenance capsule)"
call provenance nonsense >/dev/null 2>&1
is "an unknown source has no provenance" 1 "$?"

is "rpm is a source"      true  "$(predicate valid_source rpm)"
is "flatpak is a source"  true  "$(predicate valid_source flatpak)"
is "capsule is a source"  true  "$(predicate valid_source capsule)"
is "appimage is not"      false "$(predicate valid_source appimage)"
is "an empty source is not" false "$(predicate valid_source '')"

echo "── resolve: a repository package still wins, and says why ─────────────"
out=$(run_pkg resolve htop)
has "htop is found in the repositories" "htop-3.3.0-4.fc43" "$out"
has "…with its repository named"        "repo fedora"       "$out"
has "…and rpm is what APEX would use"   "APEX would use: rpm" "$out"
has "…with provenance stated"           "provenance:"       "$out"
# The Flathub row whose description merely mentions the term must not appear.
hasnt "a description-only Flathub hit is not offered" "org.example.Monitor" "$out"

out=$(run_pkg resolve neovim)
has "neovim resolves to the RPM"        "APEX would use: rpm" "$out"
# Both are real candidates, so the loser has to be printed with a command the
# user can actually run — "you could use a Flatpak" is not an instruction.
has "…and the Flatpak is offered as an alternative" "io.neovim.nvim" "$out"
has "…as a runnable command"            "sudo apex install io.neovim.nvim" "$out"
# …under an "or:" label, so it reads as the alternative rather than as a
# second thing to run.
has "…labelled as the alternative"      "or:" "$out"

echo "── resolve: a Flatpak-only name is chosen, with its real id ───────────"
out=$(run_pkg resolve firefox)
has "firefox has no RPM here"           "APEX would use: flatpak" "$out"
has "…and the id is looked up, not guessed" "org.mozilla.firefox" "$out"

echo "── resolve: the curated table re-routes, and justifies itself ─────────"
# The one case where APEX moves a name off the repository route. It must say
# so, say why, and leave the RPM reachable.
out=$(run_pkg resolve discord)
has "discord is re-routed to Flathub"  "APEX would use: flatpak" "$out"
has "…naming the reason"               "refuses to start once it considers itself out of date" "$out"
has "…and the RPM stays reachable"     "--source rpm discord" "$out"
has "…with the RPM still listed as a candidate" "rpmfusion-nonfree" "$out"

# The table is deliberately tiny. A name that is not in it must not be moved.
out=$(run_pkg resolve android-tools)
has "an uncurated name keeps the RPM" "APEX would use: rpm" "$out"
hasnt "…with no re-route explanation" "APEX prefers this source" "$out"

echo "── resolve: what cannot be installed at all says so ───────────────────"
out=$(run_pkg resolve nosuchpackage); rc=$?
is  "an unknown name fails"            1 "$rc"
has "…and suggests a search"           "apex search nosuchpackage" "$out"
has "…and a COPR"                      "apex repo enable-copr" "$out"
has "…and a capsule"                   "apex env create fedora" "$out"

out=$(run_pkg resolve glibc); rc=$?
is  "a core-system package is refused" 1 "$rc"
has "…explaining that no source can"   "No source can provide it" "$out"
out=$(run_pkg resolve kernel); rc=$?
is  "a kernel is refused"              1 "$rc"

echo "── resolve: the two syntactic forms decide themselves ─────────────────"
out=$(run_pkg resolve org.gimp.GIMP)
has "an application id needs no ranking" "nothing to choose" "$out"
has "…and is a Flatpak"                  "provenance: signed by its Flatpak remote" "$out"
out=$(run_pkg resolve ./some.rpm)
has "a file path needs no ranking"       "nothing to choose" "$out"
has "…and is an RPM"                     "provenance: signature checked" "$out"

out=$(run_pkg resolve); rc=$?
is "resolve with no name is a usage error" 1 "$rc"

echo "── --source is the escape hatch, and it always wins ───────────────────"
# Every case here stops at the root gate, which is exactly far enough: the
# routing decision has already been made by then, and nothing has been
# installed. Running as root would reach the real install path, so the suite
# refuses to be useful there rather than risk it.
if [ "$(id -u)" = 0 ]; then
    echo "FATAL: this suite must not run as root — it would reach the install path" >&2
    exit 2
fi
ok "not running as root, so the install path is unreachable"

out=$(run_pkg install --source turbo htop); rc=$?
is  "an unknown source is refused"   1 "$rc"
has "…listing the ones that exist"   "rpm, flatpak, capsule" "$out"

out=$(run_pkg install --source flatpak ./x.rpm); rc=$?
is  "--source flatpak on a file is refused" 1 "$rc"
has "…saying a file is always an RPM" "a file is always installed as an RPM" "$out"

out=$(run_pkg install --source rpm org.gimp.GIMP); rc=$?
is  "--source rpm on an app id is refused" 1 "$rc"
has "…saying an id is always a Flatpak" "always installed as a Flatpak" "$out"

# The override for the curated re-route. Without this, a user who wants the
# repository build of a curated package has no way to say so.
out=$(run_pkg install --source rpm discord)
hasnt "--source rpm suppresses the re-route" "installing discord as a Flatpak" "$out"
has   "…and reaches the root gate as an RPM install" "this needs root" "$out"

out=$(run_pkg install --source flatpak neovim)
has "--source flatpak looks the id up" "resolves to the Flatpak io.neovim.nvim" "$out"

out=$(run_pkg install --source flatpak nosuchpackage); rc=$?
is  "--source flatpak with no match fails" 1 "$rc"
has "…rather than inventing an id"         "no Flatpak application matches" "$out"

echo "── install: the curated re-route happens on the real command too ──────"
# resolve is advisory; this is the one that actually changes what is installed.
out=$(run_pkg install discord)
has "discord installs as a Flatpak"   "installing discord as a Flatpak instead of the RPM" "$out"
has "…with the reason on the spot"    "refuses to start once it considers itself out of date" "$out"
has "…and the override spelled out"   "sudo apex install --source rpm discord" "$out"

out=$(run_pkg install htop)
hasnt "an uncurated name is not re-routed" "instead of the RPM" "$out"
has   "…and goes to the root gate"         "this needs root" "$out"

echo "── the capsule source refuses root rather than reporting nothing ──────"
# A capsule belongs to a user; root has none. `sudo apex install --source
# capsule` would report an empty capsule list on every machine in the world,
# which reads as "capsules are broken".
cat > "$BIN/id" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
    -u) echo 0 ;;
    -un) echo root ;;
    *) exec /usr/bin/id "$@" ;;
esac
EOF
chmod +x "$BIN/id"
out=$(run_pkg install --source capsule htop); rc=$?
is  "root is refused"                1 "$rc"
has "…and told the command that works" "apex install --source capsule htop" "$out"
hasnt "…without claiming there are none" "no capsule to install into" "$out"
rm -f "$BIN/id"

echo "── the install paths state provenance where the user can see it ───────"
# STATIC, and deliberately so. Both of these lines are printed AFTER the root
# gate, so a suite that must never install anything cannot reach them — and
# without this check, deleting either one changes nothing that any assertion
# notices. Found by mutation testing: removing the rpm line left the whole
# suite green.
#
# The check is on the shipped file, not on a copy, and it names the function
# rather than the file so a provenance line that drifts out of cmd_install
# still fails.
install_body=$(awk '/^cmd_install\(\) \{/,/^\}/' "$ENGINE")
has "the RPM route states provenance"     'provenance rpm'     "$install_body"
has "the Flatpak route states provenance" 'provenance flatpak' "$install_body"

echo "── search reports more than one source ────────────────────────────────"
out=$(run_pkg search neovim)
has "search names the repository section" "repository packages" "$out"
has "search names the Flatpak section"    "Flatpak applications" "$out"
has "search points at the resolver"       "apex resolve" "$out"

echo "── the engine advertises the new surface ──────────────────────────────"
help=$(run_pkg --help)
for want in 'resolve <name>' '--source rpm|flatpak|capsule' '--env NAME'; do
    has "--help mentions: $want" "$want" "$help"
done

echo "── nothing escaped the sandbox ────────────────────────────────────────"
stray=$(find "$FAKEHOME" -mindepth 1 2>/dev/null | head -5)
if [ -z "$stray" ]; then ok "the fake HOME is still empty"
else bad "the fake HOME is still empty" "found: $(tr '\n' ' ' <<<"$stray")"; fi

echo
printf 'apex-resolve: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
