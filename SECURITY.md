# Security policy

## Reporting a vulnerability

Report security problems privately through GitHub's private vulnerability
reporting: [open a report](https://github.com/AndreNijman/rime-os/security/advisories/new).
Please don't open a public issue, pull request or discussion with exploit
details.

Problems in the desktop itself (the bar, lock screen, notifications and the
other Rime Shell surfaces) go to
[rime-shell](https://github.com/AndreNijman/rime-shell/security/advisories/new)
instead. If you are not sure which repository a problem belongs to, report it
here.

A useful report says:

- which part is affected (the image, `rimed` and the `rime` CLI, the agent
  runtime and its sandbox, the credential broker, the installer, Rime Remote or
  its relay);
- the image you saw it on: the output of `bootc status`, which names the digest;
- how to reproduce it, and what an attacker gains.

Reports go straight to the maintainer. One person maintains Rime, so there is
no guaranteed response time, but every report is read. Once a fix
ships in a published image, the advisory is published with credit to the
reporter unless they ask otherwise.

## Supported versions

Rime OS is one rolling image. Only the newest published image receives fixes.
Every tag a machine can track (`rime`, `apex`, `daily`, `edge`, `gaming-mesa`
and `gaming-nvidia`) resolves to that one digest, under
`ghcr.io/andrenijman/rime-os` and, during the rename from APEX-OS, the old name
`ghcr.io/andrenijman/apex-os` as well. A fix reaches a machine through
`sudo rime update` and a reboot. Older images, including the rollback
deployment a machine keeps, are not patched.

An installer ISO installs the image it was built with, so a new install should
run `sudo rime update` straight away. For Rime Remote on Android, only the
newest `android-v*` release is supported.

## Scope

In scope: anything built from this repository, including the image and its
build and signing pipeline, the Secure Boot chain, `rime update`'s signature
gate, `rimed`, the agent runtime and its sandbox, `rime-secretd`, the
installer, the Rime Remote app and the relay it connects through.

Some limits are already documented, so a report that only restates them is not
a new finding:

- The agent sandbox is built for a cooperating but fallible agent, not a
  determined kernel attacker. An escape from it is still in scope.
- The kernel does not enforce module signatures and is not locked down under
  Secure Boot.
- `rime update` does not check the Rekor transparency log, and a `bootc
  upgrade` run by hand skips Rime's signature check.

[rimeos.com/security](https://rimeos.com/security) describes what is verified
and where the gaps are.

Bugs in upstream projects (the Linux kernel, CachyOS's patches, Fedora
packages) belong with those projects. If Rime's pinned version or build makes
one worse, report that here.
