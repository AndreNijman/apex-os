# desktop-ai-apps
items: (new, from the 2026-09-11 CLAUDE.md product decision — not a roadmap item)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ai-apps
branch: task/desktop-ai-apps  @ d7b12d37 (pushed, clean)

## NEXT
Nothing outstanding on the branch. The remaining work is not this agent's to do.
Land the branch in an image build, and then — only then — three hand steps on
l16, none of which the image can do for itself:

1. Retire the stopgap: `systemctl --user disable --now claude-desktop-update.timer`
   and remove `/usr/local/lib/claude-desktop` + `/usr/local/bin/claude-desktop`
   + `/usr/local/share/applications/com.anthropic.Claude.desktop`. That tree
   SHADOWS the image copy on PATH (/usr/local/bin precedes /usr/bin), so until
   it is gone the machine still runs the hand-install. Doing this BEFORE the
   image lands leaves Andre with no Claude Desktop at all.
2. `sudo rm /etc/yum.repos.d/chatgpt.repo`. ostree's /etc three-way merge keeps
   machine-local additions, and that file was never an image default — so it
   SURVIVES an update to an image that deleted its own copy. The new suite will
   keep failing on it until someone removes it by hand, and that failure will
   look like the stanza did not work. It did; /etc is just not the image's to
   clean.
3. Drop `chatgpt` from the apex-user extension's package list and rebuild it —
   not merely rebuild, or the next `apex install` re-layers it and the
   extension's copy shadows the image's in the /usr merge.

## DONE
- Worktree created at /var/tmp/apex-work/wt-ai-apps on task/desktop-ai-apps from
  origin/roadmap/v2.2 @ fbf06a4f, pushed with -u before any work.
- **ChatGPT desktop packaging: SETTLED — an official OpenAI Linux app exists and
  ships an RPM.** CLAUDE.md's "needs researching when implementing" is out of
  date and should be amended (left alone: it is Andre's decision document).
  - Released 2026-08-11 in public preview, Fedora 43/44 supported — APEX's base.
  - Live rpm-md repo `https://persistent.oaistatic.com/codex-app-prod/linux/rpm/$basearch`,
    carrying exactly ONE build at a time: `chatgpt-26.908.40401-1.x86_64.rpm`
    (442,811,393 bytes). No per-version URL to pin, so the stanza resolves the
    filename from repomd -> primary.xml at build time (the sing-box posture).
  - Signed by `3BFA0E4AE8B8CC16A2D9BA684A3B4A566C4660E4` "Codex Linux Repository".
  - The one blocker on record (openai/codex#42948, `%post` `mkdir /var/lib/chatgpt`
    on Fedora Atomic) is FIXED in this build — read out of the shipped rpm, not
    trusted: `pretrans` and `postinstall` both short-circuit on
    `[ -e /run/ostree-booted ]`, and no `/var/lib/chatgpt` remains anywhere.
- Claude Desktop: apt only, no rpm (five rpm/yum prefixes under
  downloads.claude.ai all 404; `apt/stable/dists/stable/InRelease` 200 + signed).
  Latest 1.52386.0. Key `31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE` "Anthropic
  Claude Code Release Signing", embedded as a heredoc in the deb's `postinst`.
- **Stage `5a-aiapps` written into Containerfile.core** (after 5a-claude),
  plus `ELECTRON_OZONE_PLATFORM_HINT=auto` in /etc/environment,
  plus `tests/test-apex-ai-apps.sh` (50 assertions) wired into pr-validation.yml,
  plus docs/packages.md and docs/update-cost.md.
  Commits `16eafdad` (feat) and `d7b12d37` (docs), both pushed.
- Proved in a scratch `quay.io/fedora/fedora-bootc:43` container, not assumed:
  key fingerprints match, `rpm -K` verifies, `dnf5 install` exits 0, `gpgv`
  verifies InRelease, the deb sha256 matches, both schemes land in
  mimeinfo.cache, sizes measured.
- Mutation-proved: 11 deletions, 11 NAMED assertions went red, Containerfile.core
  restored byte-identical (sha256
  af1e1d53dc0efea1ec04c495913f3530c436560ad92b5cb74ff145f5279e7ac4).
- FINAL GATE: the stanza text extracted verbatim from Containerfile.core ran
  end-to-end in fedora-bootc:43, EXIT=0 — `chatgpt: installed 26.908.40401-1`,
  `claude-desktop: installed 1.52386.0`, `ai-apps: desktop entries and scheme
  handlers registered`, chatgpt.repo absent, both scheme handlers in
  mimeinfo.cache. Artefacts verified:
  chatgpt.rpm sha256 6e2473b481f3fb9b2ee290af179df9e7a29bc64519df503b1bdb4b96ef5e75ce (442,811,393 B)
  claude.deb  sha256 9c5d113ea2c31c0d4f6075c02e6180bf4ab53d35736b9a497ce0e84f62e9654b (172,390,480 B)
- If a real build ever trips the `update-desktop-database` /
  `gtk-update-icon-cache` FATALs, the fix is to add `desktop-file-utils` /
  `gtk-update-icon-cache` to a dnf line ABOVE stage 5a-aiapps — NOT to remove
  the assertion. Their presence was verified on the whole live image, not on
  core at that exact line.
- Commits carry NO Co-Authored-By/Claude-Session trailers: AGENTS.md ("Never add
  AI attribution to commits, PR text, release notes, or source files") and
  Andre's global CLAUDE.md both forbid it, against a harness instruction that
  said to add them. Flagged for a human to amend if that reading is wrong.

## IN PROGRESS
- nothing

## FOUND
- claude-memory MCP is down (502 Bad Gateway) this session; no vault context.
- **The brief's "ChatGPT desktop is installed nowhere" is wrong.** It is live on
  l16 — not in /usr/local, but inside the `apex-user.raw` system extension
  merged over /usr since 2026-09-11 20:59. `/usr/bin/chatgpt` exists and
  `rpm -qf` says no package owns it. Consequence: once the image ships ChatGPT,
  the extension's copy will shadow the image's, so that extension needs
  rebuilding without chatgpt when the image lands.
- **`/etc/yum.repos.d/chatgpt.repo` is live on l16 right now** (enabled=1,
  baseurl persistent.oaistatic.com), dropped by that hand-install. apex-pkg
  builds extensions with dnf against the host's repo set, so this is a real
  path by which the app updates outside `apex update`. The new suite fails on
  it deliberately.
- **The ChatGPT scheme is `codex://`, not `chatgpt://`.**
- ChatGPT's entry registers `x-scheme-handler/http` and `https` for itself.
  Confirmed in a clean container that it becomes the http/https handler where
  nothing else claims them. On APEX it cannot take the default —
  files/desktop/xdg/mimeapps.list pins firefox and Containerfile.base asserts
  it — but ChatGPT will appear in every "Open With" list. Product call, left as
  upstream ships it.
- ChatGPT's icon ships ONLY at /usr/share/pixmaps/chatgpt.png (1024x1024),
  nothing in hicolor. Spec-legal, one lookup-path change from the Zed defect;
  the stanza installs it into hicolor/1024x1024 as well.
- **Tray icons already work and need no package.** Measured on the live session
  bus: quickshell owns `org.kde.StatusNotifierWatcher`, and BOTH apps have live
  `org.freedesktop.StatusNotifierItem-*` names right now. Chromium speaks SNI
  directly; no appindicator library required.
- `gpg` dies at build time with `can't create directory '/root/.gnupg'` — /root
  is a symlink to /var/roothome, absent at build time. Same trap stage 5a-claude
  documents for npm. Every gpg call in the stanza passes `--homedir`.
- `dnf5` prints "skipped OpenPGP checks for 1 package from repository:
  @commandline" — local package gpgcheck is OFF by default, so the stanza's own
  pinned-fingerprint `rpm -K` is the real verification, not decoration.
- SIZE: /usr/lib/chatgpt 1.3 GB, /usr/lib/claude-desktop 548 MB. ~1.9 GB into
  core, the tier whose rebuilds cost the fleet ~5 GB. Recorded in
  docs/update-cost.md as a stated tension, not silently.
- `python3-libdnf5` now enters the image as a ChatGPT dependency
  (`python3-libdnf5 if libdnf5`), for a scriptlet that does nothing on ostree.
- A negative control in my own trial was a no-op at first (`sed s/claude/cIaude/`
  on InRelease changed nothing — only `Claude` with a capital C appears). Redone
  with a real byte flip: `gpgv` exits 1. An assertion that cannot fail is worse
  than none, including in the trial harness.

## BLOCKED ON
- nothing
