# followups-4 — pay down the two debt gates landed 2026-09-12

Worktree `/var/tmp/apex-work/wt-followups-4`, branch `task/followups-4`,
based on `roadmap/v2.2` @ `f2dfdbd0`.

No roadmap ids. Deadline: before final integration.

## The two debts

1. `tests/suites-not-in-ci.txt` — 9 DEBT lines. Wire each suite into a
   workflow job whose path selector actually reaches it, delete the line.
2. `tests/shellcheck-known-failing.txt` — 28 scripts. Fix or suppress with a
   reason, delete the line.

Both gates fail in BOTH directions, so a fixed entry cannot sit in the list.

## Established facts (measured, not assumed)

- `gh workflow run pr-validation.yml --ref task/followups-4` **works**, even
  though `origin/main`'s copy of the workflow has no `workflow_dispatch`
  stanza. First dispatch: run 34703613155. A push to a task branch triggers
  nothing (`push:` is `roadmap/v2.2` only), so dispatch is the whole runner
  feedback loop.
- Local (L16) results for the nine, before any change:
  - `test-apex-firewall.sh` 32/0/0
  - `test-apex-devices.sh` 55/0/0
  - `test-device-image.sh` 39/0
  - `test-apex-lid.sh` 60/0, 0 skips
  - `test-device-services-netns.sh` 12/0/0
  - `test-apex-trust-enforcement.sh` 77/0
  - `test-secret-at-rest.sh`, `test-apex-multilib.sh` — not yet run
  - `test-apex-firewall-ssh.sh` — **exits 2 with no `--target`**

## Predicted runner disagreements, to be confirmed not assumed

- `test-apex-trust-enforcement.sh` line ~80 uses `openssl req -x509
  -not_before`, an OpenSSL **3.5+** option. ubuntu-24.04 ships 3.0.13. This
  laptop has 3.5.7, which is why it passes here.
- `unshare -rmn` and `nft` availability on the runner.
- `test-apex-lid.sh` asks real logind; the runner cgroup is
  `/system.slice/hosted-compute-agent.service`, neither login session nor
  user service.

## NEXT

(in progress — see git log on task/followups-4)
