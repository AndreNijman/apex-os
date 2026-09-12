# Local inference — `apex ai`

§14's model service: one endpoint every application and agent client on this
machine can use, with APEX owning the model store, the backend choice, how much
fits in VRAM, and when an idle model gives its VRAM back.

Seven verbs. Two of them need root and the reason is the store, not the
service.

## Per-user service, shared weights

`apex-aid` is a **per-user** unit, like the agent runtime, and for a stronger
reason: it turns your prompts into generated text, so a privileged daemon
shared between accounts would be one process holding every account's
conversations with no way to tell them apart. Your sockets live in your own
`$XDG_RUNTIME_DIR` at mode `0600` inside a `0700` directory, and who is on the
other end is decided from `SO_PEERCRED`.

```
systemctl --user enable --now apex-aid
```

What *is* shared is the expensive part — the weights:

```
/var/lib/apex/ai/
  models/blobs/sha256-<64 hex>   0444 root:root   the weights
  models/manifests/<id>.json     0444 root:root   name -> digest, and what it is
  staging/                       0700 root:root   download target
```

One copy of a multi-gigabyte file for every account, root-owned and read-only,
so the backend — which runs as *you*, loads untrusted weights and speaks a
network protocol — cannot alter a model, its own or anybody else's. That is
what makes `pull` and `rm` the two root verbs while everything else is yours.

**There is no TCP port, and no flag adds one.** A TCP connection carries no
peer credential (`SO_PEERCRED` is a Unix-socket thing), so a listener on
127.0.0.1 is reachable by every account on the machine and by every sandboxed
application holding the network permission.

APEX also ships **no inference runtime**. llama.cpp with CUDA is gigabytes and
`Containerfile.core` is the tier whose rebuild the whole fleet downloads, so
the runtime is installed on demand and `apex ai status` names the command that
provides one.

## `apex ai models`

What is in the store. It reads the store directly, so it answers with the
service stopped — "what have I got" must never require a running daemon.

`--available` prints the image's curated catalogue instead: names, sizes,
licences and digests, so a licence can be read *before* several gigabytes are
downloaded.

It deliberately does not search a remote index. The catalogue ships inside the
signed image — cosign-signed, digest-pinned in CI, rolled back with the OS —
and that is the whole reason its name-to-digest mapping is worth trusting.
Fetching the mapping from the same place as the weights would make the digest
check prove nothing.

If the store cannot be read, it says installed models are **unknown**, not
absent. A failed read is not evidence of an empty store.

## `apex ai pull`

Downloads a model into the shared store and verifies it. Needs root, because
the store is root-owned.

```
sudo apex ai pull qwen25-coder
sudo apex ai pull qwen25-coder@sha256:cc324af0…
```

Three provenance cases, and only three:

| what you typed | where the name-to-digest mapping comes from |
|---|---|
| a catalogue name | the signed image |
| `--url` with `--digest` | you, explicitly |
| `--url` alone | **refused** |

`name@sha256:<hex>` is not a fourth case. The mapping still comes from the
image; the suffix asserts *what you expected it to be*, and a digest that does
not match the catalogue's is refused with both values rather than pulled.

A URL with no digest is refused rather than trusted, because verifying a
download against a digest handed to you by the same server proves only that it
sent the same bytes twice. There is no trust-on-first-use path for the same
reason: pinning whatever the first download happened to be would make every
later check pass while proving only that the file had not changed since a
moment nobody was watching.

The blob is verified **in staging and then renamed** into place. A rename
within a filesystem is atomic, so a partial or wrong-digest download is never
visible under its final content-addressed name — the failure that would
otherwise leave a corrupt file looking like a cached model forever.

Nothing is downloaded twice: the store is content-addressed, so a model whose
blob is already present has its manifest recorded and the transfer skipped.
`--dry-run` prints the URL, digest, size and the three paths it would write,
and performs no network access and no writes at all.

## `apex ai rm`

Removes a model. Root, for the same reason `pull` is.

The weights are deleted only when no other manifest names the same blob — not
a special case, but the normal consequence of content addressing, since pinning
a digest twice under two names is a thing people do. A manifest that is removed
while its blob cannot be says so rather than claiming the space back.

## `apex ai run`

Generates, streaming tokens as they arrive.

```
apex ai run "explain this backtrace"
git diff | apex ai run "review this"
```

The prompt is everything after the verb. Anything piped in is appended as
context, which is what makes the second form work.

It starts the model if it is not resident and **leaves it resident** — that is
the point of a service, so the next question does not pay the load again.

- `--explain` prints the plan — model, backend, device, layers, context — and
  generates nothing. The right first command when the answer is slow or the
  device is not the one you expected.
- `--json` returns the whole response as one object instead of streaming.
- `--model`, `--system`, `--max-tokens`, `--temperature` are the usual knobs;
  an absent `--temperature` leaves the model's own default rather than
  substituting one.
- `--on <device>` forwards the whole invocation to a trusted device's own
  service (§20, and `docs/hosts.md`). The far side selects its own backend
  against *its* hardware, which is the point of dispatching at all: a laptop
  asking a desktop to generate wants the desktop's plan, not its own. Only the
  prompt goes over, only the answer comes back — the weights stay on the
  machine that has them, and the credential is your own ssh identity.

## `apex ai status`

What the service decided, and what it would decide: backend, device, fit,
store, and the idle timeout in force.

It answers **with the daemon when it is running and without it when it is
not**, and the two answers agree, because the backend choice, the device and
the VRAM arithmetic all come from one resolver that the daemon and the CLI
share. Read-only, so it needs no root.

## `apex ai unload`

Stops the resident model and releases its VRAM now.

The service already unloads on its own timer — 300 seconds on AC, 60 on
battery. The shorter battery figure is the whole of §14's "power use" that APEX
claims honestly: a process holding VRAM keeps a discrete GPU out of its deepest
idle state, so an unused loaded model costs power for nothing. This verb exists
for the case the timer cannot serve, which is wanting the memory back *before*
starting a game or a render.

It refuses while a client is attached rather than cutting a generation off
mid-answer; the model reloads on the next request.

It deliberately does **not** stop the service. A stopped daemon would also stop
answering `apex ai status`, and "why is no model loaded" is exactly the question
you would then be unable to ask. `systemctl --user stop apex-aid` is how you
say the other thing.

## `apex ai serve`

Prints where applications should connect and a request that works, because "one
local inference API" is only usable if a program can find it. The endpoint
speaks the runtime's own OpenAI-compatible HTTP API over a Unix socket, so the
reference form is:

```
curl --unix-socket "$XDG_RUNTIME_DIR/apex-ai/api.sock" \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"hello"}]}' \
  http://localhost/v1/chat/completions
```

`--foreground` runs the service in your terminal with its log on stderr, for
debugging. `--listen` exists only to be refused, and the refusal is the
explanation.

### The limitation this verb prints rather than hides

A client whose entire configuration surface is `base_url = "http://host:port"`
has nowhere to put a socket path, and cannot reach the endpoint. That is a real
gap, and `apex ai serve` prints it along with the `socat` one-liner that closes
it and the sentence about what the one-liner costs: while that bridge is up,
every account on the machine and every sandboxed application with network
access can send prompts through your model and read the answers, because a TCP
connection carries nothing to tell them apart from you.

APEX ships no such bridge under a verb of its own. Putting it behind `apex`
would read as APEX having decided the trade was safe. It is not safe; it is a
trade, so the command and its cost are printed in the same place and you make
it yourself. The bridge APEX *does* provide is `apex ai run --on <device>`,
where the credential is your ssh identity rather than the absence of one.
