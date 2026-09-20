# android-release — publish the Android client, and stop it drifting from the OS

ask: Andre, directly — *"add the android app to releases"*, sharpened to
  *"figure out and make it good, not some suckky system where half the time
  peoples things wont work, also maybe make the apk auto updating. use the
  github releases make the page"*
repo: apex-os
worktree: `/var/tmp/apex-work/wt-android-release`, branch `task/android-release`
  (**pushed, not merged**; merged `origin/roadmap/v2.2` @ `4c2478fc` cleanly)
owns: `android/**`, `.github/workflows/release-android.yml`,
  `tests/test-android-release.sh`, `docs/android-app.md`, the README's phone
  section. Does NOT own `installer/**`, the boot path, the kernel.

## STATUS — COMPLETE except for one decision that is not an agent's to take

Six commits, all pushed. `a7d935ea` was the previous agent's; the rest are this
round's. **The previous agent's card lagged its worktree badly** — five whole
files and three modified ones were sitting uncommitted. They are committed now.

* `49900bc3` the inherited work: version window + the three release scripts +
  the workflow + both suites
* `c4d9c643` merge `roadmap/v2.2`
* `e5cf7a3b` three real defects in that inherited work (below)
* `73e60681` the in-app updater
* `ac7db108` the window reaches the metadata and the page
* `4ca2a20e` `docs/android-app.md`, README, the page evidence

## THE THREE DEFECTS IN THE INHERITED WORK — all found by running it

The previous agent wrote the window and never executed it. None of these is
subtle once the suite runs; all three are invisible if it does not.

1. **`Client.pair` handshook at `versions.first()`, not `offer.v`** — its own
   comment three lines above said it must use the desktop's. The revision goes
   into the Noise prologue, so pairing against an older desktop failed and told
   the user the machine *"did not prove it holds the key from the QR code"*.
2. **`openSessionAcrossVersions` caught `Throwable` and continued**, which its
   doc comment explicitly forbade. A dead socket would have cost one dial per
   revision and then been reported as a protocol problem.
3. **The suite did not fail on (1) — it HUNG.** Fourteen minutes, killed by
   hand. `PipedInputStream.read` only notices a dead writer that has already
   written, and the fake responder died before writing a byte. Both responders
   now close their sink in a `finally` and the class carries `@Timeout(30)`.

Both fixes are **mutation-checked**: reverting either fails exactly one named
test. The first mutation attempt hit the WRONG `catch` block (there are two
identical ones) and proved nothing — check what you mutated actually changed.

## WHAT THIS ROUND BUILT

* **In-app updater.** Decisions in `:core` (`core/…/update/Updates.kt`, 19
  tests), Android glue in `:app` (`app/…/update/`). Streams the APK from the
  release into a `PackageInstaller` session while hashing, and commits only if
  the digest agrees — so no FileProvider, no download directory, nothing new
  for `InsecureStorageTest` or `no-second-write-path.sh`.
* **NOT `/releases/latest`.** Measured: this repo's Releases page already holds
  `v1.0.0` and `v0.1.0`, both OS ISOs, and that endpoint answers `v1.0.0`. An
  updater built on it finds no APK and says "up to date" forever. Releases are
  picked out by their `android-v<code>` tag.
* **The window reaches the page.** `SUPPORTED_REMOTE_PROTOCOL_VERSIONS` is a
  literal so `release-artifacts.sh` can read it; it refuses anything that is
  not a plain integer list, and `release-notes.sh` refuses to write a page from
  metadata with no window. The old page promised "an app one revision behind
  still connects" on a build whose window is one entry long.
* **Floors:** core 421 → 526, app 33 → 53, both read from the results XML.
* **`tests/test-android-release.sh` was run by nothing.** Wired into
  pr-validation's android job, and the android path selector now also matches
  that suite and `release-android.yml` — a release-workflow-only change used to
  select no job that runs its gate.

## PROVED WITHOUT PUBLISHING

`release-artifacts.sh` was run end to end against a throwaway keystore in
`/var/lab-scratch` (shredded afterwards; **not** a release key). Exit 0 through
`assembleRelease`, `bundleRelease` and `verifyReleaseSigning`. Read back off the
artefacts: `aapt2 dump badging` says versionCode `1330`, the APK carries a v2/v3
signature, the AAB passes `jarsigner -verify`, and the built manifest carries
`REQUEST_INSTALL_PACKAGES`. The page generated from that build is
`ROADMAP/evidence/android-release-page-20260920.md`.

**No release was created and no tag was moved.** `release-android.yml` is
`workflow_dispatch`-only, main-only, and `dry_run` defaults to true.

## THE DECISION ANDRE MUST MAKE — STILL DO NOT MAKE IT FOR HIM

**No Android signing key exists.** `gh secret list -R AndreNijman/apex-os`
returns exactly `APEX_SB_CRT_B64` and `APEX_SB_KEY_B64`. None of the four
`APEX_KEYSTORE*`/`APEX_KEY*` secrets is set, so `release-android.yml` refuses on
its first real step and `pr-validation.yml` has only ever taken its unsigned
branch.

Sharpened this round, measured not assumed: `apksigner rotate` in build-tools
36.0.0 produces a v3 `SigningCertificateLineage`, and this app already enables
v3. So the key can be **rotated while the old one still exists**; it becomes
unrecoverable only once the old one is gone. The decision is therefore about the
BACKUP, and it has three parts — where it is generated, where the only copy
lives, and whether the passwords follow `apex-secretd` or the Secure Boot
pattern. Written up at the end of `docs/android-app.md`.

## NEXT

Nothing is half-done. In rough order of value:

1. **Andre decides the signing key** (above). Until then no release can be cut,
   which is the correct failure — an unsigned APK cannot be installed at all.
2. **Once the four secrets exist, dispatch `release-android.yml` with
   `dry_run: true`** and read the summary. That is the one path no local run can
   exercise: `$RUNNER_TEMP`, the `secrets` context, cosign keyless. Only then
   turn `dry_run` off.
3. **When `REMOTE_PROTOCOL_VERSION` next moves**, add the old revision to
   `SUPPORTED_REMOTE_PROTOCOL_VERSIONS` in the SAME commit, and check the
   daemon can still serve it. The window is a promise about whole sessions, not
   just handshakes.
4. `isMinifyEnabled` is still false. Turning R8 on needs `kotlinx.serialization`
   keep-rules and the failure is at runtime on a device — do not turn it on
   without one.
5. The in-app install path has never run on a phone, and says so in its own
   comments and in `docs/android-app.md`. First device that appears, install an
   older APK and update over it.

## LOCAL BUILD TRAPS

`/usr/lib/jvm/java-21-openjdk/conf` is a dangling symlink, so the system JDK
dies with `InternalError: Error loading java.security file`. Working JDK:

```
export JAVA_HOME=/var/tmp/apex-android-release-jdk/jdk-21.0.12.1+1
export ANDROID_HOME=/var/tmp/android-sdk
cd /var/tmp/apex-work/wt-android-release/android && ./gradlew --no-daemon :core:test
```

A cold `:core:test` is ~35s; `:app:assembleDebug` is ~3min. Espresso stays
pinned at 3.7.0.
