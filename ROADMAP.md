---
title: APEX-OS Agent-First Roadmap
version: "2.2.1"
date: 2026-09-06
canonical_task_file: roadmap.yaml
audience:
  - Claude Code
  - OpenCode
  - Codex CLI
  - Gemini CLI
  - human maintainers
purpose: >
  Canonical implementation roadmap for APEX-OS after the first major roadmap
  was substantially implemented. This revision incorporates the user's real
  Claude Code workflow: bypassPermissions by default, Remote Control, voice,
  push/input notifications, remote MCP memory, Codex inside Claude, a large
  local skill library, statusline quota/context telemetry, and persistent
  autonomous sessions.
---

# APEX-OS Agent-First Roadmap v2.2.1

## How agents should use this repository roadmap

`roadmap.yaml` is the canonical machine-readable task graph. This Markdown file
explains intent, invariants, interfaces, and acceptance philosophy.

Before implementing any task:

1. Inspect the current APEX-OS and APEX Shell code.
2. Map the task to existing implementation.
3. If it already exists, verify it against acceptance criteria instead of
   reimplementing it.
4. Prefer the smallest coherent change that satisfies the task.
5. Add tests before marking a task complete.
6. Record incompatible design discoveries in the task notes rather than silently
   changing the architecture.
7. Never weaken an APEX security boundary simply to reproduce an agent-native
   "bypass permissions" mode.
8. Keep upstream agent TUIs native. APEX should supervise and secure them, not
   replace their UI.
9. Keep configuration and APIs vendor-neutral where practical; use adapters for
   Claude/OpenCode/Codex/Gemini-specific behavior.
10. Update `roadmap.yaml` status and evidence when a task is completed.

## Status meanings

- `verify_existing`: believed implemented; prove it on the current integrated image.
- `partial`: foundation exists but acceptance criteria are not yet met.
- `todo`: not implemented or not found.
- `blocked`: requires upstream/hardware/vendor dependency.
- `done`: verified against acceptance criteria.

---

# 0. Existing foundations: preserve and regression-test

These features were part of the earlier roadmap and are believed to be substantially
implemented. They remain part of the canonical roadmap. Agents should verify them
before changing adjacent architecture and must not silently regress them.

- Atomic bootc image updates, system extensions, rollback, Secure Boot integration.
- Native upstream TUI agent execution through PTYs with persistence/attach/detach.
- `project`, `strict`, and explicit `unrestricted` agent sandbox modes.
- APEX Projects, Git worktrees, task checkpoints, and agent undo.
- APEX Capsules/development environments that keep the immutable host clean.
- Universal package abstraction over supported host/Flatpak/container sources.
- Declarative APEX Blueprint/apply/sync.
- APEX Modes and workload-aware policy.
- Gaming Mode and Performance Lab.
- Local AI service/runtime abstraction.
- Unified APEX Search/command surface.
- APEX Shell plugin platform with permissions.
- Compositor-neutral settings/adapters for Hyprland, niri, and Floating/labwc.
- Modern Floating/labwc theme integration, live generated keybinds, portals, input/output settings, APEX context UI, and emergency fallback behavior.
- Recovery, doctor, rollback, repair, diagnostics, and factory-reset flows.
- Disposable execution primitives.
- Multi-device handoff / remote build / remote agents / remote inference primitives.
- Boot v2 experimental path: composefs, systemd-boot, signed UKIs, boot counting,
  automatic rollback, measured boot, TPM-unlock experiments, while retaining GRUB
  for the production/legacy path until qualification is complete.

Every baseline item has a `BASE-*` verification task in `roadmap.yaml`.

# 1. Product target

APEX should be a polished atomic Linux operating system that works equally well
as:

- a normal consumer desktop;
- a modern traditional floating workstation;
- a dynamic tiling workstation;
- a niri scrolling workstation;
- a terminal-first developer machine;
- a persistent autonomous agent machine;
- a gaming system;
- a creator workstation;
- an AI/ML workstation;
- a secure remote-development machine;
- a reproducible multi-device environment.

The differentiator is not "AI built into the desktop". The differentiator is:

> Humans and agents can operate the same Linux machine with high autonomy while
> APEX keeps OS-level permissions, secrets, root privileges, cloud capabilities,
> rollback, project context, and audit state coherent.

---

# 2. Real Claude workflow assumptions

The roadmap MUST optimize for the user's actual Claude workflow rather than a
generic conservative user.

Current profile assumptions:

- Claude Code uses `bypassPermissions` as the normal/default permission mode.
- The dangerous-mode confirmation is intentionally suppressed.
- Claude commonly runs with Opus at `xhigh`.
- Remote Control is started automatically.
- Push and input-needed notifications are part of normal use.
- Voice / hold-to-talk is used.
- A remote MCP memory service is authoritative for persistent working memory.
- Codex is used from inside Claude through a plugin.
- A large local skill library is part of the Claude environment, including deep
  Cloudflare knowledge.
- The statusline tracks model, project, branch, context usage, 5-hour allowance,
  7-day allowance, and reset times.
- The Claude profile contains configuration, skills, commands, plugins,
  marketplaces, MCP definitions, statusline logic, and other reusable state.
- Some current tool credentials are still injected in ways that should migrate
  into the APEX broker model.

APEX must NOT nag on every Claude launch merely because Claude itself is in
`bypassPermissions`.

Default APEX behavior for this profile should be conceptually:

```text
Claude native permission prompts     OFF (inherited from Claude profile)
APEX project sandbox                 ON
APEX secret protection               ON
APEX root boundary                   ON
APEX cloud capability policy         ON
APEX audit                           ON
```

---

# 3. Non-negotiable security invariants

## 3.1 Separate the permission layers

These are distinct controls and must never be collapsed into one switch:

1. Agent-native permission mode.
2. APEX filesystem/process sandbox.
3. APEX system/root capability layer.
4. APEX secret/cloud capability layer.
5. APEX network policy.
6. Remote-origin policy.

`bypassPermissions` changes only layer 1.

## 3.2 Secret values are capabilities, not environment variables

Normal managed agents should not receive raw long-lived secrets where APEX can
broker the operation instead.

Preferred flow:

```text
agent/tool
  -> capability request
  -> policy decision
  -> broker-owned operation or temporary credential
  -> provider
  -> result
```

## 3.3 Root is delegated, not inherited

A managed agent must never become root merely because:

- the user authenticated with sudo earlier;
- the agent is in native bypass mode;
- the agent is in APEX unrestricted-user mode;
- Remote Control is active.

Root/system grants should be explicit, scoped, time-limited, auditable, and
bound to a concrete agent session.

## 3.4 "Unsafe everything" must exist

The owner is allowed to deliberately remove APEX protections.

Target CLI:

```bash
apex agent run --unsafe-everything --ttl 15m
```

Requirements:

- local active-session authentication;
- Polkit/PAM and optionally FIDO2/security key;
- explicit short TTL;
- prominent red Agent Center indicator;
- permanent audit record;
- automatic expiry;
- no silent persistence after reboot;
- no "remember forever";
- revocation control always visible;
- broker secrets are still not conveniently dumped into the agent environment.

Authentication reduces accidental activation. It does not make the resulting
root-capable session safe.

---

# 4. Agent permission modes

## 4.1 Inherited native mode

`a` should respect the selected agent profile. If Claude's profile defaults to
`bypassPermissions`, APEX should not override it.

## 4.2 Agent YOLO, APEX protected

Conceptual command:

```bash
apex agent run --agent-bypass
```

Expected state:

```text
agent-native confirmations   OFF
APEX project sandbox         ON
APEX secret broker           ON
APEX root boundary           ON
APEX audit                   ON
```

This should be the recommended high-autonomy mode.

## 4.3 Unrestricted user

```bash
apex agent run --sandbox unrestricted
```

Expected state:

- full user-level filesystem/process access;
- no project filesystem sandbox;
- still no automatic root;
- still brokered secrets by default;
- `no_new_privs` should remain active unless system-access mode explicitly
  changes it;
- consider clearing existing sudo timestamp/cache before launch.

## 4.4 System access

```bash
apex agent run \
  --agent-bypass \
  --sandbox unrestricted \
  --system-access
```

The user authenticates outside the agent PTY. The resulting grant is:

- opaque;
- session-bound;
- project-bound where applicable;
- capability-scoped;
- time-limited;
- non-transferable;
- non-renewable by the agent itself.

## 4.5 Break-glass full access

```bash
apex agent run --unsafe-everything --ttl 15m
```

This is deliberately different from system-access mode and should be treated as
a break-glass owner feature.

---

# 5. APEX agent profile system

The agent runtime should treat an agent installation as a profile, not merely an
executable.

Example Claude profile components:

```text
Claude profile
├── CLAUDE.md
├── settings.json
├── remote-settings.json
├── statusline.sh
├── commands/
├── skills/
├── plugins/
├── marketplace definitions
├── MCP definitions
└── reusable policy/config
```

Separate reusable profile state from machine-local secrets/runtime state.

Target CLI:

```bash
apex agent profile list
apex agent profile inspect claude
apex agent profile doctor claude
apex agent profile export claude
apex agent profile sync claude
```

Sandbox mounts should expose profile assets read-only where possible, with
separate controlled writable overlays for session/plugin runtime state.

---

# 6. Claude-native integration

## 6.1 Use Claude hooks before PTY heuristics

Claude's hook lifecycle should feed structured state into `apex-agentd`.

Use native events where available for:

- session start/end;
- tool use and failures;
- permission requests;
- task creation/completion;
- subagent start/stop;
- notifications;
- compaction;
- config changes;
- worktree events;
- file changes;
- stop/completion.

PTY/process heuristics remain the generic fallback for unknown agents.

## 6.2 Policy hook in bypass mode

An APEX Claude `PreToolUse` policy hook should provide defense in depth even
when Claude is in native bypass mode.

Hook denial is not a replacement for the kernel sandbox. The kernel sandbox must
still protect against hook bugs or bypasses.

## 6.3 Agent graph, not flat list

Agent Center should represent:

```text
Claude
├── Explore subagent
├── Plan agent
├── Codex reviewer
├── shell/tool processes
└── MCP sidecars
```

Cgroup/resource accounting and audit correlation should follow the same task
tree.

## 6.4 Statusline integration

Reuse the same structured information as the user's existing statusline where
possible.

Agent Center should expose:

- model;
- reasoning level;
- project;
- branch;
- context usage;
- 5-hour allowance;
- 7-day allowance;
- reset times;
- subagents;
- current tool;
- duration;
- resource usage.

Do not break or replace the user's terminal statusline.

---

# 7. Remote Control security

Remote Control is a normal workflow, not an edge case.

Every privileged/capability request should carry `request_origin`, such as:

```text
local-terminal
apex-shell
claude-remote-control
scheduled-job
mcp
subagent
cloud-job
```

Recommended default policy:

| Capability | Local | Claude Remote Control |
|---|---|---|
| Edit project | allow | allow |
| Run tests | allow | allow |
| GitHub push | allow | allow |
| Cloudflare preview deploy | allow | allow |
| Production deploy | configurable | configurable |
| Read raw brokered secret | deny | deny |
| Root capability | local auth | local approval required |
| Unsafe everything | local auth | local approval required |

Optional owner setting may allow remote elevation with strong WebAuthn/FIDO2
authentication, but remote root should not be default.

On screen lock:

- ordinary agents may continue;
- Remote Control may continue if configured;
- short-lived root grants should default to revocation;
- user policy may override.

---

# 8. Notifications and voice

## 8.1 Notification bridge

Do not duplicate Claude's push/input-needed notifications.

APEX should deduplicate and enrich:

```text
Claude - apex-os
Waiting for input
[Focus terminal] [Open project]
```

## 8.2 Voice

Provide a compositor-neutral global push-to-talk route to the active project or
focused agent session.

Requirements:

- clear microphone indicator;
- explicit routing target;
- no permanent microphone access to all agents;
- works in Hyprland, niri, and Floating/labwc.

---

# 9. Memory providers

Do not replace the user's existing remote MCP memory system.

Create an APEX memory-provider interface that manages lifecycle and status while
leaving the knowledge base authoritative in the user's chosen system.

Possible providers:

```text
Claude native memory
remote MCP memory
Obsidian
Mem0
custom MCP
none
```

APEX may track:

- connection state;
- project slug;
- last sync/checkpoint;
- provider health;
- auth capability used.

---

# 10. MCP architecture

## 10.1 Broker MCP authentication

Use helper-based/sidecar authentication so long-lived bearer tokens are not
stored in agent-readable config when avoidable.

Conceptual flow:

```text
Claude -> MCP auth helper -> apex-secretd -> temporary/auth header
```

## 10.2 Per-MCP sidecars

Each MCP server gets its own sandbox/capability profile.

A memory MCP does not need project filesystem access. A local coding MCP does
not need unrelated network/secrets. A Cloudflare MCP should not inherit the
user's entire home directory.

## 10.3 APEX MCP broker

Long-term:

```text
APEX MCP Broker
├── Cloudflare
├── GitHub
├── AWS
├── GCP
├── Kubernetes
├── local-system providers
└── custom MCP
```

Calls are still filtered by the APEX capability policy.

---

# 11. Secret and capability architecture

Introduce a dedicated protected secret service (`apex-secretd` or equivalent)
separate from both `apex-agentd` and broad `apexd`.

It should:

- store secrets outside user-readable agent paths;
- support TPM-backed/sealed storage where appropriate;
- return metadata/capabilities without returning values;
- execute broker-owned operations;
- mint or fetch temporary credentials when providers support them;
- rotate/revoke credentials;
- maintain an audit trail.

A generic capability record should include:

```text
provider
operation
resource
project
agent_session
request_origin
expiry
constraints
approval_policy
audit_id
```

---

# 12. Transparent tool brokering

The user's existing skills should continue to use normal tools.

Prefer transparent integration:

```text
git credential helper -> APEX
gh auth helper         -> APEX
wrangler auth          -> APEX
cloudflared auth       -> APEX
Terraform provider     -> APEX
MCP headers helper     -> APEX
```

Avoid requiring all existing Claude skills to be rewritten around `apex cf ...`
or `apex github ...`.

---

# 13. Full Cloudflare integration

Cloudflare is a first-party capability provider, not just a token stored in
Claude's environment.

## 13.1 Connection and project binding

Target:

```bash
apex cloudflare connect
apex cf connect
apex cf status
```

Preferred auth path: OAuth or appropriately scoped service/account token.
Manual token import remains available.

Project config should bind the intended account/zone/environment:

```toml
[identity.cloudflare]
account = "example-account"

[cloudflare]
zone = "example.com"

[cloudflare.preview]
worker = "project-preview"

[cloudflare.production]
worker = "project"
```

## 13.2 Capability vocabulary

Semantic operations, not one broad "Cloudflare write" permission.

Examples:

```text
cloudflare.account.read

cloudflare.worker.read
cloudflare.worker.upload-version
cloudflare.worker.deploy
cloudflare.worker.rollback
cloudflare.worker.tail

cloudflare.dns.read
cloudflare.dns.create
cloudflare.dns.update
cloudflare.dns.delete

cloudflare.r2.object.read
cloudflare.r2.object.write
cloudflare.r2.bucket.create

cloudflare.d1.read
cloudflare.d1.query
cloudflare.d1.migrate

cloudflare.kv.read
cloudflare.kv.write

cloudflare.queue.publish
cloudflare.queue.manage

cloudflare.hyperdrive.read
cloudflare.hyperdrive.edit

cloudflare.secret.create
cloudflare.secret.bind
cloudflare.secret.rotate

cloudflare.access.read
cloudflare.access.edit

cloudflare.tunnel.read
cloudflare.tunnel.edit

cloudflare.workers-ai.run

cloudflare.ai-gateway.run
cloudflare.ai-gateway.edit
```

## 13.3 Product coverage target

Developer Platform:

- Workers;
- Worker versions/deployments/rollback/tail;
- routes/static assets/Pages compatibility;
- KV;
- R2;
- D1;
- Durable Objects;
- Queues;
- Workflows;
- Containers;
- Hyperdrive;
- Vectorize;
- Analytics Engine;
- Browser Rendering;
- Images;
- Stream.

AI:

- Workers AI;
- AI Gateway;
- AI Gateway logs;
- BYOK/provider associations where supported;
- Vectorize;
- AI-related secret binding.

Networking/security:

- DNS/zones;
- redirects/transforms;
- cache purge/rules;
- WAF/rulesets;
- SSL/TLS/certificates;
- Turnstile;
- Access;
- service tokens;
- Tunnels;
- Zero Trust policy.

Operations:

- analytics;
- logs/tail;
- deployment/build status;
- usage;
- audit logs;
- spend/budget data where exposed.

## 13.4 Temporary task credentials

When Cloudflare supports scoped/temporary credentials, prefer short-lived task
credentials bound to exact account/resources/operations.

Do not pass even the temporary token directly to the agent if a broker-owned
`wrangler`/API child process can perform the operation.

## 13.5 R2

Use bucket/object-scoped credentials and capabilities.

This also becomes the preferred first-party cloud target for encrypted APEX
backups.

## 13.6 Cloudflare Secrets Store

Support creating/binding/rotating Cloudflare secrets through the APEX broker.

If a provider-side secret cannot be retrieved after creation, APEX stores only
the provider reference/metadata, not fictitious plaintext.

## 13.7 Transactional Worker deployments

Production flow should support:

```text
tests
-> APEX checkpoint
-> upload Worker version
-> preview
-> health check
-> staged traffic
-> full deployment
```

with rollback if health checks fail.

## 13.8 Environment protection

Recommended defaults:

```text
preview:    agent unattended deploy allowed
staging:    agent unattended deploy allowed
production: approval required
```

Owner can explicitly enable unattended production deployment per project.

## 13.9 DNS safety

Ordinary project grants may operate only on bound zones/records.

Registrar, nameserver, DNSSEC-root, ownership, and destructive account-level
operations belong in elevated/break-glass capability classes.

## 13.10 Cloudflare One

Broker service-token/Tunnel/Access credentials directly into protected storage.
The agent receives handles/capabilities, not plaintext secrets.

## 13.11 Cloudflare AI

APEX local AI service may route to:

- local model;
- Workers AI;
- AI Gateway;
- other cloud providers.

AI Gateway can be a provider/routing backend while APEX still owns local policy
and secret handling.

## 13.12 Cloudflare MCP

Cloudflare MCP tools should be routed through the APEX MCP/capability policy.
Do not hand provider credentials directly to the model just because the MCP
server supports OAuth.

## 13.13 Worktree preview environments

Optional per-worktree cloud preview:

```text
worktree
├── Worker version
├── preview hostname
├── D1 preview DB
├── KV preview namespace
└── temporary secrets
```

Destroy preview resources when the worktree is removed, after showing a clear
destroy plan.

## 13.14 Cost guardrails

Support per-task/project budgets:

```toml
[agent.budget]
cloudflare_daily = 5.00
workers_ai_daily = 3.00
```

Also support operation caps such as deployment count, R2 upload volume, AI spend,
and API request count.

---

# 14. Generic provider framework

Do not hard-code the capability architecture around Cloudflare.

Target providers:

```text
GitHubProvider
CloudflareProvider
AWSProvider
GCPProvider
KubernetesProvider
SSHProvider
DatabaseProvider
custom providers
```

Cloudflare should be the richest first-party implementation and reference
provider.

---

# 15. Task/audit graph

Upgrade flat JSONL logs into a correlated task graph.

A task should link:

```text
task
├── prompt / goal
├── agent session
├── request origin
├── subagents
├── worktree
├── commits
├── checkpoint
├── tests
├── secret capabilities
├── Cloudflare/GitHub operations
├── deployments
├── system privilege grants
├── remote-control events
└── final PR/result
```

Target CLI:

```bash
apex task inspect <task-id>
apex task timeline <task-id>
apex task audit <task-id>
```

---

# 16. Cross-agent handoff

Because Claude may delegate to Codex and quotas/context limits matter, define a
portable handoff packet:

```text
goal
plan
changed files
worktree
test state
important transcript summary
memory project slug
checkpoint
capability grants that may be re-requested
```

Target:

```bash
apex task handoff <task-id> codex
```

Agent-native delegation may use the same internal format.

---

# 17. Skills and plugin supply chain

## 17.1 Skills

APEX should understand skill metadata:

```text
name
description
origin
version/hash
scripts
references
permissions
```

Target:

```bash
apex agent skills
apex agent skills audit
apex agent skills sync
```

Pure reference skills and executable skills/plugins should have different trust
levels.

## 17.2 Plugins and marketplaces

Track:

- marketplace origin;
- Git commit/hash;
- update time;
- executable content;
- requested permissions;
- provenance.

Plugins execute inside the agent sandbox by default.

---

# 18. Cloud-side agent connectors are a different trust plane

APEX's Linux sandbox protects the local computer. It cannot automatically revoke
permissions already granted to the agent's cloud account, such as:

- Gmail;
- Calendar;
- Drive;
- cloud-side Claude connectors;
- remote MCP services;
- cloud jobs.

APEX security UI and profiles should distinguish:

```text
LOCAL CAPABILITIES
REMOTE/CLOUD AGENT CAPABILITIES
```

Strict/hardened profiles may use isolated agent profiles/tool sets with reduced
cloud connectors.

---

# 19. Agent lifecycle types

Agent Center should distinguish:

```text
local PTY
remote-controlled local PTY
recurring local task
scheduled local task
cloud job
remote-host agent
subagent
MCP sidecar
```

Do not treat all of these as one process-state model.

---

# 20. Finish existing agent-runtime gaps

Complete and verify:

- scoped GitHub capabilities;
- GitHub API capability vocabulary;
- Fish integration;
- Nushell integration;
- tmux integration;
- Zellij integration;
- editor/agent layout templates;
- screenshot -> TUI agent;
- drag file -> TUI agent;
- worktree test status;
- merge-conflict status;
- agent + disposable capsule integration;
- unattended capability execution without exposing generic root.

---

# 21. Floating/labwc maturity

Treat labwc as user-facing `Floating` mode, not a fallback compositor.

**Ratified 2026-09-12 — the other two names, decided rather than left open.**
`Floating` (labwc) was ratified here and shipped; the greeter and Settings now
present all three compositors by what they DO, so the remaining two needed the
same decision and have it:

| compositor | user-facing name |
|---|---|
| labwc | **Floating** |
| Hyprland | **Tiling** |
| niri | **Scrolling** |

The reasoning is the one already made for `Floating`: a person choosing a
session is choosing a way of arranging windows, not a software project, and the
project names leak an implementation detail that changes under them. `Tiling`
and `Scrolling` are also what the two upstreams call themselves in their own
first sentences, so nothing is invented here — niri is "a scrollable-tiling
Wayland compositor" and Hyprland "a dynamic tiling Wayland compositor", and the
shorter word is the distinguishing half of each.

`Scrolling` rather than `Scrollable tiling` because the pair has to be readable
in a session picker at a glance and the two names must not share a prefix; the
full phrase is for documentation, not for a carousel.

Enforced, not merely written down: `tests/check-compositor-naming.sh` (42/0)
asserts the mapping at source level and eight engine assertions in
`compositor-facade-test.qml` (60/0) assert it under a real headless labwc — the
pair exists because a grep checker alone stays green and blind when the
argument, rather than the string, is what breaks.

Required production validation:

- Firefox sharing;
- Chromium sharing;
- OBS;
- Discord;
- Flatpak portals;
- Steam;
- gamescope;
- Wine/XWayland;
- VS Code;
- JetBrains;
- LibreOffice;
- Blender;
- GTK;
- Qt;
- multi-monitor;
- scaling;
- fractional scaling;
- rotation;
- refresh-rate switching;
- VRR;
- suspend/resume;
- dock/hotplug;
- lock/idle;
- screenshots/recording.

Complete compositor-neutral Night Light.

Fix mixed-DPI APEX Shell scaling.

Preserve generated compositor-neutral keybind/settings architecture.

---

# 22. Creator-grade desktop

Add:

- ICC profile management;
- per-display color profiles;
- sRGB / Display P3 workflows;
- HDR;
- SDR/HDR mapping;
- Wayland color-management protocol support as upstream allows;
- calibration tooling;
- graphics tablet/stylus validation;
- MIDI;
- professional PipeWire audio profiles;
- USB audio interfaces;
- creator multi-monitor validation.

---

# 23. GPU parity

Performance Lab and workload policy must work across:

- NVIDIA;
- AMD Radeon;
- AMD APU;
- Intel Arc;
- Intel Xe/iGPU;
- Intel/NVIDIA hybrid;
- AMD/NVIDIA hybrid;
- eGPU where practical.

Expose only controls that have been tested safely on the hardware family.

---

# 24. Security baseline

## Firewall

Default incoming firewall with APEX UI over nftables/firewalld or equivalent.

## Disk encryption

Keep opt-in until real TPM/recovery qualification is complete.

Long-term target:

```text
LUKS2 default
TPM automatic unlock
recovery key
Secure Boot
measured boot
```

## FIDO2/security keys

Use for:

- dangerous-mode approval;
- root grants;
- secret export;
- high-risk remote elevation;
- optional unlock/recovery flows.

---

# 25. Transactional persistent-state migration

Atomic OS rollback is insufficient if persistent user/service schemas are
irreversibly migrated.

Create an APEX state-migration framework with:

```text
schema before
schema after
forward migration
rollback compatibility/strategy
checkpoint requirements
```

Protect:

- APEX databases;
- `/var/lib` service state;
- APEX Shell state;
- agent databases;
- project metadata;
- configuration schemas.

---

# 26. Update channels and staged rollout

Channels:

```text
stable
candidate
beta
edge
```

Support staged rollout such as:

```text
1% -> 5% -> 25% -> 50% -> 100%
```

With opt-in anonymous build-health telemetry, stop rollout on signals such as:

- boot failures;
- APEX Shell crash loops;
- GPU initialization failures;
- portal failures;
- network regressions.

---

# 27. Supply-chain verification

Ship:

- SBOM per image;
- dependency/package manifest;
- signed image provenance/attestations;
- client-side signature verification;
- kernel/module provenance;
- extension/package origin indicators;
- security advisory feed.

`apex status` should surface trust state clearly.

---

# 28. Backups

First-party:

```bash
apex backup
apex restore
```

Targets:

- local disk;
- NAS;
- SSH;
- S3-compatible storage;
- Cloudflare R2.

Requirements:

- client-side encryption;
- versioned backups;
- recovery key;
- brokered R2 credentials;
- no raw backup/cloud key exposed to agents.

---

# 29. Accessibility

A general-purpose distro must validate:

- screen reader;
- magnifier;
- high contrast;
- reduced motion;
- large text;
- color filters;
- sticky keys;
- slow keys;
- mouse keys;
- on-screen keyboard;
- keyboard-only installer;
- accessible login/lock/recovery.

---

# 30. Internationalisation

Implement and validate:

- keyboard layout before password creation;
- multiple layouts;
- IME/fcitx5;
- CJK;
- RTL;
- locales/timezones;
- translated installer;
- translated shell;
- per-user language;
- non-US recovery/install flows.

---

# 31. Boring desktop maturity

Validate and integrate:

- printing;
- scanning;
- SMB;
- NFS;
- WebDAV;
- MTP;
- cameras;
- SD cards;
- Thunderbolt;
- USB-C docks;
- VPN;
- WireGuard;
- OpenVPN;
- WPA2/WPA3 Enterprise;
- captive portals;
- hotspot/tethering;
- Bluetooth headsets/codecs;
- printer discovery;
- remote desktop;
- file sharing.

These are production requirements, not secondary polish.

---

# 32. Virtualization

First-party KVM/QEMU/libvirt integration for:

- Windows VM;
- Linux VM;
- snapshots;
- shared folders;
- USB passthrough;
- TPM;
- Secure Boot guests;
- disposable VM;
- developer VM;
- optional advanced GPU passthrough.

Support agent execution in disposable VMs for workloads that need a stronger
boundary than containers.

---

# 33. Hardware qualification database

With explicit user consent, record non-personal compatibility results such as:

```text
machine model
kernel
GPU
Wi-Fi chipset
firmware
APEX build
test result
```

Use it to expose machine-specific confidence:

```text
ThinkPad L16 Gen 2 AMD
✓ sleep
✓ audio
✓ Wi-Fi
✓ Bluetooth
✓ external monitor
✓ VRR
```

---

# 34. Agent resource budgets

Use cgroups for per-task/session limits:

```text
CPU
RAM
disk
I/O
network
GPU
runtime
```

Target:

```bash
apex agent run --cpu 8 --memory 16G --timeout 2h
```

Agent Center should show current resource use and budget exhaustion.

---

# 35. Secure browser automation

Create isolated browser-agent profiles/capsules separate from the user's personal
browser.

Provide:

- independent cookies/profile;
- explicit downloads area;
- capability-based credential use;
- disposable state;
- audit association with task;
- no access to personal browser secrets by default.

---

# 36. Per-project identity

Project metadata should bind identities explicitly:

```toml
[identity.github]
account = "example"

[identity.cloudflare]
account = "example"

[identity.ssh]
host_group = "robotics"

[identity.agent]
default = "claude"
```

This prevents deploying or pushing from the wrong account.

---

# 37. Boot architecture

Keep the current production boot path stable while Boot v2 is qualified.

Long-term UEFI target:

```text
UEFI
-> shim / Secure Boot
-> systemd-boot
-> signed UKI
-> composefs/bootc deployment
```

Keep GRUB for legacy BIOS.

Complete:

- automatic failed-update rollback;
- health-based boot success;
- APEX-owned EFI identity;
- measured boot;
- real TPM validation;
- TPM-clear/firmware-update recovery;
- suspend/resume validation;
- encrypted-by-default only after qualification.

---

# 38. Immediate phase order

## P0 - protect autonomy and prove current image

1. Current-image real hardware qualification.
2. Protected secret-service architecture.
3. Migrate Claude/GitHub/MCP credentials out of agent-readable config.
4. Separate native bypass, APEX sandbox, root, secret, network, and remote-origin policies.
5. Inherit agent-native permission mode by default.
6. Claude profile mount/overlay system.
7. Claude native-hook bridge.
8. Request-origin tracking for Remote Control.
9. Local-auth/FIDO-gated system-access and unsafe-everything modes.
10. Agent network modes: open, allowlist, brokered, offline.

## P1 - capability platform and mature workstation

11. Full Cloudflare provider.
12. Cloudflare temporary task credentials and broker-owned tool execution.
13. Cloudflare Secrets Store.
14. Transactional Worker preview/deploy/rollback.
15. Cloudflare MCP through APEX policy.
16. Cloudflare worktree preview infrastructure.
17. Generic provider framework.
18. GitHub capability provider improvements.
19. MCP sidecar isolation and auth helpers.
20. Agent graph/subagent accounting.
21. Cross-agent handoff format.
22. Skill/plugin provenance and audit.
23. Finish terminal/TUI integration gaps.
24. Floating/labwc physical validation.
25. Mixed DPI and compositor-neutral Night Light.
26. Creator color management.
27. AMD/Intel/NVIDIA performance parity.
28. Default incoming firewall.
29. Persistent-state migration framework.
30. Staged update channels.
31. SBOM/provenance/supply-chain status.

## P2 - general-purpose OS completeness

32. Encrypted backups + R2.
33. Accessibility.
34. Internationalisation.
35. Printing/scanning/network shares/VPN/captive portal/etc.
36. Virtualization.
37. Hardware qualification database.
38. Agent resource budgets.
39. Secure browser automation.
40. Per-project identity.
41. Remote/multi-device task execution maturity.

## Later - security defaults after qualification

42. LUKS2 by default.
43. TPM automatic unlock by default.
44. stronger remote root approval options.
45. broader cloud-provider capability ecosystem.

---

# 39. Definition of done for an APEX roadmap task

A task is not complete because code exists.

A task reaches `done` only when:

1. implementation exists in the intended production path;
2. unit/integration tests cover the critical behavior;
3. failure behavior is tested;
4. the feature works on at least the supported baseline hardware/software matrix;
5. CLI/API behavior is documented;
6. security boundaries have explicit tests where relevant;
7. upgrade/rollback compatibility is known;
8. APEX Shell behavior is tested if user-facing;
9. task evidence is recorded in `roadmap.yaml`;
10. no known blocker is hidden behind optimistic wording.

---

# 40. Final architecture

```text
                               APEX-OS
                                  |
        +-------------------------+-------------------------+
        |                         |                         |
      Humans                    Agents                    Services
        |                         |                         |
        +-------------+-----------+-----------+-------------+
                      |                       |
              APEX project/task layer   APEX capability layer
                      |                       |
          +-----------+---------+       +-----+-------------------+
          |                     |       |                         |
       projects              agent graph secrets/root/network   cloud
          |                     |       |                         |
  worktrees/checkpoints     Claude      apex-secretd/apexd    providers
                                |                              |
                       +--------+--------+            +--------+--------+
                       |        |        |            |        |        |
                     Codex    MCPs   tools/hooks    GitHub Cloudflare  ...
```

User-facing shell:

```text
APEX Shell
├── Projects
├── Agents
├── Cloud
├── System
├── Gaming
├── Performance
├── Recovery
├── Security
└── Settings
```

The normal terminal experience remains:

```bash
cd ~/Projects/apex-os
claude
```

or simply:

```bash
a
```

with the user's native Claude bypass mode preserved.

APEX's job is to make that highly autonomous workflow safer, more observable,
more reproducible, more cloud-capable, and easier to recover from without taking
away the native terminal agent experience.


---

# 41. Mandatory `/stop-slop` completion gate

Every roadmap task has a mandatory `stop_slop` gate in `roadmap.yaml`.

Before an implementation agent marks ANY task done, it must run:

```text
/stop-slop
```

and review every human-facing string touched by that task.

This applies to:

- APEX Shell labels, help text, descriptions, empty states, confirmations and warnings;
- notifications and errors;
- Android app copy;
- installer/recovery text;
- READMEs and docs;
- release notes/changelogs;
- comments that are meant to explain behavior to human maintainers;
- PR/commit descriptions drafted by an agent;
- cloud/deployment/help text.

Do not "stop-slop" machine interfaces. Preserve literal commands, code identifiers,
paths, config keys, API names, protocol fields, logs and technical values when changing
them would reduce correctness.

A task cannot move to `done` until both `tests` and `stop_slop` gates pass.

---

# 42. Immediate APEX Shell correctness and Settings overhaul

The current public APEX Shell still describes the Config pages as post-v0.1 work and
documents mixed-monitor scaling/top-bar clipping as known issues. The current shared
`CfgSlider.qml` explicitly handles mouse/touchpad wheel events to modify values. The
user's current installed image additionally reports several functional failures.

These are P0 product bugs, not cosmetic backlog.

## 42.1 Agent Settings: Always Unrestricted

Add a dedicated `Config -> Agents` settings page.

Primary control:

```text
Always unrestricted agents                  [ OFF ]
Use full user-level access for new agents.
Root and brokered secrets remain protected.
```

Enabling:

1. User toggles ON.
2. APEX opens a real local Polkit/PAM authentication prompt.
3. On success the persistent default APEX sandbox becomes `unrestricted`.
4. All newly started managed agents use unrestricted-user mode.
5. Existing sessions keep their current mode and display it honestly.
6. A persistent warning indicator is visible while the default is unrestricted.

Disabling:

- no password required;
- immediately restores the normal protected default for new sessions;
- Claude may still use its own `bypassPermissions` because native bypass and APEX
  sandbox mode are separate layers.

This setting MUST NOT grant root, disable the secret broker, or silently enable
`--unsafe-everything`.

## 42.2 Fix the top navigation

The dashboard top navigation must never overlap.

The current dashboard has six primary tabs:

```text
Home  System  Agents  Tasks  Apps  Config
```

The entire dashboard/settings frame must become responsive across small displays and
mixed-DPI scaling. Do not solve this by making text unreadably small.

## 42.3 Display settings need a real transaction

The monitor Apply workflow is safety-critical.

Expected flow:

```text
stage changes
    |
    v
Apply
    |
    v
temporary compositor/output configuration
    |
    v
visible countdown on safe output
  15 14 13 ...
    |
    +-- Keep -> persist
    |
    +-- Revert -> exact previous state
    |
    +-- timeout/failure -> automatic revert
```

A missing confirmation dialog must never leave the user unable to keep or recover
changes.

## 42.4 Input settings may not be fake

Every visible control must be wired to a real compositor/device backend and must read
back effective state. If a setting cannot be changed on the current compositor/device,
disable it and say why.

## 42.5 Scrolling must never modify sliders

All settings value bars must ignore mouse wheel and touchpad scrolling. Scrolling over
them scrolls the settings page. Values change through intentional click/drag or
keyboard access.

## 42.6 Agent colors

Do not flatten Agent Center or embedded PTY/TUI output to white. Use APEX theme tokens
for UI state, and preserve ANSI terminal colors/attributes in terminal rendering.

---

# 43. Agents and Workspaces onboarding/help

The Agents tab needs an obvious help entry at the top:

```text
?  How Agents & Workspaces work
```

It must explain from zero knowledge:

- what an APEX-managed agent is;
- how to launch Claude/OpenCode/Codex/Gemini;
- what `a` does;
- native agent bypass vs APEX protection;
- Always Unrestricted;
- projects and workspaces;
- attach/detach and persistent PTYs;
- worktrees and parallel agents;
- agent graph/subagents;
- diffs/tests/conflicts;
- checkpoints and undo;
- capability approvals;
- cloud previews;
- remote control / Android;
- keyboard shortcuts;
- matching CLI commands;
- how to recover a stuck session.

First-run guidance should be dismissible. The full help remains permanently available.

---

# 44. Blueprint UX redesign

Blueprint should answer four questions immediately:

1. What is a Blueprint?
2. What will change?
3. Can I preview it?
4. How do I undo/recover?

Primary user flow:

```text
Blueprint

[Create from this device]
[Import]
[Sync]

Current device vs Blueprint
+ 12 apps
~ 4 settings
- 2 packages

[Preview changes]
[Apply]
```

Group settings into Apps, Desktop, Agents, Packages, Modes, Identities and Cloud.
Raw configuration editing is an Advanced feature, not the first experience.

---

# 45. Gaming Settings redesign

Gaming configuration should feel like consumer settings, not a tuning worksheet.

Recommended default surface:

```text
Gaming

Optimize games automatically                     [ ON ]
Use the best APEX game profile while a game runs.

Performance boost while gaming                    [ ON ]
Variable refresh rate when available               [ ON ]
Game overlay                                       [ OFF ]
Controller-first Gaming Mode                       [ OFF ]
Pause background updates while gaming              [ ON ]

Advanced >
```

Hardware-specific scheduler/GPU/gamescope controls belong under Advanced and must not
appear enabled when the hardware cannot support them.

---

# 46. Full APEX Remote agent platform

Create a first-party remote-control system that works with the exact agents APEX
already manages. It is provider-neutral and does not depend on Claude Remote Control.

Desktop side:

```text
apex-agentd
     |
apex-remoted   (per-user, non-root)
     |
     +-- direct LAN
     |
     +-- encrypted internet relay
```

Android side, working title:

```text
APEX Remote
```

## Required Android features

### Computers
- pair by QR code;
- multiple APEX computers;
- revoke device from desktop;
- clear online/offline/connection-path state.

### Agents
- complete Agent Center list/graph;
- Claude/OpenCode/Codex/Gemini/generic PTY;
- start new agent;
- project/profile/worktree selection;
- pause/stop/attach;
- actual sandbox mode and request origin.

### Full terminal
- real PTY streaming;
- ANSI 16/256/truecolor;
- bold/dim/underline;
- scrollback and search;
- text selection/copy/paste;
- Ctrl/Esc/Tab/arrows/function-key accessory row;
- reconnect without losing the running agent.

### Projects and work
- workspaces;
- worktrees;
- task status;
- changed files/diff summary;
- tests;
- conflicts;
- checkpoints;
- undo;
- PR readiness.

### Approvals
- GitHub/cloud capability requests;
- Cloudflare preview/staging/production deployment requests;
- clear audit trail.

Root/system access remains local-approval-only by default.

### Input and media
- send prompt;
- voice push-to-talk;
- photo/screenshot/file to a safe project/temp path;
- explicit encrypted clipboard handoff.

### Notifications
- input needed;
- task complete;
- failed;
- test failed;
- deployment;
- approval request.

### Security
- app biometric/device-credential lock;
- paired per-device identity;
- no raw APEX secret-broker values;
- every action audited with device identity;
- end-to-end encryption for internet relay payload;
- device revocation.

## Connectivity

Preferred behavior:

1. direct local/LAN connection when available;
2. automatic encrypted internet fallback without router port forwarding;
3. Cloudflare-backed rendezvous/relay is a first-party integration;
4. relay sees ciphertext, not PTY/task content;
5. reconnect automatically across Wi-Fi/mobile network transitions.

The Android app must be production-quality, accessible, resilient to backgrounding,
battery saver, rotation, tablets/foldables, and poor networks.

---

# 47. Capability-native application security

The capability architecture should eventually protect normal applications as well as
agents.

Target conceptual model:

```text
APEX Capability System
├── Agents
├── Apps
├── Shell plugins
└── Services
      |
      +-- files
      +-- network
      +-- camera
      +-- microphone
      +-- screen capture
      +-- USB/devices
      +-- sensitive directories
      +-- secrets
      +-- system capabilities
```

Do not pretend native applications and Flatpaks have identical enforcement. Expose
the actual effective protection mechanism and limitations.

---

# 48. Remaining OS platform pillars

Add these to the long-term definition of a complete APEX platform:

## Storage Manager
SMART/NVMe health, wear, temperatures, TRIM, filesystem health, free-space pressure,
removable media, encryption, mounts and failure warnings.

## Firmware lifecycle
fwupd/LVFS, BIOS/UEFI, dock/Thunderbolt/SSD/device firmware, battery health and charge
thresholds.

## Multi-user
Admin/standard users, fast switching, per-user agent profiles/secrets, disposable
guest sessions and kiosk/shared-machine scenarios.

## Online Accounts
Brokered Nextcloud/Google/Microsoft/WebDAV/S3/R2-style account integration without
spraying long-lived credentials through user config.

## Safe Graphics
A conservative graphical recovery environment that still works when the normal
compositor, GPU stack or APEX Shell is broken.

## Fleet architecture
Blueprint/policy/update rings, enrollment, inventory, managed secrets/certificates,
compliance and remote recovery, while staying optional for personal installs.

## Chaos testing
Continuously test bad timing: power loss, full disk, network loss, broken monitor
layout, shell crash, daemon crash, expired token, killed agent during checkpoint,
TPM changes, suspend during work and repeated update/rollback cycles.

---

# 49. v2.2 completion philosophy

The roadmap is now large enough that new conceptual features should be added only when
they close a real platform gap.

For implementation agents, the order is:

1. Fix P0 correctness and security issues.
2. Qualify the current image.
3. Make Settings truthful and transactional.
4. Finish the Cloudflare/capability platform.
5. Build APEX Remote + Android.
6. Finish desktop/gaming/creator/hardware maturity.
7. Fill general-purpose OS gaps.
8. Expand only after existing features are physically tested.

Every task ends with tests and `/stop-slop`.


---

# 50. Hyprland Lua configuration migration - release blocker

Hyprland 0.55 deprecated the old hyprlang compositor configuration in favor of Lua.
Upstream stated that the legacy format would remain for only 1-2 releases. The current
APEX system is already warning that the legacy configuration path will be removed in
Hyprland 0.57.

This is P0. APEX must migrate before that removal lands in the shipped Hyprland version.

## Target layout

Instead of an active legacy chain such as:

```text
~/.config/hypr/hyprland.conf
  source = monitors.conf
  source = input.conf
  source = keybindings.conf
  source = rules.conf
```

use a Lua-native configuration:

```text
~/.config/hypr/
├── hyprland.lua
└── apex/
    ├── monitors.lua
    ├── input.lua
    ├── keybindings.lua
    ├── rules.lua
    ├── appearance.lua
    ├── workspaces.lua
    └── autostart.lua
```

with `hyprland.lua` loading APEX-owned modules through `require()`.

## APEX Settings must become Lua-native too

This is not complete if the base config is Lua but Settings still writes old `.conf`
fragments.

The following APEX paths must generate or call the current Hyprland Lua APIs:

- Display/monitor configuration.
- Input and per-device configuration.
- Keybinds.
- Workspace rules.
- Window/layer rules.
- Appearance/layout/animation configuration owned by APEX.
- Startup/environment behavior owned by the Hyprland compositor config.

The exact APIs must follow the Hyprland version APEX ships. Do not freeze copied examples
if upstream has changed the Lua API.

## Migration of existing users

On first boot/login after the migration:

1. Detect an APEX-managed legacy Hyprland configuration.
2. Back it up with a timestamp.
3. Convert APEX-owned/generated state to the Lua module layout.
4. Migrate recognized user overrides.
5. Report any legacy directive that cannot be converted safely.
6. Never silently discard custom keybinds, rules, monitor settings, or input settings.
7. Make the migration idempotent.

Do not continually regenerate over user-owned custom Lua.

## Validation

After any generated change:

```text
hyprctl reload
hyprctl configerrors
```

must be clean.

CI must reject active legacy Hyprland compositor `.conf` generation and should test both
the supported stable Hyprland build and an upstream/nightly compatibility target where
practical.

This task also blocks completion of Display/Input/Keybind Settings work if those paths
still persist deprecated hyprlang.

Important: Hyprland's move to Lua does not mean every `hypr*` tool has moved to Lua.
Do not mechanically convert configs for hypridle, hyprlock, or other related tools
unless that specific upstream tool requires it.

Roadmap task: `P0-025`.
