#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-rime-schema.sh — executable assertions for roadmap §25's persistent
#  state migration.
#
#  ── The failure this is guarding against ────────────────────────────────────
#  `bootc rollback` puts /usr back. It does not put /etc, /var or the user's
#  home back, and it is not supposed to. So the sequence that breaks a machine
#  is update, migrate, hit an unrelated bug, roll back — and the older build is
#  now reading files whose shape it has never seen. Every assertion below is
#  one direction of that: forward with a copy kept, backward refusing out loud,
#  and a record from the future left exactly where it is.
#
#  Two modes, the split test-boot-v2.sh uses:
#
#    (no argument)     Structural checks with no toolchain: that every store in
#                      the registry declares all five of §25's fields, that the
#                      unmanaged list still names the security-relevant stores,
#                      and that no store's version key doc says "absent means
#                      the current version" — the rule that silently misreads
#                      every file written before the key existed.
#
#    --with-binary     Drives `rime schema` and `rime blueprint` against real
#                      state trees. It DIES if the binary is absent; a skipped
#                      assertion reports as a pass.
#
#  Every run writes into its own mktemp tree and exports XDG_STATE_HOME and
#  XDG_CONFIG_HOME at it. That is not politeness: this suite exercises the
#  code that REWRITES persistent state, and a run that inherited the real
#  variables would migrate the developer's own task records.
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

MIGRATE="$REPO/rimed/rimed-core/src/migrate.rs"
SCHEMARS="$REPO/rimed/rime/src/schema.rs"
BLUEPRINT="$REPO/rimed/rimed-core/src/blueprint.rs"
TASKRS="$REPO/rimed/rimed-core/src/task.rs"
for f in "$MIGRATE" "$SCHEMARS" "$BLUEPRINT" "$TASKRS"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done

TMP="$(mktemp -d)"
trap 'chmod -R u+rwX "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

# ═════════════════════════════════════════════════════════════════════════════
sec "every store declares all five things §25 asks for"
# schema before (unversioned), schema after (current), forward migration
# (steps), rollback strategy (rollback), checkpoint (the framework's, shared).
# Counted inside the STORES const alone: migrate.rs's test module defines
# stores of its own, and counting those would let a real store drop a field
# without the total moving.
registry="$(sed -n '/^pub const STORES/,/^];/p' "$MIGRATE")"
declared="$(printf '%s\n' "$registry" | grep -c '^    Store {')"
if [[ "$declared" -ge 3 ]]; then
    ok "the registry declares $declared stores"
else
    bad "the registry declares only $declared stores"
fi
for field in 'unversioned:' 'current:' 'rollback:' 'steps:' 'version_key:' 'authored:' 'sample:'; do
    n="$(printf '%s\n' "$registry" | grep -c "^        $field")"
    if [[ "$n" -eq "$declared" ]]; then
        ok "$field is declared by all $declared stores"
    else
        bad "$field appears on $n of $declared stores"
    fi
done

sec "no version key means 'absent is whatever this build is'"
# The rule that is invisible until the second version exists, at which point
# every file written before the key claims to be current. The blueprint's doc
# said exactly this before §25.
# The pattern is anchored on the doc line that DECLARES the rule, not on prose
# that quotes it — the fix for this defect has to be allowed to say what it
# fixed, or the checker trains people to delete the explanation.
if grep -qE '^\s*/// File-format version\. Absent means \[`SCHEMA_VERSION`\]' \
        "$BLUEPRINT" "$TASKRS"; then
    bad "a version key still declares 'absent means SCHEMA_VERSION'"
    grep -nE '^\s*/// File-format version\. Absent means' "$BLUEPRINT" "$TASKRS" >&2
else
    ok "no version key declares itself as defaulting to the current version"
fi
if grep -q 'unversioned: 0' "$MIGRATE"; then
    ok "at least one store declares version 0 — the shape before its key existed"
else
    bad "no store admits to having records with no version key"
fi

sec "the gap is in the product, not hidden"
for want in agent-grants secret-store privilege-audit agent-sessions; do
    if grep -q "\"$want\"" "$SCHEMARS"; then
        ok "$want is named as unmanaged"
    else
        bad "$want is neither managed nor listed — the report would look complete"
    fi
done

sec "a person's file is never rewritten by the framework"
if grep -q 'Authored::Human' "$MIGRATE" && \
   grep -q 'written by a person' "$MIGRATE"; then
    ok "migrate_file refuses a human-authored store"
else
    bad "nothing stops migrate_file reserialising a file somebody typed"
fi

# ═════════════════════════════════════════════════════════════════════════════
if [[ "$WITH_BINARY" -eq 0 ]]; then
    printf '\n%s\n' "── binary checks skipped (pass --with-binary) ──"
    printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
    [[ "$FAIL" -eq 0 ]] || exit 1
    exit 0
fi

Rime="${Rime:-$REPO/rimed/target/debug/rime}"
[[ -x "$Rime" ]] || Rime="${CARGO_TARGET_DIR:-}/debug/rime"
if [[ ! -x "$Rime" ]]; then
    echo "FATAL: no rime binary. Build it, or set Rime=/path/to/rime." >&2
    echo "       A skipped assertion reports as a pass, which is the bug this refuses." >&2
    exit 1
fi

# Every path this suite touches lives under $TMP. Nothing below may resolve
# outside it — this is the suite that writes to persistent state, so the
# tripwire is not decoration.
export XDG_STATE_HOME="$TMP/state"
export XDG_CONFIG_HOME="$TMP/config"
export HOME="$TMP/home"
mkdir -p "$XDG_STATE_HOME/rime/tasks" "$XDG_CONFIG_HOME/rime" "$HOME"
resolved="$("$Rime" schema status --json | python3 -c 'import json,sys
d=json.load(sys.stdin)
print("\n".join(s["path"] for s in d["stores"]))')"
while read -r p; do
    [[ -z "$p" ]] && continue
    case "$p" in
        "$TMP"/*) ;;
        *) echo "FATAL: rime schema resolved $p, outside $TMP" >&2; exit 2 ;;
    esac
done <<< "$resolved"
ok "every store path this suite drives resolves inside the temp tree"

sec "a record written before the version key existed is migrated, and kept"
printf '{"created":100,"last_opened":200}\n' > "$XDG_STATE_HOME/rime/tasks/old.json"
"$Rime" schema status > "$TMP/out" 2>&1
has 'schema 0, 1 step(s) behind schema 1' "$TMP/out" "an unversioned record reads as schema 0, not as current"

"$Rime" schema migrate > "$TMP/out" 2>&1
has 'Dry run' "$TMP/out" "migrate without --commit says it is a dry run"
has 'would migrate' "$TMP/out" "the dry run names what it would do"
if [[ -f "$XDG_STATE_HOME/rime/tasks/old.json.pre-v0" ]]; then
    bad "the dry run wrote a checkpoint"
else
    ok "the dry run wrote nothing"
fi
before="$(cat "$XDG_STATE_HOME/rime/tasks/old.json")"

"$Rime" schema migrate --commit > "$TMP/out" 2>&1
has 'migrated' "$TMP/out" "--commit migrates"
if [[ -f "$XDG_STATE_HOME/rime/tasks/old.json.pre-v0" ]]; then
    ok "a copy of the file as it was is kept at .pre-v0"
    if [[ "$(cat "$XDG_STATE_HOME/rime/tasks/old.json.pre-v0")" == "$before" ]]; then
        ok "the checkpoint is the pre-migration bytes"
    else
        bad "the checkpoint is not what the file was"
    fi
else
    bad "no checkpoint was written"
fi
python3 - "$XDG_STATE_HOME/rime/tasks/old.json" <<'PY' && ok "the migrated record carries schema 1 and every value it had" \
    || bad "the migration lost a value or the stamp"
import json,sys
d=json.load(open(sys.argv[1]))
sys.exit(0 if d.get("schema")==1 and d["created"]==100 and d["last_opened"]==200 else 1)
PY

sec "the checkpoint of a rollback-then-forward cycle is still the original"
# The older build rewrites the record at schema 0 after a rollback. Migrating
# again must not replace the copy of what the file was BEFORE any of this.
printf '{"created":100,"last_opened":999}\n' > "$XDG_STATE_HOME/rime/tasks/old.json"
"$Rime" schema migrate --commit > /dev/null 2>&1
if grep -q '"last_opened":200' "$XDG_STATE_HOME/rime/tasks/old.json.pre-v0"; then
    ok "the original checkpoint survived a second migration"
else
    bad "the second migration overwrote the original checkpoint"
fi

sec "a record from a newer Rime is left exactly where it is"
printf '{"schema":9,"created":1,"last_opened":2,"invented_later":"x"}\n' \
    > "$XDG_STATE_HOME/rime/tasks/future.json"
snap="$(cat "$XDG_STATE_HOME/rime/tasks/future.json")"
"$Rime" schema status > "$TMP/out" 2>&1
has 'NEWER than the schema 1 this build reads' "$TMP/out" "a newer record is reported as newer"
hasnt 'behind' "$TMP/out" "it is not described as behind"
"$Rime" schema migrate --commit > "$TMP/out" 2>&1
has 'left alone' "$TMP/out" "--commit refuses to touch it"
if [[ "$(cat "$XDG_STATE_HOME/rime/tasks/future.json")" == "$snap" ]]; then
    ok "the file is byte-identical after --commit"
else
    bad "a record from a newer Rime was rewritten — the only copy of what it knew"
fi
if [[ -f "$XDG_STATE_HOME/rime/tasks/future.json.pre-v9" ]]; then
    bad "a checkpoint was written for a file that was not migrated"
else
    ok "no checkpoint was written for a file nothing changed"
fi

sec "a blueprint from a newer Rime refuses by name, not by serde"
# The defect: Blueprint is deny_unknown_fields, so a newer file failed on
# whichever key it hit first and the user was told "unknown field `x` at line
# 2" about a file that is not wrong.
printf 'version = 7\ninvented_later = true\n[desktop]\ncompositor = "hyprland"\n' \
    > "$XDG_CONFIG_HOME/rime/blueprint.toml"
"$Rime" blueprint show > "$TMP/out" 2>&1
has 'schema 7' "$TMP/out" "the refusal names the version the file is on"
has 'reads schema 1' "$TMP/out" "the refusal names the version this build reads"
has 'rollback' "$TMP/out" "the refusal names the likely cause"
has 'Boot the newer deployment again' "$TMP/out" "the refusal names the remedy"
hasnt 'unknown field' "$TMP/out" "it is not a serde error about a key"

sec "a blueprint on the current version still loads"
printf 'version = 1\n[desktop]\ncompositor = "hyprland"\n' \
    > "$XDG_CONFIG_HOME/rime/blueprint.toml"
"$Rime" blueprint show > "$TMP/out" 2>&1
has 'hyprland' "$TMP/out" "a current blueprint is read"
hasnt 'schema' "$TMP/out" "and says nothing about schemas"
# The file is a person's. Nothing may have rewritten it.
if [[ "$(head -1 "$XDG_CONFIG_HOME/rime/blueprint.toml")" == "version = 1" ]]; then
    ok "reading a blueprint does not rewrite it"
else
    bad "the blueprint was reserialised"
fi
"$Rime" schema migrate --commit > "$TMP/out" 2>&1
has 'never rewritten' "$TMP/out" "migrate says why it will not touch a hand-written file"

sec "a store nobody can read is not a store that is fine"
printf '{"created":1}\n' > "$XDG_STATE_HOME/rime/tasks/sealed.json"
chmod 0000 "$XDG_STATE_HOME/rime/tasks/sealed.json"
if [[ -r "$XDG_STATE_HOME/rime/tasks/sealed.json" ]]; then
    printf '  skip  this user reads a 0000 file (root or CAP_DAC_OVERRIDE)\n'
else
    "$Rime" schema status > "$TMP/out" 2>&1
    has 'unavailable' "$TMP/out" "a refused read reports as unavailable"
    has 'Permission denied' "$TMP/out" "with the reason"
fi
chmod 0600 "$XDG_STATE_HOME/rime/tasks/sealed.json"

sec "the report names the stores this framework does not cover"
"$Rime" schema status > "$TMP/out" 2>&1
has 'Not managed by this framework yet' "$TMP/out" "the gap has a heading"
has 'grants.json' "$TMP/out" "the agent grant table is named"
has 'rime-secretd' "$TMP/out" "the secret store is named"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
