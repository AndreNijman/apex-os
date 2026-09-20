#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  release-notes.sh — the Releases page a person actually reads.
#
#  Written for someone who has never sideloaded an APK and does not know what
#  a versionCode is. Everything a machine needs is in the `.json` beside the
#  APK; this file is for the human.
#
#  It is a script rather than a static file because three things in it must be
#  true of THIS build — the version, the checksum and the protocol revision —
#  and a hand-edited page that drifts from the artefact beside it is worse than
#  no page, because it is the page people check the checksum against.
#
#  Usage: android/tools/release-notes.sh --code N --name STRING --dir DIR
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

code=""; name=""; dir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --code) code="${2:?--code needs a value}"; shift 2 ;;
        --name) name="${2:?--name needs a value}"; shift 2 ;;
        --dir)  dir="${2:?--dir needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) echo "FATAL: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

fatal() { echo "FATAL: $*" >&2; exit 1; }
[ -n "$code" ] || fatal "--code is required"
[ -n "$name" ] || fatal "--name is required"
[ -n "$dir" ]  || fatal "--dir is required"
[ -d "$dir" ]  || fatal "$dir is not a directory"

apk="apex-remote-$name.apk"
[ -f "$dir/$apk" ] || fatal "$dir/$apk is missing; notes must not describe an artefact that was not built"

sums=$(cat "$dir/$apk.sha256" 2>/dev/null) || fatal "$dir/$apk.sha256 is missing"
digest="${sums%% *}"
[ -n "$digest" ] || fatal "the checksum file is empty"

meta="$dir/apex-remote-$name.json"
[ -f "$meta" ] || fatal "$meta is missing"
protocol=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["remoteProtocolPreferred"])' "$meta") \
    || fatal "could not read the protocol revision out of $meta"

# -- The window, and a page that only claims what the build can do -----------
#
# The compatibility sentence below is the one people will quote back when
# something does not connect, so it is generated from the window this build
# actually ships rather than written once and left to rot. A build that speaks
# a single revision gets a sentence that says so; a build that speaks several
# gets the range. Saying "an app one revision behind still connects" on a build
# whose window is one entry long would be a promise nobody could keep -- this
# repository's dominant defect class, in prose instead of in code.
window=$(python3 -c '
import json, sys
w = json.load(open(sys.argv[1])).get("remoteProtocolSupported") or []
print(",".join(str(int(v)) for v in w))
' "$meta") || fatal "could not read the protocol window out of $meta"
[ -n "$window" ] \
    || fatal "$meta carries no remoteProtocolSupported, so this page cannot state what it is compatible with. Rebuild with a release-artifacts.sh that emits it rather than publishing a page that implies a promise."
window_lo=$(printf '%s' "$window" | tr ',' '\n' | sort -n | head -1)
window_hi=$(printf '%s' "$window" | tr ',' '\n' | sort -n | tail -1)
if [ "$window_lo" = "$window_hi" ]; then
    compat="This build speaks exactly one revision of that protocol, **v$window_hi**, and no
other. Today that is the only revision there is, so every APEX machine speaks
it. When a second one appears, a later build of this app will speak both and
try each in turn -- which is why, when it cannot connect, it tells you the
version may be the problem instead of claiming your phone has been unpaired."
else
    compat="They do not have to match exactly. This build speaks **v$window_lo to v$window_hi**
and tries each in turn, so an app that has run ahead of your machine still
connects to it. When none of them work it says so in those words and names the
side to update, rather than claiming your phone has been unpaired."
fi

size_mb=$(python3 -c 'import os,sys; print(f"{os.path.getsize(sys.argv[1])/1048576:.0f}")' "$dir/$apk") \
    || fatal "could not size the APK"

# A quoted heredoc. Nothing in the prose below is expanded by the shell, which
# matters because it is full of backticks and dollar signs that would otherwise
# be command substitutions and variables.
cat <<'HEADER'
**APEX Remote** is the phone half of APEX-OS. It pairs with one of your own
machines over your network — or through a relay when you are away from it — and
lets you watch and drive what is running on it: agent sessions, approvals, and a
real terminal.

It talks only to machines you have paired it with by scanning a QR code off
their screen. There is no account, no sign-in, and no server of ours in the
middle holding your data.

## What you need

* A machine running APEX-OS with the remote service switched on. On that
  machine, run `apex remote status` — if it prints a protocol version, you are
  ready. If it says the service is not running, `apex remote enable` starts it.
* A phone running Android 9 (API 28) or newer.

## Installing it, if you have never sideloaded an app

Android will not install an app from a file until you tell it that the app you
are installing *from* is allowed to do that. This is a good default and it only
takes a moment to get past.

1. Download the `.apk` below on the phone. Your browser will warn you that this
   kind of file can harm your device — that warning is shown for every APK and
   is not about this one.
2. Open the downloaded file. Android will say the browser (or your Files app) is
   not allowed to install unknown apps, and offer a **Settings** button.
3. Turn on **Allow from this source**, then come back and press **Install**.
4. You can turn that permission off again afterwards. It is remembered per app,
   not globally, so leaving it on for your browser is the only thing it affects.

On GrapheneOS the flow is identical; nothing special is required.

## Check what you downloaded

Worth doing once, on the machine rather than the phone:

```
sha256sum -c apex-remote-VERSION.apk.sha256
```

HEADER

cat <<EOF
The checksum for this build is:

\`\`\`
$digest
\`\`\`

There is also a \`.sig\` and a \`.pem\` beside the APK. Those are a Sigstore
signature made by the workflow that built it, the same way this project signs
its OS images. They are for a desktop that wants to prove where the file came
from; you do not need them to install the app.

## Pairing

On the machine:

\`\`\`
apex remote pair
\`\`\`

It shows a QR code that is good for three minutes. In the app, press **Pair a
machine** and point the camera at it. If the camera is awkward, the same screen
takes the pairing text pasted in.

The key the app generates during pairing lives in the phone's hardware keystore
and is unlocked by your fingerprint or PIN. It never leaves the phone.

> **Do not uninstall the app to "reinstall it fresh".** Uninstalling destroys
> that key, and the machine will no longer recognise the phone — you will have
> to pair again from the machine. Updating over the top, which is what the
> update flow below does, keeps it.

## Versions, and what happens when they drift

The app and your machine are updated by different people at different times, so
they will not always be the same age. Your machine prints the protocol version
it speaks in \`apex remote status\`.

$compat

## Updating

The app checks this page for a newer build and offers to install it. Android
still shows you the normal install prompt; nothing is replaced silently. If the
phone is offline, or GitHub is unreachable, the app carries on working with what
it has and does not nag you about it.

To update by hand, download the newer APK and open it — Android installs it over
the top and your paired machines are kept.

| | |
|---|---|
| Version | \`$name\` |
| versionCode | \`$code\` |
| Remote protocol | \`v$protocol\` (this build speaks \`$window\`) |
| Size | ~${size_mb} MB |
| Minimum Android | 9.0 (API 28) |
EOF
