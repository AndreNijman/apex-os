pragma Singleton
import QtQuick

// The words shown by AgentHelpPanel (roadmap §43).
//
// Prose lives here and nowhere else so one file can be read start to finish,
// and so tests/check-agent-help.sh can hold it to the stop-slop rules without
// scanning QML around it. Blocks:
//
//   h     a sub-heading
//   p     a paragraph
//   cmd   a command block, printed verbatim
//   kv    a term and its meaning
//   note  a paragraph with an accent rule down its left edge
//   todo  a feature the roadmap asks for that this build does not have
//
// Each command below exists in rimed/rime/src/agent.rs, request.rs or
// secret.rs. A flag that is not implemented does not belong in a help page.

QtObject {
    // ── Translatable ────────────────────────────────────────────────────────
    // qsTr() wraps these five strings. It wraps nothing else in this file yet.
    // That gap counts as a measurement rather than a half-finished job: roadmap
    // P2-004 asks for a baseline, and a baseline means a number you can defend.
    // tests/run-i18n-test.sh extracts these five with the real lupdate,
    // compiles them with the real lrelease, then reads them back out of a
    // running QML engine in German. That proves the whole pipeline on strings
    // this repository ships, and it reports the ratio of translatable to total
    // instead of guessing it.
    //
    // cardBody once held two adjacent literals joined with `+`. lupdate
    // extracts LITERALS, so a concatenation hands the translator two fragments
    // and no way to reorder them, which costs most of what translation does.
    // One string now.
    readonly property string entryLabel: qsTr("How Agents & Workspaces work")

    readonly property string cardTitle: qsTr("New to agents?")
    readonly property string cardBody: qsTr("Rime runs Claude, OpenCode, Codex or Gemini in a terminal it owns, so closing the window leaves the agent working. The guide starts from zero and covers starting an agent, picking which one runs, attaching and detaching, worktrees, diffs, checkpoints and the three sandbox modes.")
    readonly property string cardRead: qsTr("Read the guide")
    readonly property string cardDismiss: qsTr("Got it")

    readonly property var sections: [
    {
        id: "start",
        icon: "󰐊",
        title: "Start here",
        blocks: [
            { k: "h", t: "A Rime agent is an ordinary agent in a terminal Rime owns" },
            { k: "p", t: "An agent is a coding assistant you run in a terminal: Claude Code, OpenCode, Codex or Gemini. Rime does not replace it. Rime opens the terminal and then runs the same binary you would have typed yourself, so the agent sees a plain terminal and behaves the way it does outside Rime." },
            { k: "p", t: "Your shell does not own that terminal. The agent runtime does. Close the window and the agent keeps working, then pick it up again from any other terminal." },
            { k: "p", t: "Nothing here is compulsory. Run claude or codex by hand and you lose the runtime, not the tool." },

            { k: "h", t: "Turn the runtime on" },
            { k: "p", t: "The runtime is a per-user service and it stays off until you ask for it. One command opts in:" },
            { k: "cmd", t: "rime agent enable" },
            { k: "p", t: "If the Agents tab says the runtime is not running, that is the command it wants." },

            { k: "h", t: "Start an agent" },
            { k: "p", t: "Open a terminal, go to your project, type a. Put an opening instruction in quotes when you have one." },
            { k: "cmd", t: "a\na \"fix the failing tests\"\n\nrime agent run\nrime agent run \"fix the failing tests\"" },
            { k: "p", t: "The short a is a shell function, not a binary. Read it with type a and override it in ~/.zshrc.local." },

            { k: "h", t: "Pick which agent a runs" },
            { k: "p", t: "rime agent adapters prints the agents this runtime knows about and marks the ones you have installed. Choose the one a uses:" },
            { k: "cmd", t: "rime agent adapters\nrime agent default claude\nrime agent default          # what is set now" },
            { k: "p", t: "Override the choice for a single run with -a, and run any binary at all with the generic adapter:" },
            { k: "cmd", t: "a -a opencode \"review this diff\"\nrime agent run -a generic -- ./my-own-tool --flag" },
            { k: "p", t: "Rime launches an agent. It does not manage the agent's own configuration: your Claude settings, skills and plugins stay where Claude keeps them, and the runtime reads none of it." },
            { k: "todo", t: "A profile system that inspects, exports and syncs an agent's configuration is on the roadmap. This build does not have it." },

            { k: "h", t: "Talk to it" },
            { k: "p", t: "You type prompts in the terminal, as you did before Rime. The Agents tab has no prompt box on purpose, because a second and worse terminal inside a dashboard popup is not worth building." },
            { k: "p", t: "The one prompt Rime handles is the opening instruction you pass to a. After that the agent's own interface takes over." },

            { k: "h", t: "Speak to it" },
            { k: "p", t: "SUPER+ALT+V opens the microphone. Press it again to close it." },
            { k: "p", t: "The bar at the top of the screen shows what the route is doing and, before it records a word, which session it has picked. Speech goes to the agent session whose terminal has focus. With none focused the route refuses and names the half that was missing instead of choosing a session for you." },
            { k: "p", t: "Speech to text is a command you supply and not an engine Rime ships. Put one line in ~/.config/rime-shell/push-to-talk-stt: a command that reads audio on its standard input and writes text on its standard output. Until you do, the first press says so and stops there." },
            { k: "p", t: "The microphone belongs to the shell, not to a session. The shell holds it open while the indicator is up, and what crosses to a session is text. That is what keeps the no-microphone line above true of a sandboxed session." },
            { k: "todo", t: "This build reaches the transcript and stops. Writing text into a session's terminal needs a runtime verb this build does not have, so the last step of the route reports that instead of delivering the words." },
            { k: "todo", t: "Speech goes to a session that is already running. Starting one for the project in front of you is on the roadmap. This build does not do it." },

            { k: "h", t: "Attach and detach" },
            { k: "p", t: "Press ctrl-] to step out of a session. The agent carries on. Come back whenever you like:" },
            { k: "cmd", t: "aa            # the one running session\naa 4          # session 4\nrime agent attach 4\nrime agent attach 4 --no-replay" },
            { k: "p", t: "Attaching repaints the scrollback first, so you get the screen back as you left it, and then live output. Two terminals can watch one session at once. --no-replay skips the repaint." },
            { k: "p", t: "Change the detach key in ~/.config/rime/agent.json under detach_key." },

            { k: "h", t: "Pause and stop" },
            { k: "p", t: "A row carries three buttons on its right. The last one opens the terminal, which is also what clicking the row does. The first two, pause or resume and stop, run these:" },
            { k: "cmd", t: "rime agent pause 4\nrime agent resume 4\nrime agent kill 4\nrime agent kill 4 --signal kill" },
            { k: "p", t: "Pause suspends the agent and the whole tree of processes it started, so a compile in progress stops with it. Stop sends SIGTERM first, which gives the agent a chance to tidy up. Reach for --signal kill on one that ignores it." },

            { k: "h", t: "See what is running" },
            { k: "cmd", t: "al            # live sessions\nal --all      # finished ones too\nal --json\nrime agent status 4\nrime agent status          # the runtime itself" },
            { k: "p", t: "A session reads as working, waiting_for_user, permission_request, complete or failed. The Agents tab sorts on the same five words and lifts the two that want you to the top." }
        ]
    },
    {
        id: "projects",
        icon: "󰉋",
        title: "Projects and workspaces",
        blocks: [
            { k: "h", t: "A project is a git checkout the runtime has seen" },
            { k: "p", t: "There is nothing to create and nothing to register. Run an agent inside a repository once and that repository becomes a project." },
            { k: "cmd", t: "ap list       # projects, most recent first\nap info       # the one you are standing in\nap list --json\nrime project forget <slug>    # stop tracking it; your checkout stays put" },
            { k: "p", t: "ap is short for rime project." },

            { k: "h", t: "Workspaces: the windows a project lives in" },
            { k: "p", t: "Your editor, your terminals and your browser for one project sit on a compositor workspace. Rime can write that arrangement down and put it back:" },
            { k: "cmd", t: "rime project layout save\nrime project layout show\nrime project layout restore\nrime project layout restore --dry-run\nrime project layout forget" },
            { k: "p", t: "A saved layout records how to recreate each window: its command line, its working directory and the workspace it sat on. It does not record window handles, because no compositor honours one after a restart." },
            { k: "p", t: "Rime decides which windows belong to a project from the working directory behind each one, and ignores the title. A title is whatever an application chose to print." },
            { k: "p", t: "Restore is a command you run, not a login hook. A session that reopens fourteen windows you did not ask for is worse than one that reopens none." },
            { k: "cmd", t: "rime project switch npu-twin\nrime project switch            # the project you are standing in" },
            { k: "p", t: "Switch takes you to the workspace a project's windows are on. It needs a saved layout, because the layout is the thing that records the workspace." },

            { k: "h", t: "Worktrees: two agents on one repository" },
            { k: "p", t: "A git worktree is a second checkout of the same repository, in its own directory, on its own branch. Two agents in one checkout fight over the same files. Two agents in two worktrees do not." },
            { k: "cmd", t: "aw issue-217 \"fix issue 217\"\naw issue-221 \"fix issue 221\"\n\nrime agent run \"fix issue 217\" --worktree issue-217" },
            { k: "p", t: "Each run gets .rime/worktrees/issue-217 on branch agent/issue-217. Ask for the same name again and you land back in the same worktree with the same branch." },
            { k: "p", t: "Rime hides the worktree directory through .git/info/exclude rather than .gitignore. It is this machine's runtime state, so it stays out of git status and out of your colleagues' checkouts." },
            { k: "cmd", t: "ap worktrees\nrime project remove issue-217\nrime project remove issue-217 --keep-branch" },
            { k: "p", t: "Remove deletes the worktree and its branch. Pass --keep-branch to keep the branch and drop the directory." }
        ]
    },
    {
        id: "review",
        icon: "󰕌",
        title: "Review, checkpoints and undo",
        blocks: [
            { k: "h", t: "Read the diff" },
            { k: "cmd", t: "ad            # the patch\nad --stat     # file names, no patch\nad 4          # session 4\n\nrime agent diff\nrime agent diff 4 --stat" },
            { k: "p", t: "With no id, diff describes the most recent session in the project you are standing in. It compares two git trees against that session's checkpoint, so files the agent created show up and your own uncommitted work does not drown them." },
            { k: "p", t: "A session with no checkpoint of its own falls back to the project's newest one. With no checkpoint anywhere, diff prints plain git diff and tells you that is what happened." },

            { k: "h", t: "Tests" },
            { k: "p", t: "Rime does not run your tests, and the Agents tab does not report whether they pass. Run them yourself, in the worktree the agent worked in:" },
            { k: "cmd", t: "cd .rime/worktrees/issue-217\n# your own test command" },
            { k: "todo", t: "Per-worktree test status and merge-conflict state in the Agent Center are on the roadmap. This build shows which worktree a session is on and stops there." },

            { k: "h", t: "Take a checkpoint before the work, not after" },
            { k: "p", t: "Undo restores a checkpoint. With no checkpoint there is nothing to restore, so ask for one when you start:" },
            { k: "cmd", t: "a -c \"upgrade to Qt 7\"\nrime agent run \"upgrade to Qt 7\" --checkpoint\nrime agent checkpoint \"before the refactor\"\nap checkpoints" },
            { k: "p", t: "Set auto_checkpoint to true in ~/.config/rime/agent.json and you get one before each task without asking." },

            { k: "h", t: "Undo" },
            { k: "cmd", t: "rime agent undo\nrime agent undo 4\nrime agent undo --checkpoint <id>\nrime agent undo -y            # skip the confirmation" },
            { k: "p", t: "A checkpoint captures tracked and untracked files as a real git tree, plus HEAD, the branch and your installed package list. Undo puts the working tree back, deletes the files the agent created and unwinds the commits it made." },
            { k: "p", t: "Undo takes its own checkpoint first, so undoing the undo works." },
            { k: "p", t: "Capture runs through git plumbing against a temporary index, which leaves your staged changes, your stash and your branch alone. Checkpoints live under refs/rime/checkpoints/ rather than refs/heads/, so they do not appear as branches and a plain git push does not send them." },

            { k: "h", t: "Two things undo declines to do" },
            { k: "kv", t: "Ignored files stay out", d: "Your .gitignore names build output and local secrets. Sweeping a 4 GB target/ and your .env into a git object is not an undo feature." },
            { k: "kv", t: "Packages get reported, not removed", d: "Undo lists the packages that arrived since the checkpoint and prints the rime remove line. Running a system-wide removal because you rewound a working tree is your call to make, not the runtime's." }
        ]
    },
    {
        id: "sandbox",
        icon: "󰌾",
        title: "Sandbox and permissions",
        blocks: [
            { k: "h", t: "Six controls, kept apart" },
            { k: "p", t: "Rime separates six permission layers. Moving one leaves the other five where they were." },
            { k: "kv", t: "1. The agent's own permission mode", d: "Claude's bypassPermissions lives here. It governs the questions Claude asks you." },
            { k: "kv", t: "2. The Rime filesystem and process sandbox", d: "Which files and which processes a session can see at all. The --sandbox flag." },
            { k: "kv", t: "3. The Rime system and root capability layer", d: "Whether a session can change the machine. Today that means rime request." },
            { k: "kv", t: "4. The Rime secret and cloud capability layer", d: "Whether a session can use a stored credential, and for which operation." },
            { k: "kv", t: "5. The Rime network policy", d: "Whether the session has a network at all." },
            { k: "kv", t: "6. The remote-origin policy", d: "Where a request came from: your keyboard, the shell, a remote client, a scheduled job." },

            { k: "h", t: "Three rules that hold in this build" },
            { k: "note", t: "Turning off the agent's own confirmations leaves the Rime sandbox switched on. Claude in bypassPermissions still runs inside bubblewrap with your home directory masked." },
            { k: "note", t: "Giving a session your full user access does not give it root. Unrestricted means it can do what you can do without sudo." },
            { k: "note", t: "Granting root does not hand over your stored tokens. The secret broker performs the operation and returns the output; the credential stays in a process the session cannot see." },

            { k: "h", t: "The three sandbox modes" },
            { k: "p", t: "Pass --sandbox, or -s. The default is project." },
            { k: "kv", t: "project", d: "Your project files stay writable. The rest of your home directory is not hidden, it is absent: Rime mounts an empty filesystem over $HOME and binds back the project and the build caches. ~/.ssh, ~/.gnupg, ~/.aws and your browser profiles are not there to read. /usr and /etc drop to read-only. The session gets its own process namespace and sees four processes instead of four hundred. No camera and no microphone. /run gets masked, so the session cannot reach the system bus and cannot change the machine's power tier, fan curve, charge thresholds or game mode. Rime clears the environment and rebuilds it, so an ANTHROPIC_API_KEY sitting in your shell does not follow the agent in. The network stays up." },
            { k: "kv", t: "strict", d: "The project rules, and the network taken away." },
            { k: "kv", t: "unrestricted", d: "No sandbox. Rime runs the agent binary as you, with your files. It can read ~/.ssh and the tokens you handed the secret broker. This is the escape hatch, and picking it means you know the confinement is off." },
            { k: "cmd", t: "a -s strict \"audit this dependency\"\na -s unrestricted\nrime agent run --sandbox project" },
            { k: "p", t: "Set the default for new sessions in ~/.config/rime/agent.json under sandbox. rime agent status prints the one in force." },

            { k: "h", t: "What unrestricted does not buy you" },
            { k: "p", t: "An unrestricted session runs as your account and stops there. It gets no root. Dropping the sandbox and gaining root are separate layers, and -s unrestricted moves one of them." },
            { k: "p", t: "The caveat is not sudo inside the session. Managed sessions run with PR_SET_NO_NEW_PRIVS, so sudo and su start there and come up unprivileged whatever the sandbox. An unconfined session can write the files your own shell runs later: a shell rc file, a git hook, a systemd user unit. Those run as you, in a process that inherits none of a session's limits." },

            { k: "h", t: "Always Unrestricted" },
            { k: "p", t: "Config → Agents carries one toggle that makes unrestricted the default for new sessions. Switching it on asks for your password at the desktop's authentication prompt, which is outside any agent's terminal. Switching it off asks for nothing and takes effect at once." },
            { k: "p", t: "It grants no root, switches off no secret broker and opens no break-glass mode. It moves the sandbox layer and leaves the other five where they were, so a Claude profile set to bypassPermissions keeps it in both directions." },
            { k: "p", t: "A session already running keeps the mode it started with. Each row in the Agents tab carries its own mode, and a warning banner sits above the list for as long as unrestricted is the default." },
            { k: "kv", t: "What the toggle writes", d: "sandbox in ~/.config/rime/agent.json, the same key you can set by hand", md: true },

            { k: "h", t: "System access" },
            { k: "p", t: "A separate grant that lets one session perform root operations. You authenticate outside the agent's terminal, so the agent cannot see or drive the prompt. The grant is opaque to the agent, bound to that session, scoped to named capabilities, limited in time, non-transferable, and the agent cannot renew it." },
            { k: "todo", t: "This build has no --system-access flag. An agent that needs root asks with rime request, below." },

            { k: "h", t: "Break-glass full access" },
            { k: "p", t: "The owner's break-glass switch: rime agent run --unsafe-everything --ttl 15m. It wants local authentication at a real prompt, a short expiry you name yourself, a red indicator in the Agents tab while it is live, a permanent audit record and automatic expiry. There is no remember-forever, and it does not survive a reboot." },
            { k: "p", t: "Even then the broker does not tip your stored credentials into the agent's environment." },
            { k: "todo", t: "This build has no --unsafe-everything flag and no --ttl flag." },

            { k: "h", t: "How an agent gets root today: it asks, and it waits" },
            { k: "p", t: "A session has no sudo, no root shell and a sandbox that cannot reach the system bus. A session that needs a system change files a request and then blocks on your answer:" },
            { k: "cmd", t: "rime request ask install clang --reason \"Required to compile the project\"" },
            { k: "p", t: "The request appears at the top of the Agents tab under Waiting for your decision, with a Review button that opens a terminal on it. You decide there:" },
            { k: "cmd", t: "rime request pending\nsudo rime request approve 3\nsudo rime request approve 3 --for-project\nrime request deny 3\nrime request audit" },
            { k: "p", t: "Approving with --for-project stops Rime asking about that same operation again in that project." },
            { k: "p", t: "Rime fixes the list of things an agent may ask for: install, remove, pkg-upgrade, pkg-rebuild, pkg-rollback, pin, rollback, update. Read it back with rime request verbs." },
            { k: "note", t: "There is no verb for running a command, and there will not be one. You cannot review an arbitrary shell line, and approving sh -c '...' once amounts to permanent root." },
            { k: "p", t: "The privilege exercised is yours, borrowed for one operation. rime request approve runs under the same root gate as rime install, which is why an approved request does not run while you are away from the machine." },
            { k: "p", t: "A session cannot approve its own request. The daemon works out who is asking from the connection's peer credentials and the process tree, and pays no attention to what the caller claims." },

            { k: "h", t: "Credentials the agent uses but does not hold" },
            { k: "cmd", t: "printf %s \"$TOKEN\" | rime secret add github --host github.com\nrime secret grant github git-push\nrime secret grants\nrime secret audit" },
            { k: "p", t: "The agent asks for git-push origin. rime-agentd, which runs outside the sandbox, performs the push and hands back git's output. The token exists in a process the session cannot see, because the sandbox gives it its own process namespace." },
            { k: "p", t: "The agent names a remote, and the daemon resolves that name against the repository's own configuration. Accepting a URL would let a session ask the broker to push your branch to someone else's server with your token attached." },
            { k: "p", t: "Two operations exist today: git-push and git-fetch." },
            { k: "p", t: "A project or strict session cannot read the stored token, because the file sits in your home directory and the sandbox masks it. An unrestricted session can. That is the escape hatch doing what it says." }
        ]
    },
    {
        id: "remote",
        icon: "󰢹",
        title: "Other computers and phones",
        blocks: [
            { k: "h", t: "Agents on your other machines" },
            { k: "p", t: "Register a machine with rime host add and the Agents tab grows a Remote devices section showing what runs over there. Rime asks only while that tab is in front of you; close it and the ssh connections stop." },
            { k: "cmd", t: "rime host add desk --ssh andre@192.168.1.20
rime host list
rime host probe desk" },
            { k: "cmd", t: "rime agent list --host desk\nrime agent run \"fix the build\" --host desk\nrime agent attach 4 --host desk" },
            { k: "p", t: "--host on run forwards the whole invocation to that machine's own rime agent run, so the remote applies its sandbox policy, its default agent and its checkpointing. Reconstructing those decisions here would be a second copy of one policy, and the local copy would be the wrong one." },
            { k: "p", t: "Uncommitted changes here do not travel. The remote works from its own checkout. Two flags cover the awkward cases:" },
            { k: "cmd", t: "rime agent run \"...\" --host desk --allow-dirty\nrime agent run \"...\" --host desk --remote-path /srv/build/proj" },
            { k: "p", t: "The id you pass to attach --host belongs to the remote, which is why the list form exists." },
            { k: "p", t: "The Remote devices section reads; it does not act. A session id means something only on the machine that issued it, so a Stop button there would stop an unrelated agent somewhere else. To take over a remote session, copy the line the section prints:" },
            { k: "cmd", t: "rime host run -t desk -- rime agent attach 4" },

            { k: "h", t: "Claude's own Remote Control" },
            { k: "p", t: "Remote Control belongs to Claude Code, and Rime leaves it alone. An agent you started with a and then drove from Remote Control is still a Rime session, so the sandbox, the request queue and the secret broker all apply to it. One you started by typing claude in a plain terminal is outside all of that, the same as any other unmanaged process." },
            { k: "todo", t: "Rime does not yet record where a request came from, so a privilege request from an agent driven over Remote Control looks the same as one you triggered at the keyboard. Origin-aware policy is layer 6 above and it is on the roadmap." },

            { k: "h", t: "The Android app" },
            { k: "p", t: "Rime Remote for Android will pair a phone with this machine by QR code, over your LAN or an encrypted relay, and give you the Agent Center on the phone." },
            { k: "todo", t: "Rime ships no phone client today, and no pairing surface to point one at." }
        ]
    },
    {
        id: "keys",
        icon: "󰌌",
        title: "Keys and commands",
        blocks: [
            { k: "h", t: "Keyboard" },
            { k: "kv", t: "ctrl-]", d: "Step out of the attached session and leave it working. Rebind it in ~/.config/rime/agent.json under detach_key.", mt: true },
            { k: "kv", t: "Escape", d: "Close this guide. Press it again to close the dashboard.", mt: true },
            { k: "kv", t: "SUPER+D", d: "Open the dashboard, then click Agents. Config then Keybinds lists what the shell binds and lets you change it.", mt: true },
            { k: "kv", t: "SUPER+ALT+V", d: "Open the microphone for push-to-talk, and press it again to close it. Config then Keybinds is where to change the combination.", mt: true },
            { k: "p", t: "Rime binds no key straight to the Agents tab. Add one in your compositor configuration pointing at the IPC line below." },

            { k: "h", t: "What each control on this page runs" },
            { k: "kv", t: "Open the Agents tab", d: "qs -p /usr/share/rime-shell ipc call dashboard-agents toggle", md: true },
            { k: "kv", t: "Open this guide", d: "qs -p /usr/share/rime-shell ipc call agent-help toggle", md: true },
            { k: "kv", t: "Jump to one of its sections", d: "qs -p /usr/share/rime-shell ipc call agent-help open sandbox", md: true },
            { k: "kv", t: "Bring back the first-run card", d: "qs -p /usr/share/rime-shell ipc call agent-help reset", md: true },
            { k: "kv", t: "Click a session row", d: "rime agent attach <id>", md: true },
            { k: "kv", t: "Pause button on a row", d: "rime agent pause <id>, then rime agent resume <id>", md: true },
            { k: "kv", t: "Stop button on a row", d: "rime agent kill <id>", md: true },
            { k: "kv", t: "Review on a pending request", d: "rime request show <id>, then sudo rime request approve <id>", md: true },
            { k: "kv", t: "Refresh on Remote devices", d: "rime agent list --host <device>", md: true },

            { k: "h", t: "Shell shortcuts" },
            { k: "cmd", t: "a      rime agent run\naa     rime agent attach\nal     rime agent list\nad     rime agent diff\naw     rime agent run --worktree\nap     rime project" },
            { k: "p", t: "Set RIME_NO_AGENT_ALIASES=1 in ~/.zshrc.local or ~/.bashrc to drop the shortcuts and keep tab completion. Bash and zsh have completion for session ids, agent names and sandbox modes." },

            { k: "h", t: "Flags on a run" },
            { k: "cmd", t: "-a  --agent <name>        which agent\n-s  --sandbox <mode>      strict | project | unrestricted\n-w  --worktree <name>     run in a dedicated git worktree\n-c  --checkpoint          capture a checkpoint first\n-d  --detach              start it and return\n    --cwd <path>          where to run\n    --host <device>       run it on a trusted device instead\n    --                    the rest of the line goes to the agent binary" },

            { k: "h", t: "The full vocabulary" },
            { k: "cmd", t: "rime agent   run list attach pause resume kill logs status\n             default adapters diff undo checkpoint event rm prune enable\nrime project list info worktrees checkpoints remove forget switch layout\nrime project layout   save show restore forget\nrime request ask list pending show approve deny verbs grants revoke audit\nrime secret  add list remove capabilities grant revoke grants use audit" },
            { k: "p", t: "rime agent event is the hook interface. A process inside a session already knows its id from $RIME_AGENT_SESSION, so an agent hook can publish a state change with one word:" },
            { k: "cmd", t: "rime agent event working\nrime agent event permission_request --detail \"wants to run make install\"" }
        ]
    },
    {
        id: "stuck",
        icon: "󰑓",
        title: "When a session is stuck",
        blocks: [
            { k: "h", t: "Find out what it is doing" },
            { k: "cmd", t: "rime agent status 4      # one session in detail\nrime agent status        # the runtime, and the defaults in force\nrime agent logs 4        # the transcript\nrime agent logs 4 --bytes 200000\nal --all" },

            { k: "h", t: "Then act on what you found" },
            { k: "kv", t: "waiting_for_user", d: "It asked you something. Attach with aa 4 and answer it." },
            { k: "kv", t: "permission_request", d: "The agent asked for a system change and stopped there. Run rime request pending, then approve or deny." },
            { k: "kv", t: "working, and going nowhere", d: "rime agent pause 4 freezes the whole process tree so you can look at it. rime agent resume 4 lets it go again." },
            { k: "kv", t: "Beyond help", d: "rime agent kill 4 sends SIGTERM. rime agent kill 4 --signal kill for the ones that ignore that." },
            { k: "kv", t: "Finished, and cluttering the list", d: "rime agent rm 4 forgets one session and deletes its transcript. rime agent prune forgets all the finished ones." },

            { k: "h", t: "The runtime itself" },
            { k: "p", t: "Sessions outlive the shell, your terminal and your compositor. They do not outlive the daemon: stopping rime-agentd stops the agents it started." },
            { k: "cmd", t: "rime agent enable\nsystemctl --user status rime-agentd\nsystemctl --user disable --now rime-agentd" },

            { k: "h", t: "Where things live" },
            { k: "kv", t: "~/.config/rime/agent.json", d: "default_agent, sandbox, detach_key, auto_checkpoint", mt: true },
            { k: "kv", t: "~/.local/state/rime/agent/sessions/", d: "one JSON record per session, which is what the Agents tab reads", mt: true },
            { k: "kv", t: "~/.local/state/rime/agent/logs/", d: "transcripts, mode 0600, capped at 32 MiB", mt: true },
            { k: "kv", t: "~/.local/state/rime/agent/checkpoints/", d: "checkpoint metadata; the trees themselves sit in refs/rime/checkpoints/", mt: true },
            { k: "kv", t: "$XDG_RUNTIME_DIR/rime-agentd/control.sock", d: "the control socket, mode 0600", mt: true },
        ]
    }
    ]
}
