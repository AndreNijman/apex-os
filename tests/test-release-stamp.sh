#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# test-release-stamp.sh — the release ID a build publishes as, and the
# /usr/share/rime/release.json it bakes (files/scripts/next-release-id and
# files/scripts/stamp-release).
#
# Rime Shell opens https://rimeos.com/updates/<id> once after an update, so the
# ID must be the one the site publishes the release under (YYYY.MM.DD by AWST
# promotion date, .2 .3 … the same day), must NOT move for a reissue of the same
# source (or every weekly rebuild would open a page), and the file must carry
# what the Shell and the site agree on.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
NEXT=files/scripts/next-release-id
STAMP=files/scripts/stamp-release

pass=0; fail=0
is() {   # is <name> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok   $1"; pass=$((pass + 1))
    else echo "  FAIL $1 — got '$2', want '$3'"; fail=$((fail + 1)); fi
}
A=0123456789abcdef0123456789abcdef01234567
B=89abcdef0123456789abcdef0123456789abcdef
S=fedcba9876543210fedcba9876543210fedcba98
T=00000000000000000000000000000000000000aa

echo "── next-release-id ──"
is "nothing published with an ID yet: today's date" \
   "$(bash $NEXT 2026.09.29 $A $S "" "" "")" 2026.09.29
is "an earlier day's release: today's date" \
   "$(bash $NEXT 2026.09.29 $A $S 2026.09.28.4 $B $S)" 2026.09.29
is "a second promotion the same day is .2, not .1" \
   "$(bash $NEXT 2026.09.29 $A $S 2026.09.29 $B $S)" 2026.09.29.2
is "…and a third .3" \
   "$(bash $NEXT 2026.09.29 $A $S 2026.09.29.2 $B $S)" 2026.09.29.3
is "double digits count on" \
   "$(bash $NEXT 2026.09.29 $A $S 2026.09.29.10 $B $S)" 2026.09.29.11
is "the same OS and Shell revisions are a reissue: the ID stays" \
   "$(bash $NEXT 2026.10.05 $A $S 2026.09.29.2 $A $S)" 2026.09.29.2
is "the same OS revision with a new Shell is a new release" \
   "$(bash $NEXT 2026.09.29 $A $T 2026.09.29 $A $S)" 2026.09.29.2
is "an unparseable published ID (a pre-release label): today's date" \
   "$(bash $NEXT 2026.09.29 $A $S rime $A $S)" 2026.09.29
is "a pre-rename ID never counts as today's" \
   "$(bash $NEXT 2026.09.29 $A $S apex-v2.1.0 $B $S)" 2026.09.29
bash $NEXT 29-09-2026 $A $S "" "" "" >/dev/null 2>&1; is "a malformed date is refused" "$?" 2

echo "── stamp-release ──"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT INT TERM
mkdir -p "$W/usr/share/rime-shell"; printf '%s\n' "$S" > "$W/usr/share/rime-shell/.rime-shell-commit"
bash $STAMP 2026.09.29.2 $A "$W"; rc=$?
is "a release stamps" "$rc" 0
f="$W/usr/share/rime/release.json"
j() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], "<absent>"))' "$f" "$1"; }
is "the file is valid JSON with schema 1" "$(j schema)" 1
is "…its id" "$(j id)" 2026.09.29.2
is "…version is the id" "$(j version)" 2026.09.29.2
is "…the OS revision passed in" "$(j revision)" $A
is "…the Shell revision read from the vendored tree" "$(j shellRevision)" $S
is "…the canonical notes URL" "$(j notes)" https://rimeos.com/updates/2026.09.29.2
is "…no imageDigest (an image cannot hold its own)" "$(j imageDigest)" "<absent>"
is "…world-readable" "$(stat -c %a "$f")" 644
bash $STAMP "" $A "$W"
is "no ID is a dev build" "$(j id)" dev
is "…with no notes URL, so the Shell opens nothing" "$(j notes)" ""
bash $STAMP 2026.09.29.1 $A "$W" >/dev/null 2>&1; is "a .1 suffix is refused (the second is .2)" "$?" 1
bash $STAMP 'x"y' $A "$W" >/dev/null 2>&1; is "an ID that would need escaping is refused" "$?" 1
bash $STAMP 2026.09.29 not-a-sha "$W" >/dev/null 2>&1; is "a bad revision is refused" "$?" 1
rm "$W/usr/share/rime-shell/.rime-shell-commit"
bash $STAMP 2026.09.29 $A "$W" >/dev/null 2>&1; is "an image with no vendored Shell commit is refused" "$?" 1

echo
echo "test-release-stamp: passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
