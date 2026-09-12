# p2-a — encrypted backup framework, R2 backup backend, per-project identity binding

* **Items**: P2-001, P2-002, P2-013
* **Repo**: apex-os
* **Worktree**: `/var/tmp/apex-work/wt-p2-a`
* **Branch**: `task/p2-a-backup`, from `origin/roadmap/v2.2` @ `24472b64`. Pushed before any work. **Never rebased.**
* **Dispatched**: 2026-09-12, after `p1-005` (the R2 capability set) landed.

---

## Next action

*(kept current — a fresh agent replaces the dead one by reading this line)*

**→ P2-001 and P2-002 are done and pushed (5 commits). Now P2-013: generalise
`[identity.*]` to github/ssh/agent, find the enforcement seam for each, and
report honestly where there is none.**

---

## What exists versus what this unit adds

Recorded before any code was written, because three of these facts changed the
design and one of them would have produced a backup that silently corrupts.

### P2-001 — encrypted backup framework

**Exists:** nothing. There is no backup implementation, no backup format, no
backup timer, no borg/restic/rsync/kopia/duplicity anywhere in the tree. The
only place in the repo that names the feature is a doc comment in the Cloudflare
provider, `apexd/apex-secretd/src/providers/cloudflare/mod.rs:51-73`, which says
§13.5 calls R2 *"the preferred first-party cloud target for encrypted APEX
backups"* and then: *"the object write below is the half of it that does not
need a backup format to exist first."* This unit is the other half.

Four things in the tree use the word "backup" and none of them is one:
`apex sync`'s `blueprint.toml.previous` copy (`apexd/apex/src/blueprint.rs:1589`),
`apex recover reset`'s pre-delete copy to `~/apex-reset-backup-<ts>/`
(`apexd/apex/src/recover.rs:1732`), `wipefs --backup=` filesystem-signature
rescue (`apexd/apexd-core/src/storage.rs:1013`), and the de-brand script's
grub-entry copy. "Snapshot" in this tree means a fan curve, a firmware vendor
table or a D-Bus metrics property — never a filesystem or a backup.

**Adds:** a new workspace crate `apex-backup-core` (format, crypto, targets,
inventory, restore) and an `apex backup` subcommand.

### P2-002 — Cloudflare R2 backup backend

**Exists:** the whole credential half, and it is good. `cloudflare.r2.object.read`,
`cloudflare.r2.object.write` and `cloudflare.r2.bucket.create` are declared at
`cloudflare/mod.rs:658/672/1011`; every one of them resolves its bucket through
`Binding::bucket()` at `binding.rs:834`, which reads `[cloudflare] buckets` out
of the project's own `apex.toml` and refuses anything else with
`NoBucket { path, named, bound }`. The agent never holds the token: the broker
spends it inside a `curl -q -K -` child that reads the credential off a pipe as
a config file, never argv (`providers/cloudflare/api.rs`). There is a loopback
`Fake` in `cloudflare/tests.rs:182` that 401s an unauthenticated request.

**Adds:** an R2 *target* for the backup framework that spends those operations.
No new Cloudflare operation is needed — see the three constraints below.

### P2-013 — per-project identity binding

**Exists:** the file and the reader. `apexd/apex-secret-core/src/project.rs`
reads `<project>/apex.toml` from a root daemon safely: every component below the
project root opened `O_NOFOLLOW` one `openat` at a time, the file must be a
regular file owned by the account the operation runs as, size-capped at 64 KiB,
and a parse failure reports a line and column and **never the text there**,
because that message reaches the audit trail. `ProjectConfig` is deliberately
schema-free (`path` + `toml::Table`) with `string()`/`strings()`/`sections()`
accessors, so a new section costs no parser change. `project.rs:55` already
declares the generic shape — *"§36 describes one file holding `[identity.*]`
for every provider, so the name is deliberately not provider-specific. P2-013
generalises the rest of it"* — and `project.rs:389` names
`[identity.github]` and `[identity.ssh]` as this item's.

Today exactly one identity is consumed: `[identity.cloudflare]`
(`account`, `account_id`), read at `binding.rs:630`.

**Adds:** `[identity.github]`, `[identity.ssh]`, `[identity.agent]` — read,
resolved, enforced, and visible.

---

## Three constraints in the landed R2 path that shaped the format

Each was read out of the code rather than assumed, and each changed a decision.

1. **`MAX_PAYLOAD = 10 MiB`** (`project.rs:68`) caps `r2.object.write`, and
   **`HTTP_MAX_BYTES = 3 MiB`** (`broker.rs:398`) caps every brokered reply, so
   it caps `r2.object.read`. A snapshot cannot be one object. It is chunked, and
   the read cap is the binding one.

2. **A brokered reply is `String::from_utf8_lossy(&out.stdout)`**
   (`broker.rs:581`). Raw ciphertext read back out of R2 through the broker
   would have every invalid UTF-8 sequence **silently replaced with U+FFFD** —
   a restore that returns corrupt bytes and no error. So an R2 chunk object is
   **base64 text, not raw ciphertext**. The 4/3 expansion is why the chunk is
   1 MiB of plaintext and not 2: 1 MiB + a 16-byte tag, base64'd, is ~1.37 MiB,
   comfortably inside the 3 MiB read cap.

3. **`r2.object.write` takes a *project-relative file*, not bytes**
   (`object_body`, `cloudflare/mod.rs:1952`), read through the same
   `project::read_file` `O_NOFOLLOW` walk. So chunks are staged inside the
   project root. The staging directory cannot be dot-prefixed:
   `operation::valid_name` (`operation.rs:626`) requires the first byte of every
   path segment to be alphanumeric or `_`, so `.apex-backup/` is refused by the
   framework before the provider sees it. The directory is **`_apex-backup/`**.

   There *is* a bytes path — `Request::Use` frames a body and `Bind::body`
   carries it — but `MAX_BODY_BYTES` is 1 MiB (`apex-secretd/src/main.rs:459`),
   smaller than the file path's 10 MiB, and using it would mean changing a
   landed operation's contract. The file path costs nothing and changes nothing.

4. **Enumeration already exists.** `cloudflare.r2.object.read` with a bucket and
   no key *lists the bucket* — one operation and not two, deliberately
   (`mod.rs:658`, rendering at `mod.rs:1345`). Versioned restore needs no new
   operation and no index object, so there is no lost-update race to reason
   about.

---

## The key-custody decision, and why it is not in apex-secretd

`apex-secretd` is the obvious home for a backup key and it is the wrong one, for
a reason that is a property of its contract rather than a matter of taste.

Its defining invariant is that **a value never comes back**:
`protocol.rs:7-21` — "no response variant may carry a raw credential… not
base64-encoded in a string", enforced by `SecretValue` not implementing
`Serialize` and pinned by `protocol_surface_is_pinned`. Client-side encryption
requires the process doing the restore to hold a key. A verb that returned one
would be the exception that sentence exists to refuse.

So the backup key is **asymmetric**, and the two halves live in different
places:

* **public half** — `/var/lib/apex-backup/keys/<uid>.pub`, 0644. Backing up
  needs *only* this. An unprivileged user, and an agent, can create a backup.
* **private half** — `/var/lib/apex-backup/keys/<uid>.key`, root-owned 0600
  inside a 0700 root-owned directory. Restoring needs it, so restoring needs
  root. An agent at the owner's uid gets EACCES.

This is a stronger answer to P2-002's second criterion than a symmetric key
would be: the backup path never touches the private key at all, so there is
nothing on that path to leak — an agent that is wholly compromised cannot
decrypt even the snapshot it just wrote.

**Named limitation, stated up front:** a private key that exists only on this
machine means losing the machine loses every backup. Escrow/export is not in
this unit unless budget remains; it is written down here rather than left to be
discovered.

## Scope call, made before writing code

P2-001 criterion 1 names five target kinds. Three are real in this unit —
**local**, **nas**, **r2**. **ssh** and **s3** are declared in the target
vocabulary and **refuse with a reason naming what is missing**; neither gets a
fake success path. s3 needs SigV4, which this workspace has no signer for and
which P1-005's own evidence records as the reason R2's temp credentials were
unusable. ssh needs a loopback sshd fixture. Criterion 1 will be reported
**partial** with exactly that split.

---

## Log

### 2026-09-12 — round 1, orientation

Worktree created off `24472b64`, branch pushed before any work. Read
`providers/cloudflare/` (10767 lines across five files), `apex/src/verify.rs`,
`apex-secret-core/{project,protocol,operation,store}.rs`, `broker.rs`,
`tests/test-secret-at-rest.sh`. Recorded the exists/adds split above and the
four R2 constraints. No code yet.

### 2026-09-12 — round 1, P2-001 and P2-002 landed on the branch

Five commits on `task/p2-a-backup`, all pushed. Nothing rebased.

* `f79254e3` format, sealed box, four verdicts — 42 tests, 23 mutations
* `70fd7384` targets: local, NAS with the mount checked, R2 brokered — 79 tests, 20 mutations
* `025d2a31` keys, config, session — 129 tests, 35 mutations
* `5aa7d58c` `apex backup`, the R2 call sequence, `tests/test-apex-backup.sh`

**Counts on the branch tip:** `apex-backup-core` 132 passed / 0 failed;
`apex-secretd` 183 + 15 passed / 0 failed; `apex` 535 + 8 + 6 passed / 0 failed;
`tests/test-apex-backup.sh` 64 passed / 0 failed (including the real-root half);
`tests/test-apex-verbs.sh` 59 passed / 0 failed. `cargo clippy --locked
--workspace --all-targets -- -D warnings` clean.

**85 mutations run. 75 caught on the first pass; 10 survived, of which 6 were
real gaps now closed and 4 were the tests being weaker than they looked.** The
two worth naming:

* **`restore` never checked the manifest's offsets.** Zeroing every offset left
  the entire round trip green, because restore reads sequentially and only
  `verify` looked at the field — so a manifest could disagree with itself about
  where each file starts and a restore would hand back a tree that looked
  right. The path that writes files checks it now.
* **a `valid_name` call in `SnapshotId::parse` could be deleted with nothing
  going red**, because the shape check subsumes it. A check that cannot refuse
  anything is the dead cosign branch again; it is gone and the coupling is
  asserted directly.

**One defect the shell suite found, bigger than its symptom:** a second restore
into the same directory failed `EEXIST` on a symlink. The dangerous half was
that a symlink already at an entry's path is something `File::create` FOLLOWS —
a destination holding `etc/passwd -> /etc/passwd` would have had a root restore
write outside the destination. Now cleared first, `O_NOFOLLOW` on the write as
well, and a directory in the way is reported rather than deleted.

**One mutation deliberately left surviving,** recorded in the source: the
per-entry `readdir` `?` in `FsTarget::list` cannot be reached from a fixture,
because `read_dir` opens the directory once and the `chmod 000` test fails at
the open. It needs an NFS mount losing its server mid-listing.
