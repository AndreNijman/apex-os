#!/usr/bin/env bash
# The rule `InsecureStorageTest` depends on, enforced.
#
# That test walks a directory and accounts for every byte in it. It is evidence
# about app storage only while `AppStorage` really is the only thing that writes
# there — a `SharedPreferences` file, a `DataStore`, a scrollback cache or a
# debug log would each be a second place for a key to land and a place the walk
# never looks.
#
# So: no Android storage API appears in `app/src/main` except inside the one
# adapter that hands `AppStorage` a directory. A rule with no check is a
# comment, and this file is the check.
#
# Run it from `android/`, or from anywhere — it finds its own tree.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
app="$here/../app/src/main"

# The adapter, and the only file allowed to name a storage location.
adapter="kotlin/com/apexos/remote/data/MachineRepository.kt"

# Each entry is a pattern and what is wrong with it. `filesDir` is here too:
# it is not dangerous, it is simply the thing exactly one file may say.
patterns=(
  'SharedPreferences|a preferences file is a second write path the storage walk never sees'
  'getSharedPreferences|a preferences file is a second write path the storage walk never sees'
  'PreferenceManager|a preferences file is a second write path the storage walk never sees'
  'androidx\.datastore|DataStore is a second write path the storage walk never sees'
  'openFileOutput|open the store through AppStorage instead'
  'getExternalFilesDir|external storage is world-readable by design'
  'getExternalStorageDirectory|external storage is world-readable by design'
  'MODE_WORLD_|world-readable or world-writable modes, on an app that holds a device key'
  'filesDir|only the repository adapter may name a storage location'
  'cacheDir|a cache is still app storage, and the walk does not look in it'
)

status=0
for entry in "${patterns[@]}"; do
  pattern="${entry%%|*}"
  why="${entry#*|}"
  # `--include` rather than a find pipeline so a file with a space in its name
  # cannot silently drop out of the search.
  hits="$(grep -rnE --include='*.kt' --include='*.java' --include='*.xml' \
    -- "$pattern" "$app" 2>/dev/null | grep -v "/$adapter:" || true)"
  if [ -n "$hits" ]; then
    echo "a second write path into app storage: $why"
    echo "$hits" | sed 's/^/    /'
    status=1
  fi
done

# And the adapter has to still exist, or the exemption above is quietly
# excusing nothing and the whole check has stopped looking at anything.
if [ ! -f "$app/$adapter" ]; then
  echo "$adapter is gone; this check's one exemption no longer names a file"
  status=1
fi

# A search that matches nothing proves nothing. The adapter must itself contain
# the thing everyone else is forbidden, or the patterns have rotted.
if ! grep -q 'filesDir' "$app/$adapter"; then
  echo "$adapter no longer says filesDir, so this check is searching for a string nothing uses"
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "app storage has one writer: AppStorage, through $adapter"
fi
exit "$status"
