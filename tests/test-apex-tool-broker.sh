#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-tool-broker.sh — assertions against the SHIPPED shim,
#  files/system/libexec/apex-tool-broker. Nothing here re-implements it: every
#  case runs the script as a process, under the name it would be invoked by.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  P1-012's first acceptance criterion is "existing skills can continue
#  invoking normal tools". That is a claim about a command line, and the only
#  way to check a claim about a command line is to run one.
#
#  The shim has exactly two jobs and they pull in opposite directions:
#
#    * a subcommand apex brokers must reach `apex secret use` with the right
#      operation, and must NOT reach the real tool — which would run it with no
#      credentials and produce a confusing failure;
#    * everything else must reach the real tool UNCHANGED, argv and all,
#      because `wrangler --version` breaking is how a shim gets uninstalled.
#
#  ── What is faked, and what is not ──────────────────────────────────────────
#  There is no wrangler, no terraform and no apex-secretd on the machine this
#  was written on, and none is needed. $PATH is reduced to a directory holding
#  recording stubs for `apex` and for the real tools, and the suite REFUSES TO
#  RUN if `command -v apex` does not resolve to the stub — a test that measured
#  a real `apex` would be measuring the developer's machine.
#
#  Nothing is written outside a temp directory, no network is used, and no
#  credential exists anywhere in this file: the shim never sees one. That is
#  the point of it.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
shim="$here/../files/system/libexec/apex-tool-broker"
[ -x "$shim" ] || { echo "FAIL  $shim is not executable"; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
bin="$tmp/bin"        # the stubs, and the shim under the tools' names
real="$tmp/real"      # the "real" wrangler and terraform
mkdir -p "$bin" "$real"

cat > "$bin/apex" <<'STUB'
#!/bin/sh
echo "apex $*" >> "$APEX_TEST_LOG"
echo "brokered"
STUB
for tool in wrangler terraform; do
    cat > "$real/$tool" <<STUB
#!/bin/sh
echo "real-$tool \$*" >> "\$APEX_TEST_LOG"
echo "ran the real $tool"
STUB
    chmod +x "$real/$tool"
    ln -s "$shim" "$bin/$tool"
done
chmod +x "$bin/apex"

export APEX_TEST_LOG="$tmp/log"
export PATH="$bin:$real:/usr/bin:/bin"

# The refusal that makes every assertion below mean something.
if [ "$(command -v apex)" != "$bin/apex" ]; then
    echo "FAIL  apex does not resolve to the stub; this suite would measure the host"
    exit 1
fi
if [ "$(command -v wrangler)" != "$bin/wrangler" ]; then
    echo "FAIL  wrangler does not resolve to the shim"
    exit 1
fi

pass=0; fail=0
check() { # check <name> <expected> <actual>
    if [ "$2" = "$3" ]; then
        pass=$((pass + 1)); printf 'ok    %s\n' "$1"
    else
        fail=$((fail + 1))
        printf 'FAIL  %s\n      expected: %s\n      actual:   %s\n' "$1" "$2" "$3"
    fi
}
run() { : > "$APEX_TEST_LOG"; "$@" > "$tmp/out" 2> "$tmp/err" || true; }
logged() { cat "$APEX_TEST_LOG" 2>/dev/null | tr '\n' '|' | sed 's/|$//'; }

# ── a brokered subcommand reaches the daemon and not the tool ────────────────
export APEX_CF_WORKER=project
run wrangler deploy
check "wrangler deploy asks apex to broker it" \
    "apex secret use cloudflare cloudflare.wrangler.deploy project" "$(logged)"

run wrangler versions upload
check "wrangler versions upload asks apex to broker it" \
    "apex secret use cloudflare cloudflare.wrangler.versions-upload project" "$(logged)"

run terraform plan
check "terraform plan asks apex to broker it" \
    "apex secret use cloudflare cloudflare.terraform.plan " "$(logged)"

run terraform apply
check "terraform apply asks apex to broker it" \
    "apex secret use cloudflare cloudflare.terraform.apply " "$(logged)"

# ── everything else reaches the real tool, unchanged ────────────────────────
run wrangler --version
check "wrangler --version runs the real tool" "real-wrangler --version" "$(logged)"

run wrangler types --path worker-configuration.d.ts
check "an unbrokered subcommand keeps its whole argv" \
    "real-wrangler types --path worker-configuration.d.ts" "$(logged)"

run terraform fmt -check
check "terraform fmt runs the real tool" "real-terraform fmt -check" "$(logged)"

# ── a brokered subcommand with flags the fixed argv cannot carry ────────────
run wrangler deploy --dry-run
check "a --dry-run is refused, never silently deployed" "" "$(logged)"
if grep -q "fixed command line" "$tmp/err"; then
    pass=$((pass + 1)); printf 'ok    %s\n' "the refusal says why"
else
    fail=$((fail + 1)); printf 'FAIL  the refusal does not say why: %s\n' "$(cat "$tmp/err")"
fi

# ── --env is understood, not refused ────────────────────────────────────────
# The most common real invocation after the bare subcommand. Refusing it would
# mean "existing skills can continue invoking normal tools" was true only for
# skills that never pick an environment.
export APEX_CF_WORKER=project
run wrangler deploy --env staging
check "APEX_CF_WORKER wins over --env when both are given" \
    "apex secret use cloudflare cloudflare.wrangler.deploy project" "$(logged)"

unset APEX_CF_WORKER
run wrangler deploy --env staging
check "--env names the worker the daemon resolves" \
    "apex secret use cloudflare cloudflare.wrangler.deploy staging" "$(logged)"

run wrangler deploy --env=canary
check "--env=name is the same flag" \
    "apex secret use cloudflare cloudflare.wrangler.deploy canary" "$(logged)"

run wrangler versions upload --env staging
check "--env works on versions upload too" \
    "apex secret use cloudflare cloudflare.wrangler.versions-upload staging" "$(logged)"

# ── the worker comes from wrangler.toml, the way wrangler finds it ──────────
# A skill that has always run a bare `wrangler deploy` sets no variable. If the
# shim needed one, transparency would be a claim rather than a fact.
work="$tmp/project"; mkdir -p "$work"
printf 'name = "from-toml"\nmain = "src/index.ts"\n' > "$work/wrangler.toml"
(cd "$work" && run wrangler deploy)
check "a bare deploy reads the worker out of wrangler.toml" \
    "apex secret use cloudflare cloudflare.wrangler.deploy from-toml" "$(logged)"

printf '{\n  // the worker\n  "name": "from-json",\n  "main": "src/index.ts"\n}\n' > "$work/wrangler.jsonc"
rm "$work/wrangler.toml"
(cd "$work" && run wrangler deploy)
check "and out of wrangler.jsonc when that is what the project has" \
    "apex secret use cloudflare cloudflare.wrangler.deploy from-json" "$(logged)"

# ── a worker nobody named ───────────────────────────────────────────────────
unset APEX_CF_WORKER
cd "$tmp"
run wrangler deploy
check "a deploy with no worker named reaches neither apex nor wrangler" "" "$(logged)"
if grep -q "APEX_CF_WORKER" "$tmp/err"; then
    pass=$((pass + 1)); printf 'ok    %s\n' "it says which worker it needs"
else
    fail=$((fail + 1)); printf 'FAIL  it does not say what is missing: %s\n' "$(cat "$tmp/err")"
fi

# ── the shim never carries a credential ─────────────────────────────────────
# Not an assertion about the shim's care: there is nothing in it that could.
if grep -qiE 'CLOUDFLARE_API_TOKEN|Bearer |api[_-]?token' "$shim"; then
    fail=$((fail + 1)); printf 'FAIL  the shim mentions a credential\n'
else
    pass=$((pass + 1)); printf 'ok    %s\n' "the shim never names a credential"
fi

# ── nothing was written outside the temp directory ──────────────────────────
printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
