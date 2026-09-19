# android-release — publish the Android client, and stop it drifting from the OS

ask: Andre, directly — *"add the android app to releases"*, then sharpened to
  *"maybe make the apk auto updating"* and *"not some suckky system where half
  the time peoples things wont work."*
repo: apex-os
worktree: `/var/tmp/apex-work/wt-android-release`, branch `task/android-release`
  (created 2026-09-20 off `roadmap/v2.2` @ `303221d5`; **pushed, not merged**)
owns: `android/**` and the new release workflow. Does NOT own `apexd/**`,
  `installer/**`, `Containerfile.*`, the kernel.

## STATUS — IN PROGRESS

Done and pushed:

* `a7d935ea` — `versionCode` comes from `APEX_VERSION_CODE` instead of the
  literal `1`. Verified by building the debug APK both ways and reading
  `output-metadata.json`.

## THE CORRECTION THAT MATTERS MOST

**The brief said the Android wire is at `PROTOCOL_VERSION` 11. It is not.**
Two different constants, and confusing them sends you to the wrong file:

* `apexd/apex-remote-core/src/lib.rs:55` — **`REMOTE_PROTOCOL_VERSION = 1`**.
  This is the Android ↔ `apex-remoted` wire. Mirrored at
  `android/core/src/main/kotlin/com/apexos/remote/core/Client.kt:300`.
* `apexd/apex-agent-core/src/protocol.rs:169` — `PROTOCOL_VERSION = 11`. This
  travels *inside* the Noise tunnel and Android already handles it leniently
  (`ignoreUnknownKeys = true`, `Sessions.kt:222`) and never compares it. It is
  not a compatibility problem and needs nothing.

## THE SHIPPED DEFECT THIS UNIT FOUND

A version mismatch on an **already-paired** machine is reported to the user as
*"this machine does not accept this device: it is not paired, or it has been
revoked."* — `Client.kt:108-125`. The version is never mentioned.

Why: the version is bound into the Noise prologue
(`apex-remote-core/src/noise.rs:129`), never transmitted. The daemon
(`apex-remoted/src/serve.rs:180-189`) **never inspects a version** — it builds
the responder with its own constant, the AEAD tag fails, and it drops the
socket with one journald line. Pairing is fine (`Client.kt:37-43` checks
`offer.v` explicitly and says the right thing); reconnecting is not.

So bumping `REMOTE_PROTOCOL_VERSION` today tells every installed phone it has
been revoked. That is exactly the failure Andre described.

## NEXT

1. **Protocol version window in `android/core`.** Retry the handshake across a
   supported list instead of a single constant — the prologue is hashed, never
   sent, so a client can simply try each version. `openSession` takes
   `versions: List<Int>`; `Client.pair` accepts `offer.v in versions` rather
   than `!=`. When the whole window fails, name BOTH causes and print the
   window. Test with a synthetic `{1,2}` window against
   `ClientTranscriptTest`'s fake server — the production list is `{1}` and a
   test on it proves nothing. Bump the 421 floor in `pr-validation.yml`.
2. **Release flow.** `android/tools/release-version.sh` (gate) +
   `release-artifacts.sh` (build), `.github/workflows/release-android.yml`,
   `tests/test-android-release.sh` with a fake `gh`. Follow the **Secure Boot**
   ritual (`build-image.yml:501-573`), not `pr-validation.yml`'s: `$RUNNER_TEMP`,
   `chmod 600`, validate with `keytool -list` before the build, `shred`.
3. **In-app auto-update.** Traps found but not yet paid: AGP 9 defaults
   `buildFeatures.buildConfig` OFF (needed for `BuildConfig.VERSION_CODE`); the
   download directory must not break `android/tools/no-second-write-path.sh`;
   a FileProvider is a new manifest element beside `ManifestTest`'s FIXED
   permission set, which will go red when `REQUEST_INSTALL_PACKAGES` is added.
4. Docs page + the Releases page body.

## THE DECISION ANDRE MUST MAKE — DO NOT MAKE IT FOR HIM

**No Android signing key exists.** Measured, not assumed:
`gh secret list -R AndreNijman/apex-os` returns exactly `APEX_SB_CRT_B64` and
`APEX_SB_KEY_B64`. None of `APEX_KEYSTORE_BASE64`, `APEX_KEYSTORE_PASSWORD`,
`APEX_KEY_ALIAS`, `APEX_KEY_PASSWORD` is set, so `pr-validation.yml`'s release
step has never signed anything and has only ever taken its unsigned branch.

Lose that key and **every installed app is permanently unable to upgrade** —
Android refuses an update signed by a different key, and uninstalling to fix it
destroys the paired device key. No agent should generate it.

## LOCAL BUILD TRAP

`/usr/lib/jvm/java-21-openjdk/conf` is a **dangling symlink** into a missing
`/etc/java`, so every gradle invocation dies with
`InternalError: Error loading java.security file`. It is owned by no rpm.
Working JDK 21 unpacked at `/var/tmp/apex-android-release-jdk/jdk-21.0.12.1+1`.

```
export JAVA_HOME=/var/tmp/apex-android-release-jdk/jdk-21.0.12.1+1
export ANDROID_HOME=/var/tmp/android-sdk
cd /var/tmp/apex-work/wt-android-release/android && ./gradlew --no-daemon :core:test
```
