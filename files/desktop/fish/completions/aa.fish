# Rime OS — completion for the `aa` shortcut (`rime agent attach`).
#
# Its argument is a session id, so this cannot reuse the `rime` completion:
# that one reads the verb from argument 1, and for `aa 4` there is no verb.
complete -c aa -f
complete -c aa -a '(_rime_session_ids)' -d session
