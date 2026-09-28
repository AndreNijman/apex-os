# Rime OS — completion for the `ad` shortcut (`rime agent diff`).
complete -c ad -f
complete -c ad -a '(_rime_session_ids)' -d session
