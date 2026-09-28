# Local inference: `apex ai`

§14's model service: one endpoint that every application and agent client on
this machine can use. APEX owns the model store, the backend choice, how much
fits in VRAM, and when an idle model gives its VRAM back.

Seven verbs. Two of them need root, because the store is root-owned; the
service itself needs none.

## Per-user service, shared weights

`apex-aid` is a **per-user** unit, like the agent runtime, and for a stronger
reason: it turns your prompts into generated text. A privileged daemon shared
between accounts would be one process holding every account's conversations
with no way to tell them apart. Your sockets live in your own
`$XDG_RUNTIME_DIR` at mode `0600` inside a `0700` directory, and the service
identifies the process on the other end from `SO_PEERCRED`.

```
systemctl --user enable --now apex-aid
```

The weights are the expensive part, and all accounts share them:

```
/var/lib/apex/ai/
  models/blobs/sha256-<64 hex>   0444 root:root   the weights
  models/manifests/<id>.json     0444 root:root   name -> digest, and what it is
  staging/                       0700 root:root   download target
```

Every account uses one copy of a multi-gigabyte file, root-owned and
read-only, so the backend (which runs as *you*, loads untrusted weights and
speaks a network protocol) cannot alter a model, its own or anybody else's.
That is why `pull` and `rm` are the two root verbs and everything else is
yours.

**There is no TCP port, and no flag adds one.** A TCP connection carries no
peer credential (`SO_PEERCRED` works only on a Unix socket), so every account on
the machine could reach a listener on 127.0.0.1, and so could every sandboxed
application holding the network permission.

APEX also ships **no inference runtime**. llama.cpp with CUDA is gigabytes, and
`Containerfile.core` is the tier whose rebuild the whole fleet downloads. You
install a runtime on demand, and `apex ai status` names the command that
provides one.

## `apex ai models`

Lists what is in the store. It reads the store itself rather than asking the
daemon, so it answers with the service stopped. Finding out what you have must
not require a running daemon.

`--available` prints the image's curated catalogue instead: names, sizes,
licences and digests, so you can read a licence *before* downloading several
gigabytes.

It does not search a remote index. The catalogue ships inside the signed image
(cosign-signed, digest-pinned in CI, rolled back with the OS), and that is why
you can trust its name-to-digest mapping. Fetching the mapping from the same
place as the weights would make the digest check prove nothing.

If it cannot read the store, it reports installed models as **unknown**. A
failed read is not evidence of an empty store.

## `apex ai pull`

Downloads a model into the shared store and verifies it. Needs root, because
root owns the store.

```
sudo apex ai pull qwen25-coder
sudo apex ai pull qwen25-coder@sha256:cc324af0…
```

Three provenance cases, and only three:

| what you typed | where the name-to-digest mapping comes from |
|---|---|
| a catalogue name | the signed image |
| `--url` with `--digest` | you, on the command line |
| `--url` alone | **refused** |

`name@sha256:<hex>` is not a fourth case. The mapping still comes from the
image, and the suffix states *what you expected it to be*. `pull` refuses a
digest that does not match the catalogue's, prints both values, and downloads
nothing.

`pull` refuses a URL with no digest. Verifying a download against a digest the
same server handed you proves only that the server sent the same bytes twice. For the
same reason there is no trust-on-first-use path: pinning whatever the first
download happened to be would make every later check pass while proving only
that the file had not changed since a moment nobody was watching.

`pull` verifies the blob **in staging and then renames it** into place. A
rename within one filesystem is atomic, so a partial or wrong-digest download
never appears under its final content-addressed name. Without that step, a
corrupt file could sit in the store looking like a cached model forever.

Nothing downloads twice. The store is content-addressed, so when a model's blob
is already present, `pull` records the manifest and skips the transfer.
`--dry-run` prints the URL, digest, size and the three paths it would write, and
performs no network access and no writes.

## `apex ai rm`

Removes a model. Needs root, for the same reason `pull` does.

`rm` deletes the weights only when no other manifest names the same blob. That
follows from content addressing, and it comes up in practice, because people
pin one digest under two names. When `rm` removes a manifest but cannot remove
its blob, it says so instead of claiming the space back.

## `apex ai run`

Generates, streaming tokens as they arrive.

```
apex ai run "explain this backtrace"
git diff | apex ai run "review this"
```

The prompt is the rest of the command line after the verb. `run` appends
anything piped in as context, which is how the second form works.

It starts the model if the model is not resident and **leaves it resident**, so
the next question does not pay the load again. That is the reason to run a
service at all.

- `--explain` prints the plan (model, backend, device, layers, context) and
  generates nothing. Run it first when the answer is slow or the device is not
  the one you expected.
- `--json` returns the whole response as one object instead of streaming.
- `--model`, `--system`, `--max-tokens` and `--temperature` are the usual knobs.
  Without `--temperature`, the model keeps its own default; `run` does not
  substitute one.
- `--on <device>` forwards the whole invocation to a trusted device's own
  service (§20, and `docs/hosts.md`). The far side selects its backend against
  *its* hardware, which is why you dispatch at all: a laptop asking a desktop to
  generate wants the desktop's plan. Only the prompt goes over and only the
  answer comes back. The weights stay on the machine that has them, and the
  credential is your own ssh identity.

## `apex ai status`

Shows what the service decided, and what it would decide: backend, device, fit,
store, and the idle timeout in force.

It answers **with the daemon when the daemon is running and without it when it
is not**, and the two answers agree. The daemon and the CLI share one resolver
for the backend choice, the device and the VRAM arithmetic. It only reads, so
it needs no root.

## `apex ai unload`

Stops the resident model and releases its VRAM now.

The service already unloads on its own timer: 300 seconds on AC, 60 on battery.
The shorter battery figure is the part of §14's "power use" that APEX can
back with a mechanism. A process holding VRAM keeps a discrete GPU out of its deepest idle
state, so a loaded model nobody is using costs power for nothing. This verb
covers the case the timer cannot: you want the memory back *before* you start a
game or a render.

It refuses while a client is attached, so it never cuts a generation off
mid-answer. The model reloads on the next request.

It does **not** stop the service. A stopped daemon would also stop answering
`apex ai status`, and then you could not ask why no model is loaded. To stop
the service, run `systemctl --user stop apex-aid`.

## `apex ai serve`

Prints where applications should connect and a request that works. A local
inference API is only usable if a program can find it. The endpoint speaks the
runtime's own OpenAI-compatible HTTP API over a Unix socket, so the reference
form is:

```
curl --unix-socket "$XDG_RUNTIME_DIR/apex-ai/api.sock" \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"hello"}]}' \
  http://localhost/v1/chat/completions
```

`--foreground` runs the service in your terminal with its log on stderr, for
debugging. `--listen` exists only to be refused, and the refusal explains why.

### The limitation this verb prints

A client whose entire configuration surface is `base_url = "http://host:port"`
has nowhere to put a socket path, so it cannot reach the endpoint.
`apex ai serve` prints that gap, the `socat` one-liner that closes it, and the
cost of the one-liner: while that bridge is up, every account on the machine and every
sandboxed application with network access can send prompts through your model
and read the answers. A TCP connection carries nothing that tells them apart
from you.

APEX ships no such bridge under a verb of its own. Putting it behind `apex`
would suggest APEX had judged the trade safe. It is a trade, so APEX prints the
command and its cost in the same place and you decide. The bridge APEX *does*
provide is `apex ai run --on <device>`, where the credential is your ssh
identity.
