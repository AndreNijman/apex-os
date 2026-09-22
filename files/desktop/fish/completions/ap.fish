# APEX-OS — completion for the `ap` shortcut (`apex project`).
complete -c ap -f
complete -c ap -n 'not __fish_seen_subcommand_from list info worktrees checkpoints remove forget env layout switch' \
    -a 'list info worktrees checkpoints remove forget env layout switch' -d 'project verb'
complete -c ap -n '__fish_seen_subcommand_from layout; and not __fish_seen_subcommand_from open' \
    -a 'save show restore forget templates open' -d 'layout verb'
complete -c ap -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from open' \
    -a '(_apex_layout_templates)' -d template
