#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-env.sh — assertions against the SHIPPED capsule engine,
#  files/system/libexec/apex-env. Nothing here re-implements it: every case
#  either runs the script as a process or sources it and calls the function the
#  image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `apex env create` is the one verb in APEX that pulls hundreds of megabytes
#  and makes a container. It can never run for real in CI, and it must never
#  run for real on a developer's machine during a test — a previous suite in
#  this repository switched the developer's live CPU scheduler and another
#  raised a keyring dialog, both because a test reached the host.
#
#  So the container manager is faked. $PATH is reduced to a directory holding
#  recording stubs for `podman` and `distrobox`, and the suite REFUSES TO RUN
#  if `command -v distrobox` does not resolve to the stub. The engine's own
#  argv construction is a pure function precisely so that the flags that matter
#  — the NVIDIA driver passthrough, ROCm's /dev/kfd and render group — can be
#  asserted exactly without a GPU, a container or a network.
#
#  ── What is asserted ────────────────────────────────────────────────────────
#    * capsule names, which become container names AND file paths under
#      ~/.local/share/apex/env, so a traversal must be refused not concatenated
#    * the image alias table, including that `cuda` and `rocm` really do come
#      out with a device profile attached
#    * the exact distrobox argv per device profile
#    * the rootless preflight: root is refused, and an account with no
#      subordinate uid range is refused BEFORE anything is downloaded
#    * that `rm` will not remove a container APEX did not create
#    * the per-capsule package-manager mapping §9 routes `--source capsule` to
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  No container is created, no image is pulled, no network is used, and nothing
#  is written outside a temp directory — asserted at the end by checking that
#  the fake HOME is still empty. There are no skips: every case runs against
#  the stubs, so a machine without podman gets the same result as one with it.
#
#  PASS = every case prints exactly what it should, with a non-zero exit where
#         one is expected.
#
#  Run from anywhere: ./tests/test-apex-env.sh
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`, as in every suite here: this one COUNTS failures instead of
# aborting, and many assertions run commands that exit non-zero on purpose.
# GitHub Actions invokes a script as `bash -e {0}`, and under `-e` the first
# such command would end the run silently rather than report anything.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-env
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }
ENGINE=$(cd "$(dirname "$ENGINE")" && pwd)/$(basename "$ENGINE")

WORK=$(mktemp -d /tmp/apex-env-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0

ok()  { printf 'PASS  %-52s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-52s %s\n' "$1" "$2"; fail=$((fail+1)); }

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

# ── the sandbox the whole suite runs in ─────────────────────────────────────
# A fake HOME, a fake data directory, fake subuid/subgid files, and recording
# stubs for the two external programs the engine drives. CALLS is the transcript
# every assertion about "what did it actually run" reads.
BIN="$WORK/bin"
FAKEHOME="$WORK/home"
CALLS="$WORK/calls"
mkdir -p "$BIN" "$FAKEHOME"

cat > "$BIN/distrobox" <<EOF
#!/usr/bin/env bash
{ printf 'distrobox'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
exit \${FAKE_DISTROBOX_RC:-0}
EOF

# `container exists` answers from a file the test writes, so both "the name is
# free" and "something else already has it" are reachable.
cat > "$BIN/podman" <<EOF
#!/usr/bin/env bash
{ printf 'podman'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
case "\$1 \${2:-}" in
    "container exists")
        grep -qxF "\$3" "$WORK/existing" 2>/dev/null && exit 0
        exit 1
        ;;
    "image inspect")
        echo "docker.io/library/ubuntu@sha256:1111111111111111111111111111111111111111111111111111111111111111"
        exit 0
        ;;
esac
exit 0
EOF
chmod +x "$BIN/distrobox" "$BIN/podman"
: > "$WORK/existing"

printf 'tester:100000:65536\n' > "$WORK/subuid"
printf 'tester:100000:65536\n' > "$WORK/subgid"
printf 'nobody:100000:65536\n' > "$WORK/subuid-missing"
printf 'nobody:100000:65536\n' > "$WORK/subgid-missing"

# Everything the engine needs and nothing else. jq, coreutils and friends come
# from the system directories; podman and distrobox can only come from $BIN.
SAFE_PATH="$BIN:/usr/bin:/bin"

# Run the shipped engine as a process, in the sandbox. `env -i` clears the
# environment so a variable on the developer's shell cannot change the result.
run_env() {
    : > "$CALLS"
    env -i \
        PATH="$SAFE_PATH" \
        HOME="$FAKEHOME" \
        USER=tester \
        APEX_ENV_HOME="$WORK/records" \
        APEX_ENV_SUBUID_FILE="$WORK/subuid" \
        APEX_ENV_SUBGID_FILE="$WORK/subgid" \
        "${EXTRA_ENV[@]}" \
        bash "$ENGINE" "$@" 2>&1 </dev/null
}
EXTRA_ENV=()

# Source the shipped engine and call one of its functions. Sourcing runs main()
# with no arguments, which prints usage and returns 0, leaving every function
# and constant defined exactly as the image has them.
call() {
    env -i PATH="$SAFE_PATH" HOME="$FAKEHOME" USER=tester \
        APEX_ENV_HOME="$WORK/records" \
        APEX_ENV_SUBUID_FILE="$WORK/subuid" \
        APEX_ENV_SUBGID_FILE="$WORK/subgid" \
        "${EXTRA_ENV[@]}" \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        "$f" "${a[@]}"
    ' _ "$ENGINE" "$@"
}
predicate() {
    env -i PATH="$SAFE_PATH" HOME="$FAKEHOME" USER=tester \
        APEX_ENV_HOME="$WORK/records" \
        APEX_ENV_SUBUID_FILE="$WORK/subuid" \
        APEX_ENV_SUBGID_FILE="$WORK/subgid" \
        "${EXTRA_ENV[@]}" \
        bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        if "$f" "${a[@]}"; then echo true; else echo false; fi
    ' _ "$ENGINE" "$@"
}

echo "── the stubs must be the only container tooling reachable ─────────────"
# This is the guard, not a formality. If it fails, every case below would run
# the machine's real podman and the suite would create containers on the
# developer's desktop. A hard exit, never a skip.
resolved=$(env -i PATH="$SAFE_PATH" bash -c 'command -v distrobox')
if [ "$resolved" != "$BIN/distrobox" ]; then
    echo "FATAL: distrobox resolves to '$resolved', not the stub at $BIN/distrobox" >&2
    echo "Refusing to run: this suite would drive the real container manager." >&2
    exit 2
fi
ok "distrobox resolves to the recording stub"
resolved=$(env -i PATH="$SAFE_PATH" bash -c 'command -v podman')
if [ "$resolved" != "$BIN/podman" ]; then
    echo "FATAL: podman resolves to '$resolved', not the stub at $BIN/podman" >&2
    exit 2
fi
ok "podman resolves to the recording stub"

echo "── capsule names become container names AND paths under \$XDG_DATA_HOME ─"
is "ordinary name"        true  "$(predicate valid_env_name fedora)"
is "digits and dashes"    true  "$(predicate valid_env_name ml-2024)"
is "dot and underscore"   true  "$(predicate valid_env_name py_3.13)"
is "traversal"            false "$(predicate valid_env_name ../../etc/passwd)"
is "a bare .."            false "$(predicate valid_env_name ..)"
is "embedded .."          false "$(predicate valid_env_name 'a..b')"
is "embedded slash"       false "$(predicate valid_env_name a/b)"
is "leading dash"         false "$(predicate valid_env_name -rf)"
is "uppercase"            false "$(predicate valid_env_name Fedora)"
is "empty"                false "$(predicate valid_env_name '')"
is "a space"              false "$(predicate valid_env_name 'a b')"
is "a newline"            false "$(predicate valid_env_name 'a
b')"
is "41 characters"        false "$(predicate valid_env_name "$(printf 'a%.0s' {1..41})")"
is "40 characters"        true  "$(predicate valid_env_name "$(printf 'a%.0s' {1..40})")"

echo "── an image reference cannot smuggle a second argument ────────────────"
# The argv is newline-delimited on its way to distrobox, so whitespace in a
# reference would split one argument into two.
is "a normal reference"   true  "$(predicate valid_image_ref docker.io/library/ubuntu:24.04)"
is "a digest reference"   true  "$(predicate valid_image_ref 'quay.io/x/y@sha256:abc')"
is "a reference with a space" false "$(predicate valid_image_ref 'ubuntu --privileged')"
is "a reference with a newline" false "$(predicate valid_image_ref 'ubuntu
--privileged')"
is "a reference that is a flag" false "$(predicate valid_image_ref '--rm')"
is "an empty reference"   false "$(predicate valid_image_ref '')"
is "a relative home path" false "$(predicate valid_home_path caps/x)"
is "an absolute home path" true "$(predicate valid_home_path /var/home/t/caps)"

echo "── the named images §8 asks for all resolve ───────────────────────────"
for a in fedora ubuntu arch debian python cuda rocm; do
    img=$(call alias_image "$a")
    if [ -n "$img" ]; then ok "alias '$a' has an image"
    else bad "alias '$a' has an image" "empty"; fi
done
has "fedora follows the host release" "fedora-toolbox:" "$(call alias_image fedora)"
is  "ubuntu is pinned to the LTS" "docker.io/library/ubuntu:24.04" "$(call alias_image ubuntu)"
is  "arch has no other honest tag" "docker.io/library/archlinux:latest" "$(call alias_image arch)"
# python must NOT be docker.io/library/python: that image has no user and is
# not a distrobox-compatible base, so the capsule would not integrate.
hasnt "python is not the upstream python image" "library/python" "$(call alias_image python)"
has  "python brings the toolchain with it" "python3-pip" "$(call alias_packages python)"
is  "an unknown name is not an alias" false "$(predicate is_alias not-a-distro)"
is  "the empty string is not an alias" false "$(predicate is_alias '')"

echo "── cuda and rocm are device profiles, not just images ─────────────────"
is "cuda implies the nvidia profile" nvidia "$(call alias_gpu cuda)"
is "rocm implies the amd profile"    amd    "$(call alias_gpu rocm)"
is "a plain distro implies none"     none   "$(call alias_gpu fedora)"
is "nvidia is a valid profile" true  "$(predicate valid_gpu_profile nvidia)"
is "amd is a valid profile"    true  "$(predicate valid_gpu_profile amd)"
is "hw is a valid profile"     true  "$(predicate valid_gpu_profile hw)"
is "none is a valid profile"   true  "$(predicate valid_gpu_profile none)"
is "gpu=yes is not a profile"  false "$(predicate valid_gpu_profile yes)"

echo "── the flags each profile adds, exactly ───────────────────────────────"
# These cannot be checked on a machine without the hardware, and they are the
# whole content of the profile, so they are pinned here argument by argument.
nvidia_flags=$(call gpu_flags nvidia)
is "nvidia passes the host driver through" "--nvidia" "$nvidia_flags"

amd_flags=$(call gpu_flags amd)
has "amd forwards the compute device" "/dev/kfd" "$amd_flags"
has "amd forwards the render nodes"   "/dev/dri" "$amd_flags"
# Without keep-groups a rootless container drops the user's supplementary
# groups, so /dev/kfd is present and unopenable — which reads as a driver bug.
has "amd keeps the render group"      "keep-groups" "$amd_flags"
has "amd goes through additional-flags" "--additional-flags" "$amd_flags"

hw_flags=$(call gpu_flags hw)
has "hardware dev gets the USB bus" "/dev/bus/usb" "$hw_flags"
has "hardware dev keeps its groups" "keep-groups" "$hw_flags"

none_flags=$(call gpu_flags none)
is "the default profile adds nothing" "" "$none_flags"

# A profile that does not exist must not silently produce an empty flag list,
# because that would create the capsule WITHOUT the access the user asked for.
call gpu_flags bogus >/dev/null 2>&1
is "an unknown profile is an error" 1 "$?"

echo "── the container command, built once and asserted here ────────────────"
argv=$(call create_argv work docker.io/library/ubuntu:24.04 none '' '')
has "create is non-interactive"  "--yes" "$argv"
has "the name is passed through" "work"  "$argv"
has "the image is passed through" "docker.io/library/ubuntu:24.04" "$argv"
hasnt "no device flags without a profile" "--nvidia" "$argv"
hasnt "no home unless one was asked for"  "--home"   "$argv"
# One argument per line is the contract mapfile depends on: create, --yes,
# --name, <name>, --image, <image>.
is "every argument is its own line" 6 "$(wc -l <<<"$argv")"

argv=$(call create_argv cuda docker.io/library/ubuntu:24.04 nvidia '' '')
has "a cuda capsule carries --nvidia" "--nvidia" "$argv"

argv=$(call create_argv rocm docker.io/library/ubuntu:24.04 amd '' '')
has "a rocm capsule carries /dev/kfd" "/dev/kfd" "$argv"

argv=$(call create_argv py registry.fedoraproject.org/fedora-toolbox:43 none 'python3 python3-pip' /var/home/t/caps/py)
has "extra packages are requested" "--additional-packages" "$argv"
has "…as one argument"             "python3 python3-pip"   "$argv"
has "a custom home is passed"      "--home"                "$argv"
has "…with its path"               "/var/home/t/caps/py"   "$argv"

echo "── the package manager each capsule speaks ────────────────────────────"
is "fedora-toolbox uses dnf" dnf    "$(call pm_for_image registry.fedoraproject.org/fedora-toolbox:43)"
is "ubuntu uses apt"         apt    "$(call pm_for_image docker.io/library/ubuntu:24.04)"
is "debian uses apt"         apt    "$(call pm_for_image docker.io/library/debian:stable)"
is "arch uses pacman"        pacman "$(call pm_for_image docker.io/library/archlinux:latest)"
is "opensuse uses zypper"    zypper "$(call pm_for_image registry.opensuse.org/opensuse/tumbleweed)"
is "alpine uses apk"         apk    "$(call pm_for_image docker.io/library/alpine:3.20)"
call pm_for_image docker.io/library/scratch >/dev/null 2>&1
is "an unknown base is an error, not a guess" 1 "$?"

# Every one of these runs with no terminal attached, from `apex install`.
has "dnf is non-interactive"    "-y"              "$(call pm_install_argv dnf htop)"
has "apt is non-interactive"    "DEBIAN_FRONTEND" "$(call pm_install_argv apt htop)"
has "apt takes -y too"          "-y"              "$(call pm_install_argv apt htop)"
has "pacman is non-interactive" "--noconfirm"     "$(call pm_install_argv pacman htop)"
has "zypper is non-interactive" "--non-interactive" "$(call pm_install_argv zypper htop)"
has "several packages survive"  "curl"            "$(call pm_install_argv dnf htop curl)"

echo "── the rootless preflight refuses before anything is downloaded ───────"
# `id` is faked so the root refusal is reachable without being root. The
# refusal has to name the command that would work, not just say no.
cat > "$BIN/id" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
    -u) echo 0 ;;
    -un) echo root ;;
    *) exec /usr/bin/id "$@" ;;
esac
EOF
chmod +x "$BIN/id"
out=$(run_env create fedora); rc=$?
is  "root is refused" 1 "$rc"
has "…and told why"   "must not be created as root" "$out"
hasnt "…before distrobox is called" "distrobox" "$(cat "$CALLS")"
rm -f "$BIN/id"

# An account with no subordinate id range: podman would fail part-way through
# extracting the image, long after the download.
EXTRA_ENV=(APEX_ENV_SUBUID_FILE="$WORK/subuid-missing" APEX_ENV_SUBGID_FILE="$WORK/subgid-missing")
out=$(run_env create fedora); rc=$?
EXTRA_ENV=()
is  "no subuid range is refused" 1 "$rc"
has "…naming the account"        "tester" "$out"
has "…with the command that fixes it" "usermod --add-subuids" "$out"
hasnt "…before anything is pulled" "distrobox <create" "$(cat "$CALLS")"

is "a present range passes" true  "$(predicate has_subid_range tester)"
is "another account's range is not this one's" false "$(predicate has_subid_range nobody)"
# Belt and braces: a uid range without a matching gid range is not enough.
# podman needs both, and half a provisioning is the shape a hand-edited
# /etc/subuid actually takes.
printf 'tester:100000:65536\n' > "$WORK/subuid-half"
: > "$WORK/subgid-half"
EXTRA_ENV=(APEX_ENV_SUBUID_FILE="$WORK/subuid-half" APEX_ENV_SUBGID_FILE="$WORK/subgid-half")
is "a half-provisioned account is refused" false "$(predicate has_subid_range tester)"
EXTRA_ENV=()

echo "── create, end to end, against the stubs ──────────────────────────────"
out=$(run_env create cuda); rc=$?
calls=$(cat "$CALLS")
is  "create succeeds" 0 "$rc"
has "distrobox was driven" "distrobox <create>" "$calls"
has "…with the capsule name" "<--name> <cuda>" "$calls"
has "…and the nvidia profile" "<--nvidia>" "$calls"
has "…non-interactively" "<--yes>" "$calls"
has "the user is told how to enter it" "apex env enter cuda" "$out"

record="$WORK/records/cuda.json"
if [ -f "$record" ]; then ok "a record was written"
else bad "a record was written" "no $record"; fi
is "the record names the image"  "docker.io/library/ubuntu:24.04" "$(jq -r .image "$record")"
is "the record keeps the profile" "nvidia" "$(jq -r .gpu "$record")"
is "the record keeps the alias"   "cuda"   "$(jq -r .alias "$record")"
is "the record knows the package manager" "apt" "$(jq -r .package_manager "$record")"
# Provenance: which image this actually came from, recorded while podman can
# still answer. Six months later the tag has moved and the digest has not.
has "the record pins the digest" "sha256:" "$(jq -r .image_digest "$record")"

out=$(run_env create cuda); rc=$?
is  "a second create is refused" 1 "$rc"
has "…and says how to replace it" "apex env rm cuda" "$out"

# A container APEX did not create must not be adopted by using its name.
echo mybox > "$WORK/existing"
out=$(run_env create mybox); rc=$?
is  "a foreign container name is refused" 1 "$rc"
has "…and points at distrobox for it" "distrobox rm mybox" "$out"
: > "$WORK/existing"

out=$(run_env create 'ok --gpu'); rc=$?
is  "a name with a space is refused" 1 "$rc"
out=$(run_env create ok --gpu turbo); rc=$?
is  "an unknown device profile is refused" 1 "$rc"
has "…listing the ones that exist" "nvidia, amd, hw, none" "$out"

# An explicit --gpu overrides what the alias would have implied, in both
# directions: a cuda capsule with no device access is a legitimate request.
out=$(run_env create rocm --gpu none); rc=$?
is "an explicit profile overrides the alias" 0 "$rc"
is "…and is what gets recorded" "none" "$(jq -r .gpu "$WORK/records/rocm.json")"
hasnt "…so no device flags were passed" "/dev/kfd" "$(cat "$CALLS")"

echo "── list and info read back what create wrote ──────────────────────────"
out=$(run_env list)
has "list shows the capsule" "cuda" "$out"
has "list shows its profile" "nvidia" "$out"
out=$(run_env list --json)
if jq -e 'type == "array" and length == 2' <<<"$out" >/dev/null 2>&1; then
    ok "list --json is an array of every capsule"
else
    bad "list --json is an array of every capsule" "got: $(head -2 <<<"$out")"
fi
out=$(run_env info cuda)
is "info is the record itself" "nvidia" "$(jq -r .gpu <<<"$out")"
out=$(run_env info nosuch); rc=$?
is  "info on an unknown capsule fails" 1 "$rc"
has "…and suggests the listing" "apex env list" "$out"

echo "── enter and exec refuse an unknown capsule before running anything ───"
out=$(run_env enter nosuch); rc=$?
is  "enter refuses an unknown capsule" 1 "$rc"
hasnt "…without calling distrobox" "distrobox" "$(cat "$CALLS")"
out=$(run_env exec nosuch -- ls); rc=$?
is  "exec refuses an unknown capsule" 1 "$rc"
out=$(run_env enter ../../etc); rc=$?
is  "enter refuses a traversal" 1 "$rc"

run_env exec cuda -- echo hello >/dev/null
calls=$(cat "$CALLS")
has "exec enters the capsule"      "distrobox <enter>" "$calls"
has "…without a TTY"               "<--no-tty>" "$calls"
has "…and passes the command on"   "<echo> <hello>" "$calls"

run_env enter cuda >/dev/null
has "enter is interactive" "distrobox <enter> <cuda>" "$(cat "$CALLS")"

echo "── installing inside a capsule uses that capsule's package manager ────"
run_env install cuda htop >/dev/null
calls=$(cat "$CALLS")
has "an ubuntu capsule gets apt"  "<apt-get>" "$calls"
has "…non-interactively"          "DEBIAN_FRONTEND=noninteractive" "$calls"
hasnt "…and never dnf"            "<dnf>" "$calls"

run_env create py >/dev/null 2>&1
run_env install py htop >/dev/null
calls=$(cat "$CALLS")
has "a fedora capsule gets dnf" "<dnf>" "$calls"
hasnt "…and never apt"          "<apt-get>" "$calls"

out=$(run_env install nosuch htop); rc=$?
is "installing into an unknown capsule fails" 1 "$rc"
out=$(run_env install cuda); rc=$?
is "installing nothing is a usage error" 1 "$rc"

echo "── rm only removes what APEX created ──────────────────────────────────"
out=$(run_env rm mybox); rc=$?
is  "rm refuses a container it has no record of" 1 "$rc"
has "…and says --force exists"  "--force" "$out"
hasnt "…without calling distrobox" "distrobox" "$(cat "$CALLS")"

out=$(run_env rm cuda); rc=$?
is  "rm removes a known capsule" 0 "$rc"
has "…forcing a running container down" "distrobox <rm> <--force> <cuda>" "$(cat "$CALLS")"
if [ -f "$WORK/records/cuda.json" ]; then
    bad "…and drops the record" "record survived"
else ok "…and drops the record"; fi

out=$(run_env rm mybox --force); rc=$?
is "--force removes a foreign container" 0 "$rc"

echo "── the engine advertises what it does ─────────────────────────────────"
help=$(run_env --help)
for want in 'create <name>' 'enter <name>' 'exec <name>' 'rm <name>' \
            '--gpu nvidia|amd|hw|none' 'rootless' 'cuda, rocm'; do
    has "--help mentions: $want" "$want" "$help"
done
out=$(run_env frobnicate); rc=$?
is "an unknown verb is an error" 1 "$rc"

echo "── nothing escaped the sandbox ────────────────────────────────────────"
# The engine ran a dozen times with HOME pointed at a directory it should never
# have written to. Anything here means a path was derived from something other
# than APEX_ENV_HOME.
stray=$(find "$FAKEHOME" -mindepth 1 2>/dev/null | head -5)
if [ -z "$stray" ]; then ok "the fake HOME is still empty"
else bad "the fake HOME is still empty" "found: $(tr '\n' ' ' <<<"$stray")"; fi

echo
printf 'apex-env: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
