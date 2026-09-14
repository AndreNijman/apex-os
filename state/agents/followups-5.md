# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
Sweep the tree for `PRODUCER | grep -q` under `set -o pipefail` where the
producer can be SIGPIPEd (tar/cat/find/journalctl/large printf) — the
mirror-image defect fixed as `94a3a2ac`. Then read run 34814935040 (the tip
run on `654fa854`, dispatched 06:47Z by the landing push, in flight) and
confirm `Run virtualization assertions` went green — `df6c4797` is IN it.

## DONE
- **ROUND 27 VERIFICATION, read out of run 34803818557 job 103851638407
  (`Package engine`, on `479105b6`): 62 steps green, ONE red.** All seven
  persistent engine reds are FIXED AND CONFIRMED ON THE RUNNER, with counts
  byte-matching what I measured locally:
  `inject 48/0`, `worktrees 61/0`, `disposable 58/0`, `profile 48/0`,
  `secret-broker 66/0` (= `bd69bb2f`, 188 assertions that had never run),
  `shell-agent 58/0` (= `5229bef4`), `root-approval 20/0` (= `479105b6`).
  The single red is `apex-vm: 74 passed, 60 failed` — the run predates
  `df6c4797`, which is the fix, so this is expected, not a regression.
  BOTH intermittents were GREEN on this run: `labwc-keybind-reload 18/0`
  and `mux-layouts 46/0`.
- `df6c4797` test(vm): the one probe that could not be faked with $PATH read
  the host. PUSHED+LANDED. All 60 apex-vm FAILs were `require_stack`'s
  `have_kvm` reading the real /dev/kvm, in a suite that fakes virsh/qemu-img/
  lsusb/swtpm through $PATH and whose subject is the domain XML. Now
  `KVM_NODE="${APEX_VM_KVM:-/dev/kvm}"`. Local: before 133/0 here and
  **74/60** without a node — the runner's own line, now confirmed identical
  on the runner — after 138/0 in BOTH.
- `479105b6` test(root-approval): `mount --bind` cannot create a target only
  APEX machines have. Mirror the DIRECTORY and bind it. Runner now 20/0.
  Near-miss recorded: v1 `cp`'d a stub over a mirrored SYMLINK to the real
  engine; safe here only because /usr is ro, would have eaten the shipped
  engine on every runner.
- `5229bef4` test(shell): guard asked the ambient PATH, assertions a scrubbed
  one. 23 nushell FAILs were a missing `/usr/local/bin`. Runner now 58/0.
- `bd69bb2f` test(agent): five engine suites stopped at the first session CI
  could not place; they now re-enter through `tests/in-login-session.sh`.
- `94a3a2ac` ci(chaos): the archive guard failed exactly when the path WAS
  present — `tar -tzf A | grep -q P` under pipefail. LANDED.
- `13e7ec84` ci: 93 `run:` steps carry `if: ${{ !cancelled() }}`. LANDED.
- `69ef58cf` / `cf7722d9` / `2a8ada6f` / `c07574b7` — see git log.

## IN PROGRESS
- Sweeping `| grep -q` under pipefail across the tree (558 raw hits; filtering
  to SIGPIPE-able producers in pipefail scripts).
- Run 34814935040 on tip `654fa854` in flight: Static ✓, Android ✓, Rust and
  Package engine still running.

## FOUND
- **THE 1 → 8 IS NOT A REGRESSION.** `13e7ec84` (`!cancelled()`) is why the
  engine job reports EIGHT/NINE red steps where it used to report ONE. A red
  step used to switch off every step below it, so the count was never a count
  of defects — it was a count of how far the job got. READ THE RISE AS
  VISIBILITY, NOT DECAY. It is now vindicated: the same job on `479105b6`
  runs all 63 steps and reports 62 ✓ / 1 X.
- **TWO SUITES REPORT `0 passed, 0 failed` ON THE RUNNER** — the dominant
  defect family, a gate that runs and inspects nothing: `hypr-lua: 0 passed,
  0 failed` and `input-live: 0 passed, 0 failed`. Both are ✓ steps. Under
  investigation.
- **The intermittents are environmental, not flaky-by-race.** Both were green
  on 34803818557 and each was red on exactly one earlier run.
- **THE GENERAL CLASS** — a gate that reads state belonging to the environment
  it runs in: (1) §26 channels leaked to the real `/proc/mounts` (runner /usr
  rw, L16 ro); (2) Input-page parity picked its comparison target by BRANCH
  NAME, falling back to the other repo's `main`; (3) Chaos diagnostics
  uploaded nothing for its whole life.
- **A result must be read out of a gate's LOG, not off a pipeline's exit
  code.** `cargo test … | tail -60` reports `tail`'s 0 over a truncated log.
- P3 labwc CLOSED (run 34717723637). **77 steps gated, not 45.**

## BLOCKED ON
- nothing
