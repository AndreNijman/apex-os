#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-channel.sh — executable assertions for roadmap §26's update
#  channels and the health signal that stops a rollout.
#
#  Two modes, the split test-boot-v2.sh uses:
#
#    (no argument)     Structural checks with no toolchain: that `edge` is
#                      promoted with the tags it is a name for, that the three
#                      slower channels are NOT promoted on every build, that the
#                      read-back gate covers edge, and that the promotion
#                      dispatch verifies a signature and refuses to skip a
#                      channel. These live in `static` because they compare
#                      .github/workflows against apexd/, two different path
#                      selectors — a PR touching only the workflow sets
#                      rust=false, and the drift would ship.
#
#    --with-binary     Drives `apex channel` against fixture roots.
#
#  ── The claim these are guarding ────────────────────────────────────────────
#  `apex channel list` tells a user that stable carries "only builds that have
#  run on the other channels first". That sentence is true only if two things
#  hold in CI: the slower channels do not move on every build, and a promotion
#  refuses a digest that has not been on the channel above. Neither is visible
#  from the CLI, and both are one careless line away from being false while
#  every test still passes.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WITH_BINARY=0
[[ "${1:-}" == "--with-binary" ]] && WITH_BINARY=1

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
has() {
    if grep -qF -- "$1" "$2"; then ok "$3"; else
        bad "$3 — no '$1' in:"; sed 's/^/       /' "$2" >&2
    fi
}
hasnt() {
    if grep -qF -- "$1" "$2"; then
        bad "$3 — found '$1' in:"; sed 's/^/       /' "$2" >&2
    else ok "$3"; fi
}

BUILD="$REPO/.github/workflows/build-image.yml"
PROMOTE="$REPO/.github/workflows/promote-channel.yml"
CHANNELRS="$REPO/apexd/apexd-core/src/channel.rs"
CLIRS="$REPO/apexd/apex/src/channel.rs"
OPSRS="$REPO/apexd/apex/src/ops.rs"
for f in "$BUILD" "$PROMOTE" "$CHANNELRS" "$CLIRS" "$OPSRS"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done

TMP="$(mktemp -d)"
trap 'chmod -R u+rwX "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

# ═════════════════════════════════════════════════════════════════════════════
sec "edge moves with the tags it is a name for"
promote_block="$(sed -n '/- name: Promote to every published tag/,/- name: Assert every published tag/p' "$BUILD")"
printf '%s\n' "$promote_block" > "$TMP/promote"
has 'promote edge' "$TMP/promote" "every build of main moves :edge"
# And the slow channels must NOT be here. A channel that advanced on every
# build is edge with a different spelling, and a user who chose it would be
# taking the same risk while believing they had opted out of it.
for c in stable candidate beta; do
    if grep -qE "promote \"?$c\"?\$" "$TMP/promote"; then
        bad ":$c is promoted on every build — it would be edge under another name"
    else
        ok ":$c is not promoted on every build"
    fi
done

sec "the read-back gate covers edge"
readback="$(sed -n '/- name: Assert every published tag resolves/,/exit 1/p' "$BUILD")"
printf '%s\n' "$readback" > "$TMP/readback"
has 'edge' "$TMP/readback" "the read-back loop includes edge"
# The four migration tags must still be there. They are what every machine in
# the field tracks, and a tag that stops moving does not error.
for t in apex daily gaming-mesa gaming-nvidia; do
    has "$t" "$TMP/readback" "the read-back loop still includes $t"
done

sec "a promotion cannot point a channel at an unsigned or skipped build"
has 'cosign verify' "$PROMOTE" "the promotion verifies a signature"
has 'build-image.yml@refs/heads/main' "$PROMOTE" "against this repository's build workflow on main"
has 'already be on the channel above' "$PROMOTE" "a promotion refuses a build that skipped a channel"
for pair in 'beta)      above=edge' 'candidate) above=beta' 'stable)    above=candidate'; do
    has "$pair" "$PROMOTE" "the ladder step '$pair' is declared"
done
# workflow_dispatch only: a promotion that fired on a push would move stable on
# every merge, which is the failure this whole file exists to prevent.
if grep -qE '^  (push|pull_request|schedule):' "$PROMOTE"; then
    bad "promote-channel.yml fires on something other than a dispatch"
else
    ok "promote-channel.yml is workflow_dispatch only"
fi
has 'group: apex-image-publish' "$PROMOTE" "it shares build-image.yml's concurrency group"

sec "the rollout stop is wired into the update path, not just available"
# A gate nobody's update consults is a report. `ops::update` is the only place
# that can refuse, so that is where the call has to be.
if grep -q 'channel::halt_reason' "$OPSRS"; then
    ok "ops::update consults the rollout stop"
else
    bad "nothing in ops.rs calls halt_reason — the stop cannot stop anything"
fi
if grep -q 'channel::record_update' "$OPSRS"; then
    ok "ops::update records what the machine was running"
else
    bad "nothing records the pre-update digest, so the gate can never arm"
fi
# The record is written BEFORE the pull. Written after a successful upgrade it
# would be missing for exactly the update that crashed the machine.
rec_line="$(grep -n 'channel::record_update' "$OPSRS" | head -1 | cut -d: -f1)"
pull_line="$(grep -n '"bootc", &\["upgrade"\]' "$OPSRS" | head -1 | cut -d: -f1)"
if [[ -n "$rec_line" && -n "$pull_line" && "$rec_line" -lt "$pull_line" ]]; then
    ok "the record is written before the pull (line $rec_line before $pull_line)"
else
    bad "the record is written after the pull ($rec_line vs $pull_line) — it would be missing for the update that broke the machine"
fi

sec "the health verdict cannot be built from a file that never exists"
# apex-boot-health is conditioned on systemd-boot's LoaderBootCountPath and
# every published image boots GRUB, so /var/lib/apex/boot/last-health.json has
# never been written on any APEX machine. A verdict that read it would be
# permanently empty and permanently green.
if grep -q 'last-health.json' "$CHANNELRS" "$CLIRS"; then
    bad "the health verdict reads last-health.json, which no published image writes"
else
    ok "the verdict does not depend on the boot-health file"
fi
if grep -q 'systemctl' "$CLIRS"; then
    ok "it uses failed units, which every boot path has"
else
    bad "nothing collects failed units"
fi

# ═════════════════════════════════════════════════════════════════════════════
if [[ "$WITH_BINARY" -eq 0 ]]; then
    printf '\n%s\n' "── binary checks skipped (pass --with-binary) ──"
    printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
    [[ "$FAIL" -eq 0 ]] || exit 1
    exit 0
fi

APEX="${APEX:-$REPO/apexd/target/debug/apex}"
[[ -x "$APEX" ]] || APEX="${CARGO_TARGET_DIR:-}/debug/apex"
if [[ ! -x "$APEX" ]]; then
    echo "FATAL: no apex binary. Build it, or set APEX=/path/to/apex." >&2
    echo "       A skipped assertion reports as a pass, which is the bug this refuses." >&2
    exit 1
fi

export XDG_CONFIG_HOME="$TMP/config"
export XDG_STATE_HOME="$TMP/state"
mkdir -p "$XDG_CONFIG_HOME/apex" "$XDG_STATE_HOME"

# A fixture machine: the ostree= symlink chain `apex trust` and `apex channel`
# both read, with a chosen tag in the deployment origin.
fixture() {
    local root="$TMP/$1" tag="$2"
    local csum=f3f505fc39fb268c59f4458365c96b764a7bd7d30f2f51e98bb6a009666b7852
    local bootcsum=1d98b51dd76621b656c50e4f22dc7e5eade9b0f869443a3efa90eee08eb9373e
    rm -rf "$root"
    mkdir -p "$root/proc" "$root/etc/containers" \
             "$root/ostree/boot.0/default/$bootcsum" \
             "$root/ostree/deploy/default/deploy/$csum.0"
    printf 'root=UUID=x rw ostree=/ostree/boot.0/default/%s/0\n' "$bootcsum" > "$root/proc/cmdline"
    ln -sfn "../../../deploy/default/deploy/$csum.0" "$root/ostree/boot.0/default/$bootcsum/0"
    printf '[origin]\ncontainer-image-reference=ostree-unverified-registry:%s\n' "$tag" \
        > "$root/ostree/deploy/default/deploy/$csum.0.origin"
    printf '{"default":[{"type":"insecureAcceptAnything"}]}\n' > "$root/etc/containers/policy.json"
    printf '%s' "$root"
}

run() { APEX_TRUST_ROOT="$1" "$APEX" "${@:2}" > "$TMP/out" 2> "$TMP/err"; echo $?; }

sec "a machine installed before channels existed is told where it stands"
# The state of every APEX machine that exists. `:daily` moves on every build of
# main, so it IS edge, and "unknown channel" would be a worse answer than none.
R="$(fixture legacy 'ghcr.io/andrenijman/apex-os:daily')"
run "$R" channel status >/dev/null
has 'following    : daily' "$TMP/out" "the readout names the tag the machine follows"
has 'edge' "$TMP/out" "and says that tag is the edge channel"
has 'moves with every build' "$TMP/out" "and says why"
hasnt 'unknown' "$TMP/out" "it does not answer 'unknown'"

sec "a machine on a channel tag reads as that channel"
for c in stable candidate beta edge; do
    R="$(fixture "on-$c" "ghcr.io/andrenijman/apex-os:$c")"
    run "$R" channel status >/dev/null
    has "following    : $c" "$TMP/out" "a machine on :$c reads as $c"
done

sec "moving toward stable warns about the state that does not roll back"
R="$(fixture back 'ghcr.io/andrenijman/apex-os:edge')"
# --dry-run still needs root, and asserting the refusal is how we know the
# privileged classification did not drift to depend on the flag.
run "$R" channel set stable --dry-run >/dev/null
has 'must run as root' "$TMP/err" "channel set refuses without root, dry run included"

sec "the four channels are listed with what each one costs"
R="$(fixture list 'ghcr.io/andrenijman/apex-os:daily')"
run "$R" channel list >/dev/null
for c in stable candidate beta edge; do
    has "$c" "$TMP/out" "$c is listed"
done
has 'every successful build of main' "$TMP/out" "edge says what it costs"
has '* edge' "$TMP/out" "the machine's own channel is marked, through its alias"

sec "the report carries no identifier, and says nothing was sent"
R="$(fixture report 'ghcr.io/andrenijman/apex-os:daily')"
run "$R" channel report --json >/dev/null
python3 - "$TMP/out" <<'PY' && ok "the payload has exactly channel, tag, digest, healthy, reasons" \
    || bad "the payload's shape changed"
import json,sys
d=json.load(open(sys.argv[1]))
sys.exit(0 if sorted(d["wouldSend"]) == ["channel","digest","healthy","reasons","tag"] else 1)
PY
python3 - "$TMP/out" <<'PY' && ok "nothing was sent, and the JSON says so" || bad "the JSON does not record that nothing was sent"
import json,sys
d=json.load(open(sys.argv[1]))
sys.exit(0 if d["sent"] is False and d["optedIn"] is False and d["endpoint"] is None else 1)
PY
run "$R" channel report >/dev/null
has 'Nothing was sent' "$TMP/out" "the report says nothing was sent"
has 'reporting is off, which is the default' "$TMP/out" "and that off is the default"
hasnt 'machine-id' "$TMP/out" "the machine id is not in the payload"

sec "opting in without an endpoint still sends nothing, and says why"
printf 'report = true\n' > "$XDG_CONFIG_HOME/apex/channel.toml"
run "$R" channel report >/dev/null
has 'no endpoint is configured' "$TMP/out" "it names the missing endpoint"
has 'operates' "$TMP/out" "and says APEX runs no service to send it to"
rm -f "$XDG_CONFIG_HOME/apex/channel.toml"

sec "a channel nobody has heard of is refused by name"
R="$(fixture bogus 'ghcr.io/andrenijman/apex-os:daily')"
rc="$(run "$R" channel set nightly)"
[[ "$rc" != 0 ]] && ok "an invented channel exits non-zero" || bad "'nightly' was accepted"
for c in stable candidate beta edge; do
    has "$c" "$TMP/err" "the refusal names $c"
done

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
