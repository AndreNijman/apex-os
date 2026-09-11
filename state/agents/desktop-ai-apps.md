# desktop-ai-apps
items: (new, from the 2026-09-11 CLAUDE.md product decision — not a roadmap item)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ai-apps
branch: task/desktop-ai-apps

## NEXT
Write the Containerfile.core stanza (after 5a-zed) + tests/test-apex-ai-apps.sh,
after a scratch fedora:43 podman run proves the ChatGPT rpm installs and its
%post does not abort the build.

## DONE
- Worktree created at /var/tmp/apex-work/wt-ai-apps on task/desktop-ai-apps from
  origin/roadmap/v2.2 @ fbf06a4f, pushed with -u before any work.
- **ChatGPT desktop packaging: SETTLED — an official OpenAI Linux app exists and
  ships an RPM.** CLAUDE.md's "needs researching" is out of date. Measured, not
  assumed:
  - OpenAI released the ChatGPT desktop app for Linux in public preview on
    2026-08-11, supporting Fedora 43/44 (APEX's base) with x86_64 + aarch64
    rpm and deb packages.
  - Live rpm-md repo: `https://persistent.oaistatic.com/codex-app-prod/linux/rpm/$basearch`
    — `repodata/repomd.xml` fetched OK. It carries exactly ONE package, the
    current one: `chatgpt-26.908.40401-1.x86_64.rpm` (442,811,393 bytes),
    summary "ChatGPT by OpenAI", url https://developers.openai.com/codex/app.
    Older builds are not retained, so a pinned-version URL would rot; resolve
    latest at build time via repomd → primary.xml (the zed/sing-box posture).
  - Signed: `Header OpenPGP V4 RSA/SHA512 signature, key ID 4a3b4a566c4660e4`.
    The package carries its own key at
    `/etc/pki/rpm-gpg/RPM-GPG-KEY-chatgpt-3BFA0E4AE8B8CC16A2D9BA684A3B4A566C4660E4.asc`.
  - Payload: `/usr/bin/chatgpt` (relative symlink → `../lib/chatgpt/codex-launcher`),
    `/usr/lib/chatgpt/**` (Electron; 6824 files), `/usr/share/applications/chatgpt.desktop`,
    `/usr/share/pixmaps/chatgpt.png`, `/usr/share/metainfo/com.openai.chatgpt.metainfo.xml`.
  - **No systemd unit, no timer, no cron anywhere in the payload.** The app's
    update channel is the `/etc/yum.repos.d/chatgpt.repo` it ships
    (`enabled=1`, gpgcheck=1, repo_gpgcheck=1) — that file, not a daemon, is
    what has to be removed to satisfy criterion 3.
  - The one blocker on record (openai/codex#42948, "%post mkdir /var/lib/chatgpt:
    Read-only file system" on Fedora Atomic/Bazzite) is **fixed in this build**.
    Read out of the shipped rpm rather than trusted: `pretrans` now opens with
    `if [ -e /run/ostree-booted ]; then exit 0; fi` and `postinstall`'s
    `refresh_dnf5_keys()` returns early on the same test. No `/var/lib/chatgpt`
    anywhere in the current scriptlets.
  - So "there is no honest way to ship it" is NOT the answer: dnf5-install the
    vendor rpm into the image, verified against its own pinned key fingerprint,
    and delete the repo file it drops.
- Claude Desktop packaging confirmed: apt only, no rpm repo (five candidate rpm
  prefixes under downloads.claude.ai all 404; `apt/stable/dists/stable/InRelease`
  is 200 and signed). Latest `claude-desktop 1.52386.0` (l16's stopgap has
  1.49585.0). Deb unpacks to `/usr/lib/claude-desktop`, `/usr/bin/claude-desktop`
  (relative symlink), icons in hicolor 16/32/48/128/256, entry
  `com.anthropic.Claude.desktop`.

## IN PROGRESS
- Containerfile.core stanza + test suite.

## FOUND
- claude-memory MCP is down (502 Bad Gateway) this session; no vault context.
- **The ChatGPT scheme is `codex://`, not `chatgpt://`.** Its entry ships
  `MimeType=x-scheme-handler/codex;x-scheme-handler/http;x-scheme-handler/https;…`
  — it registers itself as an http/https handler. APEX already pins
  `x-scheme-handler/http=firefox.desktop` in `/etc/xdg/mimeapps.list` and
  asserts it at Containerfile.base:2002, so the default browser is safe; but
  ChatGPT will appear in every "Open With" list for a web link. Product call.
- Claude's entry is clean: `MimeType=x-scheme-handler/claude;`, plus
  `StartupWMClass=com.anthropic.Claude`, `SingleMainWindow=true` and two desktop
  actions (New Chat, New Claude Code Session).
- Claude's deb `postinst` registers Anthropic's apt repo + an
  unattended-upgrades snippet ("the VS Code / Chrome / 1Password model"), and a
  GNOME Shell search provider. Unpacking the deb instead of dpkg-installing it
  means none of that runs — which is exactly what criterion 3 wants. Its
  comments name a sibling `rpm-scripts.sh`, so an Anthropic rpm may be coming,
  but none is published today.
- ChatGPT's icon ships ONLY at `/usr/share/pixmaps/chatgpt.png` — nothing in
  hicolor. Spec-legal fallback, but it is the same shape as the Zed defect and
  the live editors test only searches `/usr/share/icons`.
- The repo has NO `ELECTRON_OZONE_PLATFORM_HINT`/ozone setting and NO
  appindicator package anywhere — the Wayland and tray halves of criterion 1
  have no support in the image today.

## BLOCKED ON
- nothing
