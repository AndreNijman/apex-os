# Rime OS — completion for the `a` shortcut (`rime agent run`).
complete -c a -f
complete -c a -s a -l agent -x -a '(_rime_agent_names)' -d 'which agent'
complete -c a -s n -l name -x -d 'what to call the session'
complete -c a -s s -l sandbox -x -a 'strict project unrestricted' -d confinement
complete -c a -s w -l worktree -x -d 'run in a worktree'
complete -c a -s c -l checkpoint -x -d 'start from a checkpoint'
complete -c a -s d -l detach -d 'start it and do not attach'
