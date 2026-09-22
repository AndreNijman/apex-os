# APEX-OS — completion for the `aa` shortcut (`apex agent attach`).
#
# Its argument is a session id, so this cannot reuse the `apex` completion:
# that one reads the verb from argument 1, and for `aa 4` there is no verb.
complete -c aa -f
complete -c aa -a '(_apex_session_ids)' -d session
