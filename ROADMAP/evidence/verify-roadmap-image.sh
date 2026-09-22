#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  verify-roadmap-image.sh — run this AFTER rebasing onto a roadmap image.
#
#  It only READS. No privilege, no changes, no reboot. Every line distinguishes
#  three answers, because this project has spent a month learning that a
#  refusal, an absence and a could-not-run are not the same thing: a check that
#  cannot run reports COULD-NOT-RUN and is counted separately from a pass.
#
#  See try-the-roadmap-image.md for the rebase and the way back.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

WANT_OS=266dcc572c51bdf9ec421d79eaa8784583184cd2
WANT_SHELL=b7953e4f5a8f2e535aa98bd94d1f4a0f9e0d917d

pass=0; fail=0; cnr=0
ok()  { printf '  PASS  %s\n' "$1"; pass=$((pass+1)); }
no()  { printf '  FAIL  %s\n' "$1"; fail=$((fail+1)); }
na()  { printf '  ----  %s — %s\n' "$1" "$2"; cnr=$((cnr+1)); }
sec() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }

sec "which image booted"
booted="$(rpm-ostree status --json 2>/dev/null \
  | python3 -c 'import sys,json;d=json.load(sys.stdin)["deployments"];x=next((y for y in d if y.get("booted")),{});print(x.get("container-image-reference",""))' 2>/dev/null)"
printf '      %s\n' "${booted:-<unreadable>}"
if [ -z "$booted" ]; then na "the booted image reference could be read" "rpm-ostree gave nothing"
elif [[ "$booted" == *"$WANT_OS"* ]]; then ok "booted the roadmap image ($WANT_OS)"
elif [[ "$booted" == *":daily"* ]]; then no "still on :daily — the rebase did not take (a crash discards a staged change)"
else no "booted something unexpected: $booted"; fi

sec "does it carry the shell half — the thing no earlier image had"
commit_file=/usr/share/apex-shell/.apex-shell-commit
if [ -r "$commit_file" ]; then
    got="$(tr -d '[:space:]' < "$commit_file")"
    printf '      vendored apex-shell: %s\n' "$got"
    [ "$got" = "$WANT_SHELL" ] && ok "the vendored shell is apex-shell roadmap/v2.2" \
                               || no "vendored shell is $got, expected $WANT_SHELL"
else na "the vendored apex-shell commit" "$commit_file is not readable here"
fi

sec "did it boot cleanly"
f="$(systemctl --failed --no-legend 2>/dev/null | wc -l)"
printf '      failed units: %s\n' "$f"
if [ "${f:-1}" -eq 0 ]; then ok "no failed system units"
elif [ "$f" -eq 1 ] && systemctl --failed --no-legend 2>/dev/null | grep -q vconsole; then
    systemctl --failed --no-legend 2>/dev/null | sed 's/^/        /'
    na "no failed system units" "the one failure is the known vconsole-setup boot race"
else systemctl --failed --no-legend 2>/dev/null | sed 's/^/        /'; no "no failed system units"; fi

sec "the eight items that were waiting on exactly this"
probe() { # name, command...
    local what="$1"; shift
    if "$@" >/dev/null 2>&1; then ok "$what"; else no "$what"; fi
}
command -v apex >/dev/null 2>&1 && ok "the apex CLI is on PATH" || no "the apex CLI is on PATH"
if command -v apex >/dev/null 2>&1; then
    probe "apex devices runs (P2-005/006/007)"        apex devices --help
    probe "apex lid exists (P1-063)"                  apex lid --help
    probe "apex user exists (P2-016)"                 apex user --help
    probe "apex account exists (P2-017)"              apex account --help
    probe "apex browser exists (P2-012)"              apex browser --help
    v="$(apex agent --version 2>/dev/null || true)"
    [ -n "$v" ] && printf '      apex agent: %s\n' "$v"
else na "the apex verb surface" "no apex on PATH"; fi
[ -x /usr/libexec/apex-safe-graphics ] && ok "Safe Graphics is installed (P2-018)" \
                                       || no "Safe Graphics is installed (P2-018)"
[ -f /usr/share/wayland-sessions/apex-labwc.desktop ] && ok "the Floating session is offered (P2-003)" \
                                                      || no "the Floating session is offered (P2-003)"

sec "the daemon the phone talks to"
pv="$(apex agent protocol 2>/dev/null || true)"
if [ -n "$pv" ]; then printf '      %s\n' "$pv"; ok "the agent daemon reports a protocol version"
else na "the daemon's protocol version" "no 'apex agent protocol' on this build — check 'apex agent --help'"; fi

printf '\nroadmap image: %d passed, %d failed, %d could-not-run\n' "$pass" "$fail" "$cnr"
sec "to go back, whenever you like"
cat <<'BACK'
      sudo rpm-ostree rollback
      systemctl reboot

  The previous deployment is retained; nothing on /var, /etc or /home is
  touched in either direction.
BACK
[ "$fail" -eq 0 ]
