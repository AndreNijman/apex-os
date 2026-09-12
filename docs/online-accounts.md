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

Five providers, **three transports**. Nextcloud is WebDAV: a Nextcloud account
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

* **The device-code flow is not implemented for Google or Microsoft.** APEX has
  a complete RFC 8628 device-authorization implementation already — `apex
  cloudflare connect` uses it, and both its tokens land in this same store,
  with the refresh token under a *separate* service pinned to a different host
  so the endpoint pin makes it unspendable as an API token. That is the shape
  P2-017's OAuth half should take, and it is not wired to these two providers
  yet. Until it is, a Google or Microsoft account can be added only by pasting
  an access token you obtained elsewhere, and `apex account add` says so before
  it reads stdin.
* **Nothing refreshes a token.** Not for these providers and not for
  Cloudflare's either; `apex cloudflare connect` has the same gap on record.
  The provider table carries `refreshable` as a value the code reads, so the
  day a refresher exists it does not have to be taught which providers have
  one.
* **There is no file-manager integration.** APEX ships gvfs with its WebDAV,
  SMB and NFS backends, and `apex devices share` reports which of them are
  present, but no account here mounts anything and nothing hands GTK a
  credential. A `gvfsd-dav` mount wants the password in the session; that is
  the file this layer exists to empty, and bridging it honestly needs a
  short-lived credential the broker mints per mount, not a copy of the stored
  one.
* **S3 is a name here and not yet a signer.** `sigv4` is what the provider
  table records; the broker's HTTP path sends `Authorization:` headers it is
  given and has no request signer, which is the same wall P1-011's temporary-
  credential work hit with R2's `temp-access-credentials`.
