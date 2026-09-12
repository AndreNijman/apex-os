# APEX-OS — completion for the `ad` shortcut (`apex agent diff`).
complete -c ad -f
complete -c ad -a '(_apex_session_ids)' -d session
