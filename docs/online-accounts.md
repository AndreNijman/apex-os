# Online accounts

Roadmap P2-017. This is the reference for `rime account`: what a Rime online
account *is*, where the credential lives, what an app gets instead of the
credential, and what happens when you remove an account.

`docs/agent-runtime.md` covers `rime secret`, which is the same store seen from
the agent side. Read that first if you want the broker; this document is the
cloud-identity surface on top of it.

## An account is a credential in the store that already exists

There is no accounts daemon and no accounts database. An online account is an
entry in `rime-secretd`'s store, under a reserved name:

```
/var/lib/rime-secretd/users/<uid>/account.nextcloud.home.json     0600 root
/var/lib/rime-secretd/users/<uid>/account.nextcloud.home.secret   0600 root
```

The directory is root-owned, `0700` and outside `$HOME`. That is P0-002's
boundary, and it covers all of P2-017's first criterion, "without spraying
credentials across user config". A process running as you cannot read the app
password at rest, and no verb on the socket returns one.

This is a decision, not an implementation detail. The obvious alternative is
what other desktops do: a small accounts service with its own store next to the
credential store. Two stores mean two identity checks, and P2-016 measured what
that costs: `rime-remoted` checked the caller for `Pair` and for nothing else,
so a second account on the same machine read the owner's device list out of
`Devices` and reached the owner's device store through `Revoke`. One store
means one `SO_PEERCRED` check, on every verb, in one file.

The name is `account.<provider>.<name>`. You type the last two:
`nextcloud.home`, `s3.backups`. The prefix lets `rime account list` show your
accounts without showing your git token, and stops `rime account rm` from
deleting a credential it had no business naming.

## Providers

```
rime account providers

PROVIDER     NAME                 CREDENTIAL     HOST
webdav       WebDAV               app-password   yours (--host)
nextcloud    Nextcloud            app-password   yours (--host)
s3           S3 or Cloudflare R2  access-key     yours (--host)
google       Google               device-code    www.googleapis.com
microsoft    Microsoft 365        device-code    graph.microsoft.com
```

Five providers, **four transports**. Nextcloud is WebDAV: a Nextcloud account
and a bare WebDAV account differ in how you obtain the credential, and in
nothing the broker does afterwards. A provider therefore declares the operation
namespace it routes into separately from its own id, and both route into
`webdav`.

P1-001's capability framework routes on an operation id's first segment
(`provider.class.verb`), and the tree has no operation-to-provider table. Two
spellings of the same operation would mean two identical registry entries, and
the second would drift from the first.

A provider whose host is fixed refuses `--host`. Pinning a Google credential to
a host of your choosing would pin it somewhere Google is not, and the pin is
what stops a brokered request being aimed elsewhere later.

## Adding an account

`rime account add` reads the credential from **stdin**. It has no `--token`
flag and will not get one: `/proc/<pid>/cmdline` is world-readable for as long
as the command runs, and the shell history keeps it afterwards.

```sh
printf %s "$APP_PASSWORD" | rime account add nextcloud.home \
    --host cloud.example --username me
```

`--username` is required for a provider that signs in with Basic (WebDAV and
Nextcloud). Without it Rime would send the password alone, the server would
answer 401, and a forgotten flag would look like a wrong password.

`rime account add` knows the endpoint path and how to present the credential;
`rime secret add` does not. The same account stored by hand would be:

```sh
printf %s "$APP_PASSWORD" | rime secret add account.nextcloud.home \
    --host cloud.example --username me --auth raw \
    --path /remote.php/dav/files/me
```

That last path is why the provider table exists. Nextcloud serves files at
`/remote.php/dav/files/<username>/`, so the endpoint ends in the account's own
name. A credential stored at the bare prefix points at a collection the server
answers 404 for, on every file, which looks like a wrong password. `rime account
add` composes the path. `--path` still wins, because a deployment behind a
reverse proxy can have any prefix.

Storing a credential grants nothing. No project may use it yet.

## What an app gets, which is not the credential

A scope, per project:

```sh
rime account scopes nextcloud

SCOPE            OPERATION              EFFECT SUMMARY
files.list       webdav.file.list       read   list the files in a folder on this account
files.read       webdav.file.read       read   read a file from this account
files.write      webdav.file.write      write  write a file to this account, replacing what is there

rime account grant nextcloud.home files.read
```

`rime account revoke nextcloud.home files.read` takes it back, and takes back
only that one: the account and its credential stay.

`files.read` is what you type; the daemon enforces `webdav.file.read`. The
grant is keyed `service:capability` for one project root, and the daemon
performs the operation itself: the credential never leaves
`/var/lib/rime-secretd`, and the reply carries the operation's output. The
`Response` type has no variant that can hold a credential and `SecretValue` has
no `Serialize` impl, so leaking one would be a compile error, not a broken rule.

Grants here are always per project. `rime secret grant --everywhere` exists for
an operation whose provider declares it reaches the same thing wherever it is
asked; a file operation resolves a path the caller supplies, so it does not
qualify.

## Renewing an OAuth account

```sh
rime secret grant account.google.work.refresh oauth.token.refresh
rime account refresh google.work
```

You need both lines. A refresh **spends** a stored credential, so it is a
capability like every other: granted per project, and adding the account does
not grant it. Signing in stores a credential; deciding which project may renew
with it is a second decision, and it is yours. The daemon performs the request
and replaces both tokens; `rime account refresh` never sees either.

`rime account refresh` refuses an account whose flow yields nothing to renew (an
app password, an access key) by name, and points at `rime account add`, instead
of sending it to the daemon to fail.

## Removing an account

```sh
rime account rm nextcloud.home
```

The daemon shreds the credential, and **every grant that named it goes with
it**. That is P2-017's third criterion. `Request::Remove` already behaved this
way before this layer existed, which is the argument for one store from another
angle.

Removing an account does **not** revoke anything at the provider. An app
password you gave Rime is still valid at `cloud.example` until you delete it
there. Rime cannot do that for you for four of the five providers, and claiming
otherwise would leave a live credential behind a screen that said it was gone.

## What is not built

The limits live here and not in a release note, because a layer whose limits
nobody states gets trusted for things it never did.

* ~~**The device-code flow is not implemented for Google or Microsoft.**~~
  **Built, and it needs your own `--client-id`.** `rime account add
  google.work --client-id <id>` runs RFC 8628 through `rime/src/oauth_device.rs`
  (the transport `rime cloudflare connect` uses, moved out of it and given its
  client, scopes and wording as arguments), prints a code, and files both
  halves: the access token pinned to the provider's API host, and the refresh
  token under a separate name pinned to the authorisation host, where the
  endpoint pin makes it unspendable as an API token.

  The client id is **required** and has no default. Rime registers no OAuth
  application at either provider and will not sign your account in under
  another project's: that would put their credential in this binary and their
  name on the consent screen you approve. Register a limited-input-device
  client of your own and pass its id. Google issues a client secret with it and
  wants it on the poll; Rime reads that from **stdin**, never argv, and has no
  `--client-secret` flag for the same reason it has no `--password` one.

  **Every provider's token can now be spent.** `rime-secretd` ships a `gdrive`
  provider and an `msgraph` one, so `rime account grant google.<name>
  files.read` records a grant for `gdrive.file.read`, `rime account grant
  microsoft.<name> files.read` records one for `msgraph.file.read`, and the
  daemon performs both; `add` prints the grant command for each scope the
  provider's table has. Microsoft was the last provider whose token Rime could
  store and refresh forever but spend on nothing.

  The two route into **different** transports from the same scope name. A
  copied table entry would get that wrong without any error, so a test asserts
  it.
* ~~**Nothing refreshes a token.**~~ **Built: `rime cloudflare refresh` for
  Cloudflare, and `rime account refresh` for Google and Microsoft.** An `oauth`
  provider offers `oauth.token.refresh` (RFC 6749 §6, performed in the daemon),
  and both commands spend it. The host the refresh token was **pinned to** when
  it was stored decides where the request goes: the operation declares no
  resource and no parameters, so no caller can aim one somewhere else.

  Google and Microsoft used to be refused, for a reason: §6 requires a refresh
  to present the same OAuth client the grant was issued to, and nothing
  recorded the client a person signed in with. The fix had to change the
  *grant*, not the refresher, and that is what landed: `rime account add`
  writes the client id into the store's non-secret half beside the refresh
  token, the field the provider already read first. `rime account refresh
  <account>` spends it.

  Microsoft should renew on that alone: its table entry is
  `ClientSecret::None`, a public client. **Google needs care.** Its guide lists
  `client_secret` as required on the poll and optional on the refresh. This
  build stores no client secret, so if a renewal turns out to want one, Google
  answers `invalid_client`, nothing is replaced, and you sign in again. `rime
  account add` says so when you add the account. Neither case has been run
  against the real Google: no Google account was available to test with.

  Two limits. A reply that rotates no refresh token leaves the stored one
  alone, because overwriting a still-good credential is the failure a refresh
  exists to prevent. And a refresh is a capability like any other: it is
  granted per project, and connecting does not grant it. `rime cf status` asks
  the daemon whether that grant exists and prints the grant line when it is
  missing. It used to name `rime cf refresh` whenever a refresh token existed,
  which on every machine that had connected pointed at a command about to be
  refused for a reason the line did not mention.
* **No account transport carries a reply over 3 MiB, so none of them can move
  a large file.** `rime-secretd`'s `broker::HTTP_MAX_BYTES` is the cap. The
  broker refuses a read over it, naming curl's exit code, and returns nothing
  partial. Until round 32 it refused nothing: a file whose `Content-Length` was
  over the cap arrived as **a successful read of an empty file**, and a
  transfer cut short by a timeout or by the far end hanging up arrived as a
  successful read of a PREFIX.

  The cause was one property of curl, unrelated to any of these providers:
  **curl's `write-out` runs when a transfer ends, however it ended**, so
  `%{http_code}` printed `200` under an abort, and every transport that read
  the status off that line believed it. Six sites had the bug. Two more, `mcp`
  and `webdav`, did not, because they go through the broker's own helpers,
  which had always passed curl's exit code to the caller.

  The cap is not a chunking layer. This build cannot read a large file through
  an account, and the refusal says so instead of returning part of one.
  `rime-backup`'s R2 path is the one place that reads blobs back, and it sizes
  its chunks against this cap on purpose (see the format module's note).

* **There is no file-manager integration.** Rime ships gvfs with its WebDAV,
  SMB and NFS backends, and `rime devices share` reports which of them are
  present, but no account here mounts anything and nothing hands GTK a
  credential. A `gvfsd-dav` mount wants the password in the session, which is
  the file this layer exists to empty. Bridging the two needs a short-lived
  credential the broker mints per mount, not a copy of the stored one.
* ~~**S3 is a name here and not yet a signer.**~~ **Built.** `5054be77` landed
  an `s3` provider with a SigV4 signer whose test vectors came from botocore
  instead of being written from the specification, so `objects.read` and
  `objects.write` work. The bullet stays, struck through, because it named the
  wall P1-011's temporary-credential work hit from the other side, and a reader
  should be able to find that.

  One correction went with it: this table used to offer a third S3 scope,
  `objects.list` → `s3.object.list`. There is no such operation. The provider
  renders a listing through `s3.object.read` when the resource names a bucket
  with no key, the same one-operation-for-both shape `cloudflare.r2.object.read`
  has, so one scope covers both and its summary says so.

* **Google and Microsoft have one grantable scope each, and every provider now
  has at least one.** `files.read` → `gdrive.file.read` for Google, performed
  by the `gdrive` provider in `rime-secretd`; `files.read` →
  `msgraph.file.read` for Microsoft, performed by the `msgraph` one.

  The table used to claim more than it could do. `rime account grant google
  files.read` was accepted and recorded a grant for `gdrive.file.read` **when
  no build had ever offered that operation**: the user was told they held a
  permission that would fail later, somewhere else. The table was emptied, and
  each `files.read` came back only when its operation did. A test holds it:
  `every_account_scope_names_an_operation_some_provider_actually_offers` in
  `rime-secretd`, the only place where the scope table and the shipped registry
  are both visible, fails the build if a scope names an operation no provider
  serves.

  The case "a provider can be stored and refreshed and spent on nothing" has
  **no shipped example left**. The refusal that covers it
  (`AccountError::NoScopesYet`, and the sentence `rime account add` prints)
  stays, held to a provider the tests construct. Both crates assert that the
  set of providers with an empty scope table is EMPTY (the positive claim),
  because a loop over an empty list checks nothing.

  **What one Microsoft scope buys, and the one thing it cannot address.**
  `Files.Read` is Microsoft's own least-privileged permission for
  `driveItem: content`, and unlike Google's `drive.file` it reads the account's
  own OneDrive. The narrowing is Rime's, not Microsoft's: there is one
  operation because one was written. `msgraph.file.read` takes a driveItem id,
  and a **consumer** OneDrive id is `{driveId}!{n}` (Microsoft's own documented
  example is `12319191!11919`), which the shared resource vocabulary refuses
  because it does not accept `!`. Work and school ids (`01BYE5RZ…`) are
  unaffected. Nobody widened the vocabulary for one provider; lifting this
  needs a resource kind that percent-encodes.

  Graph's `/content` answers `302` with a **pre-authenticated** download URL on
  another host. Rime follows it once, with no credential on the second request
  and no `Location` chain, and returns that URL in nothing: no reply, no error,
  no audit line. It is a bearer capability for as long as it lives. The module
  note on `providers/msgraph.rs` records the whole decision.

  **What one Google scope buys, which is less than it sounds.** Google's
  limited-input-device grant accepts only `email`, `openid`, `profile`,
  `drive.appdata`, `drive.file`, `youtube` and `youtube.readonly`. Full `drive`
  and `drive.readonly` are not on the list, and `drive.file` sees only files
  the OAuth client itself created or the user individually picked. A
  device-code Google account therefore cannot list the files in a Drive at
  all, which is why there is no `files.list`; and on a Drive Rime has never
  written to, **every file id answers 404**. That is the scope working. The
  scope that makes the first file readable is `files.write`, and it needs a
  transport before it needs a table entry: a Drive upload goes to
  `/upload/drive/v3/files`, a different path from the `/drive/v3` an account
  is stored with.

  A token's scopes are fixed when it is issued and a refresh cannot widen them,
  so an account signed in before `drive.file` was asked for holds a token
  without it. Its reads answer 403 until you run `rime account add
  google.<name>` again.
