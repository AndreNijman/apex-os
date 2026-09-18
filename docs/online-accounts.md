# Online accounts

Roadmap P2-017. This is the reference for `apex account` — what an APEX online
account *is*, where the credential lives, what an app gets instead of the
credential, and what happens when an account is removed.

`docs/agent-runtime.md` covers `apex secret`, which is the same store seen from
the agent side. Read that first if you want the broker; this document is the
cloud-identity surface on top of it.

## An account is a credential in the store that already exists

There is no accounts daemon and no accounts database. An online account is an
entry in `apex-secretd`'s store, under a reserved name:

```
/var/lib/apex-secretd/users/<uid>/account.nextcloud.home.json     0600 root
/var/lib/apex-secretd/users/<uid>/account.nextcloud.home.secret   0600 root
```

Root-owned, `0700` directory, outside `$HOME`. That is P0-002's boundary and it
is the whole of P2-017's first criterion — "without spraying credentials across
user config". A process running as you cannot read the app password at rest,
and no verb on the socket returns one.

This is worth stating as a decision rather than an implementation detail,
because the obvious alternative is what every other desktop does: a small
accounts service with its own store next to the credential store. Two stores
means two identity checks, and P2-016 measured what that costs — `apex-remoted`
checked the caller for `Pair` and for nothing else, and a second account on the
same machine read the owner's device list out of `Devices` and reached the
owner's device store through `Revoke`. One store means one `SO_PEERCRED` check,
on every verb, in one file.

The name is `account.<provider>.<name>`. You type the last two:
`nextcloud.home`, `s3.backups`. The prefix is how `apex account list` shows
your accounts without showing your git token, and how `apex account rm` cannot
delete a credential it had no business naming.

## Providers

```
apex account providers

PROVIDER     NAME                 CREDENTIAL     HOST
webdav       WebDAV               app-password   yours (--host)
nextcloud    Nextcloud            app-password   yours (--host)
s3           S3 or Cloudflare R2  access-key     yours (--host)
google       Google               device-code    www.googleapis.com
microsoft    Microsoft 365        device-code    graph.microsoft.com
```

Five providers, **four transports**. Nextcloud is WebDAV: a Nextcloud account
and a bare WebDAV account differ in how you obtain the credential and in
nothing the broker does afterwards. So a provider declares the operation
namespace it routes into separately from its own id, and both route into
`webdav`.

That is not tidiness. P1-001's capability framework routes on an operation id's
first segment — `provider.class.verb`, with no operation-to-provider table
anywhere in the tree. Two spellings of the same operation would mean two
identical registry entries, and the second one would drift from the first.

A provider whose host is fixed refuses `--host`. Pinning a Google credential to
a host of your choosing would pin it somewhere Google is not, and the pin is
what stops a brokered request being aimed elsewhere later.

## Adding an account

The credential is read from **stdin**. There is no `--token` flag and there
will not be one: `/proc/<pid>/cmdline` is world-readable for as long as the
command runs, and the shell history keeps it afterwards.

```sh
printf %s "$APP_PASSWORD" | apex account add nextcloud.home \
    --host cloud.example --username me
```

`--username` is required for a provider that signs in with Basic — WebDAV and
Nextcloud. Without it APEX would send the password on its own, the server would
answer 401, and a forgotten flag would read back as a wrong password.

`apex account add` knows the endpoint path and how the credential is presented,
which is the difference between it and `apex secret add`. The same account
stored by hand would be:

```sh
printf %s "$APP_PASSWORD" | apex secret add account.nextcloud.home \
    --host cloud.example --username me --auth raw \
    --path /remote.php/dav/files/me
```

That last path is why the provider table is worth having. Nextcloud serves
files at `/remote.php/dav/files/<username>/`, so the endpoint ends in the
account's own name — and a credential stored at the bare prefix points at a
collection the server answers 404 for, on every file, which reads like a wrong
password. `apex account add` composes it. `--path` still wins, because a
deployment behind a reverse proxy can have any prefix.

Nothing is allowed yet. Storing a credential grants nothing at all.

## What an app gets, which is not the credential

A scope, per project:

```sh
apex account scopes nextcloud

SCOPE            OPERATION              EFFECT SUMMARY
files.list       webdav.file.list       read   list the files in a folder on this account
files.read       webdav.file.read       read   read a file from this account
files.write      webdav.file.write      write  write a file to this account, replacing what is there

apex account grant nextcloud.home files.read
```

`apex account revoke nextcloud.home files.read` takes it back, and takes back
only that one: the account and its credential stay.

## Renewing an OAuth account

```sh
apex secret grant account.google.work.refresh oauth.token.refresh
apex account refresh google.work
```

Two lines, and the first one is not a formality. A refresh **spends** a stored
credential, so it is a capability like every other: granted per project, and
adding the account does not grant it. Signing in stores a credential; deciding
which project may renew with it is a second decision and it is yours. The
daemon performs the request and replaces both tokens — `apex account refresh`
never sees either.

An account whose flow yields nothing to renew (an app password, an access key)
is refused here by name, with `apex account add` as the answer, rather than
being sent to the daemon to fail.

`files.read` is what you type; `webdav.file.read` is what is enforced. The
grant is keyed `service:capability` for one project root, and the daemon
performs the operation itself — the credential does not leave
`/var/lib/apex-secretd`, and the reply carries the operation's output. The
`Response` type has no variant that can hold a credential and `SecretValue` has
no `Serialize` impl, so that is a compile error rather than a rule.

Grants here are always per project. `apex secret grant --everywhere` exists for
an operation whose provider declares it reaches the same thing wherever it is
asked; a file operation resolves a path the caller supplies, so it does not
qualify.

## Removing an account

```sh
apex account rm nextcloud.home
```

The credential is shredded and **every grant that named it goes with it**. That
is P2-017's third criterion, and it is `Request::Remove`'s existing behaviour
rather than something this layer adds — which is the argument for one store
stated a second way.

Removing an account does **not** revoke anything at the provider. An app
password you gave APEX is still valid at `cloud.example` until you delete it
there. APEX cannot do that for you for four of the five providers, and saying
otherwise would leave a live credential behind a screen that said it was gone.

## What is not built

Written here rather than in a release note, because a layer whose limits are
not stated gets trusted for things it never did.

* ~~**The device-code flow is not implemented for Google or Microsoft.**~~
  **Built, and `--client-id` is the honest part.** `apex account add
  google.work --client-id <id>` runs RFC 8628 through `apex/src/oauth_device.rs`
  — the same transport `apex cloudflare connect` uses, moved out of it and
  given its client, scopes and wording as arguments — prints a code,
  and files both halves: the access token pinned to the provider's API host,
  the refresh token under a separate name pinned to the authorisation host,
  where the endpoint pin makes it unspendable as an API token.

  The client id is **required** and there is no default, because APEX registers
  no OAuth application at either provider and will not sign your account in
  under another project's — that would put their credential in this binary and
  their name on the consent screen you approve. Register a limited-input-device
  client of your own and pass its id. Google issues a client secret with it and
  wants it on the poll; that comes from **stdin**, never argv, and there is no
  `--client-secret` flag for the same reason there is no `--password` one.

  **Every provider's token can now be spent.** `apex-secretd` ships a `gdrive`
  provider and an `msgraph` one, so `apex account grant google.<name>
  files.read` records a grant for `gdrive.file.read`, `apex account grant
  microsoft.<name> files.read` records one for `msgraph.file.read`, and the
  daemon performs both; `add` prints the grant command for each scope the
  provider's table has. Microsoft was the last provider whose token could be
  stored, refreshed forever and spent on nothing.

  The two route into **different** transports off the same scope name, which is
  what a copied table entry would silently get wrong, so it is asserted rather
  than described.
* ~~**Nothing refreshes a token.**~~ **Built for Cloudflare only, and the
  "only" is the honest part.** An `oauth` provider offers
  `oauth.token.refresh` — RFC 6749 §6, performed in the daemon — and
  `apex cloudflare refresh` spends it. Where the request goes is decided by
  the host the refresh token was **pinned to** when it was stored: the
  operation declares no resource and no parameters, so no caller can aim one
  somewhere else.

  Google and Microsoft used to be refused with the reason: §6 requires a
  refresh to present the same OAuth client the grant was issued to, and the
  client a person signed in with was recorded nowhere. The way out was always
  a change to the *grant* rather than to the refresher, and that is what
  landed — `apex account add` writes the client id into the store's non-secret
  half beside the refresh token, which is the field the provider already read
  first. `apex account refresh <account>` spends it.

  Microsoft should renew on that alone: its table entry is
  `ClientSecret::None`, a public client. **Google is the one to be careful
  about.** Its guide lists `client_secret` as required on the poll and optional
  on the refresh; this build stores no client secret, so if a renewal turns out
  to want one it answers `invalid_client`, nothing is replaced, and signing in
  again is the way through. `apex account add` says that at the time rather
  than leaving it to be discovered, and neither case has been run against the
  real Google — there is no account here.

  Two limits worth stating. A reply that rotates no refresh token leaves the
  stored one alone, because overwriting a still-good credential is the failure
  a refresh exists to prevent. And a refresh is a capability like any other:
  it is granted per project, and connecting does not grant it. `apex cf
  status` asks the daemon which of those two it is and prints the grant line
  when it is missing, rather than naming `apex cf refresh` whenever a refresh
  token exists — on every machine that had connected, that was a command about
  to be refused for a reason the line did not mention.
* **There is no file-manager integration.** APEX ships gvfs with its WebDAV,
  SMB and NFS backends, and `apex devices share` reports which of them are
  present, but no account here mounts anything and nothing hands GTK a
  credential. A `gvfsd-dav` mount wants the password in the session; that is
  the file this layer exists to empty, and bridging it honestly needs a
  short-lived credential the broker mints per mount, not a copy of the stored
  one.
* ~~**S3 is a name here and not yet a signer.**~~ **Built.** `5054be77` landed
  an `s3` provider with a SigV4 signer whose test vectors were generated by
  botocore rather than written from the specification, so `objects.read` and
  `objects.write` are real. The bullet is kept struck through rather than
  deleted because it named the wall P1-011's temporary-credential work hit from
  the other side, and that is worth being able to find.

  One correction went with it: this table used to offer a third S3 scope,
  `objects.list` → `s3.object.list`. There is no such operation. The provider
  renders a listing through `s3.object.read` when the resource names a bucket
  with no key, the same one-operation-for-both shape `cloudflare.r2.object.read`
  has, so there is one scope and its summary says both things it does.

* **Google and Microsoft have one grantable scope each, and no provider now
  has none.** `files.read` → `gdrive.file.read` for Google, performed by the
  `gdrive` provider in `apex-secretd`; `files.read` → `msgraph.file.read` for
  Microsoft, performed by the `msgraph` one.

  This bullet exists because the table used to claim more than it could do.
  `apex account grant google files.read` was accepted and recorded a grant for
  `gdrive.file.read` **when no build had ever offered that operation**: a
  permission the user was told they had, which fails later and somewhere else.
  The table was emptied, and each `files.read` came back only when its
  operation did. It is checked rather than described —
  `every_account_scope_names_an_operation_some_provider_actually_offers` in
  `apex-secretd` is the only place the scope table and the shipped registry are
  both visible, and it fails the build if a scope names an operation no
  provider serves.

  The "a provider can be stored and refreshed and spent on nothing" case has
  **no shipped example left**, and the refusal that covers it
  (`AccountError::NoScopesYet`, and the sentence `apex account add` prints) was
  kept rather than deleted, held to a provider the tests construct. The set of
  providers with an empty scope table is asserted EMPTY in both crates — the
  positive claim — because an equality against an empty list, with a loop over
  it, proves nothing.

  **What one Microsoft scope buys, and the one thing it cannot address.**
  `Files.Read` is Microsoft's own least-privileged permission for
  `driveItem: content`, and unlike Google's `drive.file` it really does read
  the account's own OneDrive — the narrowing is APEX's, not Microsoft's: there
  is one operation because one was written. `msgraph.file.read` takes a
  driveItem id, and a **consumer** OneDrive id is `{driveId}!{n}` (Microsoft's
  own documented example is `12319191!11919`), which the shared resource
  vocabulary refuses because it does not accept `!`. Work and school ids
  (`01BYE5RZ…`) are unaffected. The vocabulary was not widened for one
  provider; lifting this needs a resource kind that percent-encodes.

  Graph's `/content` answers `302` with a **pre-authenticated** download URL on
  another host, which APEX follows exactly once, with no credential on the
  second request and no `Location` chain, and that URL comes back in nothing —
  no reply, no error, no audit line. It is a bearer capability for as long as
  it lives. See the module note on `providers/msgraph.rs` for the whole
  decision.

  **What one scope buys, said plainly, because it is less than it sounds.**
  Google's limited-input-device grant accepts only `email`, `openid`,
  `profile`, `drive.appdata`, `drive.file`, `youtube` and `youtube.readonly` —
  full `drive` and `drive.readonly` are not on the list, and `drive.file` sees
  only files the OAuth client itself created or the user individually picked.
  So "list the files in this Drive" is not something a device-code Google
  account can do at all, which is why there is no `files.list`; and on a Drive
  APEX has never written to, **every file id answers 404**. That is the scope
  working. The scope that makes the first file readable is `files.write`, and
  it needs a transport before it needs a table entry: a Drive upload goes to
  `/upload/drive/v3/files`, a different path from the `/drive/v3` an account is
  stored with.

  A token's scopes are fixed when it is issued and a refresh cannot widen them,
  so an account signed in before `drive.file` was asked for holds a token
  without it. Its reads answer 403 until `apex account add google.<name>` is
  run again.
