# Persistent state across an update and a rollback

Roadmap §25. What `bootc rollback` does not undo, and what APEX does about it.

## The sequence that breaks a machine

An update is atomic and a rollback is one command, so people reasonably expect
both to be reversible. They are, for `/usr`. They are not for anything else, and
they are not supposed to be — the whole point of `/etc`, `/var` and your home is
that they survive the image being replaced.

So the sequence that hurts is not an update. It is:

1. You update. The new build reads a config, migrates it, writes it back.
2. Something unrelated is broken — a driver, a kernel, a regression.
3. You `sudo bootc rollback` and reboot.
4. The old build is now reading a file whose shape it has never seen.

The image rolled back. The state did not. What happens at step 4 is the whole
subject of this document.

## What each store declares

Every store APEX manages declares five things in one place,
`apexd/apexd-core/src/migrate.rs`:

| declared | what it is |
|---|---|
| `unversioned` | what a document with **no** version key is |
| `current` | the version this build writes |
| `steps` | the forward migrations, one per adjacent pair of versions |
| `rollback` | what an older build does when it meets a file this one wrote |
| `sample` | a real document at the oldest version, which the tests run against |

Checkpoints are the framework's, shared by every store: a file that is migrated
on disk is copied to `<path>.pre-v<version>` first, and an existing copy is left
alone, so a rollback-then-forward cycle cannot destroy the original.

### `unversioned` is not "current"

The version key is absent from every file written before the key existed. The
tempting rule is "absent means the current version", and it is wrong in a way
that is invisible until the second version exists — at which point every file
written before the key claims to be current and is read by the new rules.

Two of APEX's own schemas were documented that way, and both are fixed:
`blueprint.toml`'s `version` and `tasks.toml`'s now say "absent means 1", the
version that existed when the key was introduced. A task state record with no
`schema` key is version 0, which is a real version rather than a missing one.

## The two answers to a rollback

There is no third, and there is deliberately no down-migration: a down-migration
is the right answer only when a new schema drops information the old reader
needs, and no store does that today. When one does, the enum grows an arm and
that store declares it. Machinery with no caller is how a framework becomes
decoration.

### `readable-by-older`

Every migration for this store only **adds** keys. Nothing already present
changes name, type or meaning, so an older build reads the file and gets the
right answer for everything it knows about.

This binds the reader as well as the migration. The older build has to keep the
keys it does not recognise and write them back out, or the first save after a
rollback deletes what the newer build recorded — and `/var` is the half that
does not roll back, so there is nothing to restore it from. `TaskState` carries a
`#[serde(flatten)] unknown` map for exactly this. It did not before §25, and its
comment argued that it need not, because "there is no second writer whose fields
would be lost". On an atomic OS the second writer is a newer build of the same
program.

The claim is checked, not trusted. `migrate::additive_only` replays a store's
migration over its declared sample and asserts that every key and value present
before is present after, walking nested tables. A store that declares
`readable-by-older` and then renames a field fails a test here rather than
losing somebody's data after a rollback.

The first version of that check fed every store an empty document. A migration
that renamed a key never saw the key, so the check passed for a migration that
broke the very claim it was written to prove. That is why `sample` is a field on
the store rather than a fixture in the test: the property has to have something
to bite on, and the store is what knows its own oldest shape.

### `refuse-and-explain`

The older build cannot read the file, admits it, and says so in words the reader
can act on. This is a property of code that already shipped, so it can only ever
be arranged in advance — a build that refuses an unknown version is a build a
future migration can rely on.

A refusal has to name four things, or it is a machine somebody reinstalls:

- the file, because you have more than one config;
- both versions, because "incompatible" does not say which direction;
- the likely cause, which is almost always a rollback;
- the remedy — boot the newer deployment again, or move the file aside.

The blueprint's refusal did none of that, and the reason is worth recording.
`Blueprint` is `deny_unknown_fields`, so a file from a newer APEX failed on
whichever new key TOML reached first, and the user was told `unknown field
'foo' at line 12` about a file that is not wrong. The version is read **before**
the strict parse now. Reading it is shape-free, so it answers "this is schema 2
and I read 1" for a document the parser cannot otherwise make sense of.

`blueprint-state.toml` was worse: it has a required `schema` field, nothing read
it, and its only caller discards parse errors. A machine that had rolled back
was told no `apex apply` had ever run on it.

## A file somebody typed is never rewritten

`blueprint.toml` and `tasks.toml` are TOML people edit, with comments and an
order they chose. Parsing and reserialising loses both, and AGENTS.md prohibits
silently overwriting a user's edits. So a store declares who writes it:

- **human** — migrated in memory on every read. The file on disk changes only
  when the user next saves it. `migrate_file` refuses these outright.
- **machine** — migrated on disk, behind a checkpoint.

Deferring the write is also the better rollback story, for free. A file nobody
rewrote is a file the older build can still read.

## Seeing it

```bash
apex schema status          # every store, its version, what a rollback does to it
apex schema migrate         # a dry run: what would change, and to what
apex schema migrate --commit
```

`status` opens each file read-only and closes it. No daemon, no root, no
subprocess, so it is safe to run while you are deciding whether to roll back —
which is the moment it is worth reading.

`migrate` changes nothing without `--commit`. A file written by a newer APEX is
reported and left exactly as it is: overwriting it would destroy the only copy
of what that build recorded.

## The stores this does not cover

`apex schema status` ends with a list of them, and the list is in the product
rather than in this document alone, because a report that quietly omitted the
secret broker's grant table would read as "everything is covered".

| store | why not |
|---|---|
| `~/.local/state/apex/agent/sessions/<id>.json` | `apex-agent-core` does not depend on `apexd-core` |
| `~/.local/state/apex/agent/grants.json` | same |
| `~/.local/state/apex/agent/privilege-audit.jsonl` | append-only JSONL; a line carries its own shape, so a document version does not fit |
| `/var/lib/apex-secretd/users/<uid>/` | root-owned, and `apex-secret-core` does not depend on `apexd-core` |
| `/var/lib/apex/pkg/state.json` | written by `apex-pkg` in shell, which carries its own `pkg_compat_level` |

The framework lives in `apexd-core`, the type library the CLI and the daemon
share. The agent runtime and the secret broker are deliberately siblings of it,
not built on it — `apexd/apex-aid/Cargo.toml` says why for its own case. Wiring
those stores means either a new dependency edge or moving the framework into a
crate below all three. That is a decision to make on purpose.

Two of them are the security-relevant ones, so the shape of the risk is worth
stating plainly: a grant table read by the wrong schema is a privilege decision
made on a misreading. Both grant tables refuse nothing and version nothing
today. Neither drops a key it does not recognise — `request::Grants` and
`store::Grants` are plain maps — so the failure they are exposed to is a change
in what a key **means**, not the loss of one.

## The shape that needs no framework at all

`ROADMAP/state/agents/<slug>.md` is persistent state with the same requirement:
it has to survive an interruption the writing process did not choose. It has no
schema version and needs none, because it is prose with headings. A reader that
does not recognise a section skips it; a human reads it either way.

That is the honest boundary. A store needs a version when its format is a
program's private structure, and a program on the other side of a rollback is a
different program. Where the format is something a person can read, the cheaper
answer is to keep it that way.
