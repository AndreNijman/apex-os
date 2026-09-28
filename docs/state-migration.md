# Persistent state across an update and a rollback

Roadmap §25: the state `bootc rollback` does not undo, and how APEX handles it.

## The sequence that breaks a machine

An update is atomic and a rollback is one command, so you would expect both to
be reversible. They are, for `/usr`. Nothing else rolls back, by design: `/etc`,
`/var` and your home exist to survive a new image.

The sequence that hurts is this one:

1. You update. The new build reads a config, migrates it, writes it back.
2. Something unrelated breaks: a driver, a kernel, a regression.
3. You `sudo bootc rollback` and reboot.
4. The old build now reads a file in a shape it has never seen.

The image rolled back and the state did not. This document is about step 4.

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

The framework owns checkpoints, and every store shares them: before it migrates
a file on disk it copies the file to `<path>.pre-v<version>`, and it leaves an
existing copy alone, so a rollback-then-forward cycle cannot destroy the
original.

### `unversioned` is not "current"

No file written before the version key existed has one. The tempting rule is
"absent means the current version", and its failure stays invisible until a
second version exists. From then on, every file written before the key claims to
be current, and the new rules read it.

APEX documented two of its own schemas that way, and both are fixed:
`blueprint.toml`'s `version` and `tasks.toml`'s now say "absent means 1", the
version that existed when the key arrived. A task state record with no `schema`
key is version 0, and 0 is a real version.

## The two answers to a rollback

There is no third, and on purpose there is no down-migration. A down-migration
is the right answer only when a new schema drops information the old reader
needs, and no store does that today. When one does, the enum grows an arm and
that store declares it. Until then, down-migration code would have no caller.

### `readable-by-older`

Every migration for this store only **adds** keys. Nothing already present
changes name, type or meaning, so an older build reads the file and gets the
right answer for everything it knows about.

The rule binds the reader as well as the migration. The older build has to keep
the keys it does not recognise and write them back out. Otherwise the first save
after a rollback deletes what the newer build recorded, and since `/var` does
not roll back, nothing can restore it. `TaskState` carries a
`#[serde(flatten)] unknown` map for this. Before §25 it had none, and its
comment argued it needed none, because "there is no second writer whose fields
would be lost". On an atomic OS the second writer is a newer build of the same
program.

A test checks the claim. `migrate::additive_only` replays a store's migration
over its declared sample and asserts that every key and value present before is
present after, walking nested tables. A store that declares `readable-by-older`
and then renames a field fails that test instead of losing somebody's data after
a rollback.

The first version of that check fed every store an empty document. A migration
that renamed a key never saw the key, so the check passed a migration that broke
the claim it existed to prove. That is why `sample` is a field on the store and
not a fixture in the test: the check needs real data to work on, and only the
store knows its own oldest shape.

### `refuse-and-explain`

The older build cannot read the file, admits it, and says so in words the reader
can act on. That behaviour has to exist in code that already shipped, so you can
only arrange it in advance: a build that refuses an unknown version is one a
future migration can rely on.

A refusal has to name four things, or somebody reinstalls the machine:

- the file, because you have more than one config;
- both versions, because "incompatible" does not say which direction;
- the likely cause, which is almost always a rollback;
- the remedy: boot the newer deployment again, or move the file aside.

The blueprint's refusal named none of them. `Blueprint` is
`deny_unknown_fields`, so a file from a newer APEX failed on whichever new key
TOML reached first, and the user read `unknown field 'foo' at line 12` about a
file that is not wrong. The loader now reads the version **before** the strict
parse. That read does not depend on the document's shape, so it can answer
"this is schema 2 and I read 1" for a document the parser cannot otherwise make
sense of.

`blueprint-state.toml` was worse: it has a required `schema` field, nothing read
it, and its only caller discards parse errors. A machine that had rolled back
reported that no `apex apply` had ever run on it.

## A file somebody typed is never rewritten

`blueprint.toml` and `tasks.toml` are TOML people edit, with comments and an
order they chose. Parsing and reserialising loses both, and AGENTS.md prohibits
silently overwriting a user's edits, so each store declares who writes it:

- **human**: migrated in memory on every read. The file on disk changes only
  when the user next saves it. `migrate_file` refuses these outright.
- **machine**: migrated on disk, behind a checkpoint.

Deferring the write also helps a rollback, at no cost: the older build can still
read a file nobody rewrote.

## Seeing it

```bash
apex schema status          # every store, its version, what a rollback does to it
apex schema migrate         # a dry run: what would change, and to what
apex schema migrate --commit
```

`status` opens each file read-only and closes it. It needs no daemon, no root
and no subprocess, so you can run it while you decide whether to roll back,
which is when its answer matters.

`migrate` changes nothing without `--commit`. It reports a file written by a
newer APEX and leaves it exactly as it is: overwriting it would destroy the only
copy of what that build recorded.

## The stores this does not cover

`apex schema status` ends with a list of them. The list lives in the product as
well as in this document, because a report that left out the secret broker's
grant table would read as "everything is covered".

| store | why not |
|---|---|
| `~/.local/state/apex/agent/sessions/<id>.json` | `apex-agent-core` does not depend on `apexd-core` |
| `~/.local/state/apex/agent/grants.json` | same |
| `~/.local/state/apex/agent/privilege-audit.jsonl` | append-only JSONL; a line carries its own shape, so a document version does not fit |
| `/var/lib/apex-secretd/users/<uid>/` | root-owned, and `apex-secret-core` does not depend on `apexd-core` |
| `/var/lib/apex/pkg/state.json` | written by `apex-pkg` in shell, which carries its own `pkg_compat_level` |

The framework lives in `apexd-core`, the type library the CLI and the daemon
share. The agent runtime and the secret broker sit beside it on purpose instead
of depending on it; `apexd/apex-aid/Cargo.toml` says why for its own case.
Wiring those stores in means either a new dependency edge or moving the
framework into a crate below all three, and somebody has to make that decision
on purpose.

Two of them matter for security. A grant table read by the wrong schema is a
privilege decision made on a misreading. Both grant tables refuse nothing and
version nothing today. Neither drops a key it does not recognise
(`request::Grants` and `store::Grants` are plain maps), so the risk they carry
is a change in what a key **means**.

## The shape that needs no framework at all

`ROADMAP/state/agents/<slug>.md` is persistent state with the same requirement:
it has to survive an interruption the writing process did not choose. It has no
schema version and needs none, because it is prose with headings. A reader that
does not recognise a section skips it; a human reads it either way.

That marks the boundary. A store needs a version when its format is a program's
private structure, because the program on the other side of a rollback is a
different program. When a person can read the format, the cheaper answer is to
keep it readable.
