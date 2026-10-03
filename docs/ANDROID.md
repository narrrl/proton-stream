# Android Development

The Android client lives in `android/`; its Rust boundary is
`crates/pstr-android`. It targets Android 12 (API 31) and newer on
`arm64-v8a` phones/tablets and `x86_64` emulators. Compose owns navigation and
screens, while UniFFI exposes the existing catalog, share, streaming, watch-state,
and offline logic. Keep reusable behavior in the Rust crates rather than
reimplementing it in Kotlin.

## Toolchain

Install JDK 17, Android SDK Platform/Build Tools 36, NDK
`29.0.14206865`, Rust 1.96, Gradle 8.13, and the native helpers:

```bash
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
```

Set `ANDROID_HOME` (or `ANDROID_SDK_ROOT`); `cargo-ndk` discovers the NDK below
that SDK. The workspace-provided `uniffi-bindgen` binary keeps binding generation
on the same UniFFI version as the bridge. Use `bash scripts/build-android.sh debug` for an installable debug APK,
`bash scripts/build-android.sh check` for Android lint/unit tests, and
`bash scripts/build-android.sh release` for release APK/AAB output. Gradle first
generates UniFFI Kotlin into `android/app/build/generated/source/uniffi`, then
builds both Rust ABIs into `android/app/build/generated/jniLibs`. Generated files
belong under `build/` and must not be committed.

`versionName` always comes from `[workspace.package].version` in `Cargo.toml`, so
a tagged build cannot drift from the desktop release. The default `versionCode`
is `major * 1,000,000 + minor * 1,000 + patch` (`0.1.1` becomes `1001`). Set
`ANDROID_VERSION_CODE` only when Play requires a higher monotonic code; Gradle
rejects non-integers, zero, and values above Android's limit. The Android
workflow tests both the default derivation and the override path.

## Local Run and Verification

Start an API 31+ emulator or attach a device, then run:

```bash
bash scripts/build-android.sh debug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

Before submitting Android work, run the Rust workspace gate from the repository
root, then:

```bash
bash scripts/build-android.sh check          # ktlint, Android lint, host tests
bash scripts/android-acceptance.sh --release # the on-device matrix, minified APK
```

`check` needs no device. Run the matrix with `--release` where you can: it
installs the signed minified APK and runs instrumentation against it, which is
the only path that exercises R8 — the step that has broken this app five distinct
ways that a debug build cannot reproduce. Without the `ANDROID_RELEASE_*`
variables below, drop `--release` and the debug APK is used instead. The matrix — phone and tablet layouts, rotation,
process recreation, playback and background controls, Picture-in-Picture,
download cancellation/resume, and an offline launch with networking disabled —
is `scripts/android-acceptance.sh`; `--list` prints every case and what it
needs, and `PSTR_ACCEPTANCE_ONLY=name` runs one. Five cases need a real share,
given as `PSTR_ACCEPTANCE_SHARE_URL` and `PSTR_ACCEPTANCE_SHARE_PASSWORD`; they
reach the device as instrumentation arguments and are stored nowhere. Without
them those cases report `skip`. `picture-in-picture` is still `pending`, so the
run tells you the matrix is incomplete — but no longer because of B40, which is
fixed; the case now just needs writing. `docs/TESTING.md` has the layer-by-layer
detail, including why lint runs against a baseline.

Requires JDK 17. `build-android.sh` locates it, because a newer JVM does not
fail with a version message — the Kotlin compiler throws
`IllegalArgumentException: 26.0.2` from an IntelliJ version parser.

## Release Signing

Release signing is optional locally and never stored in the repository. Set all
four variables before `bash scripts/build-android.sh signed`:

```text
ANDROID_RELEASE_STORE_FILE
ANDROID_RELEASE_STORE_PASSWORD
ANDROID_RELEASE_KEY_ALIAS
ANDROID_RELEASE_KEY_PASSWORD
```

GitHub Actions expects the keystore itself as the base64-encoded
`ANDROID_RELEASE_KEYSTORE_BASE64` secret and the other three values as secrets.
Keep the keystore and passwords in a durable external secret manager; losing the
key prevents users from upgrading an installed APK.

A `v*` tag runs `release.yml`, which calls `android.yml` with
`signed_release: true` and publishes the signed APK as
`proton-stream-<version>-android.apk` next to the desktop artifacts, together
with the `mpv-android-source-<rev>.tar.gz` corresponding source for the bundled
libmpv. The AAB is kept as the `android-bundle` workflow artifact only; nothing
uploads it to Play. A manual `android.yml` run with `signed_release` builds the
same signed files without publishing them.

## Licensing and Native Playback

The Android application is GPL-3.0-or-later; the shared Rust crates remain MIT.
See `android/LICENSE.md` and `android/THIRD_PARTY_NOTICES.md`. Before distributing
a binary with bundled libmpv, pin its exact source revision/build configuration,
package the notices, and retain the corresponding source plus build scripts
required by the GPL. The current playback service scaffold is not a substitute
for the final bundled libmpv integration.

`bash scripts/build-libmpv-android.sh` pins and stages libmpv for both supported
ABIs, plus a source archive and revision record, under
`android/app/build/generated/mpv`. Gradle runs that task before native builds,
packages the staged libraries, and compiles `pstr_mpv`, the JNI/EGL adapter. It
feeds libmpv through the Rust stream C ABI; decrypted bytes remain in native
memory. `PlaybackService` owns the mpv core for background audio and media
controls, while the activity supplies the current `Surface` and enters PiP.
Do not publish an Android binary until this exact pipeline is verified on real
arm64 hardware and an x86_64 emulator.
