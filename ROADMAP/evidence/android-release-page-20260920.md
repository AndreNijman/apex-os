# Evidence — the APEX Remote Releases page, generated 2026-09-20

This is the page body `android/tools/release-notes.sh` produces, run against a
real artefact directory rather than written by hand. It is here so the prose can
be reviewed before a release is ever cut.

Produced with:

```
android/tools/release-notes.sh --code 1330 --name 0.1.0+1330.gac7db108 --dir <release dir>
```

The `--dir` held the debug APK under the release's name (43,502,784 bytes — a
release APK will differ), its real `.sha256`, and the metadata file shaped
exactly as `release-artifacts.sh` writes it, window included. So the checksum,
the size, the version and the protocol sentence below are all read from a real
file, which is the property that makes this reviewable at all.

**No release was created and no tag was moved.** `release-android.yml` is
`workflow_dispatch`-only and `dry_run` defaults to true; this artefact was made
by running the notes script directly.

---

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

The checksum for this build is:

```
6e6a406e11a79013710c782442305221dc206f48fe8247d91b632bd7865fbcfc
```

There is also a `.sig` and a `.pem` beside the APK. Those are a Sigstore
signature made by the workflow that built it, the same way this project signs
its OS images. They are for a desktop that wants to prove where the file came
from; you do not need them to install the app.

## Pairing

On the machine:

```
apex remote pair
```

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
it speaks in `apex remote status`.

This build speaks exactly one revision of that protocol, **v1**, and no
other — today that is the only revision there is, so every APEX machine speaks
it. When a second one appears, a later build of this app will speak both and try
each in turn. That is why, when it cannot connect, it tells you the version may
be the cause instead of claiming your phone has been unpaired.

## Updating

The app checks this page for a newer build and offers to install it. Android
still shows you the normal install prompt; nothing is replaced silently. If the
phone is offline, or GitHub is unreachable, the app carries on working with what
it has and does not nag you about it.

To update by hand, download the newer APK and open it — Android installs it over
the top and your paired machines are kept.

| | |
|---|---|
| Version | `0.1.0+1330.gac7db108` |
| versionCode | `1330` |
| Remote protocol | `v1` (this build speaks `1`) |
| Size | ~41 MB |
| Minimum Android | 9.0 (API 28) |
