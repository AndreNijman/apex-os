# APEX-OS — completion for the `ap` shortcut (`apex project`).
complete -c ap -f
complete -c ap -n 'not __fish_seen_subcommand_from list info worktrees checkpoints remove forget env layout switch' \
    -a 'list info worktrees checkpoints remove forget env layout switch' -d 'project verb'
complete -c ap -n '__fish_seen_subcommand_from layout' \
    -a 'save show restore forget' -d 'layout verb'
