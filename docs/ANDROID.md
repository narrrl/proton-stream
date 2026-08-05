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
root and `bash scripts/build-android.sh check`. Test phone and tablet layouts,
rotation, process recreation, playback/background controls, Picture-in-Picture,
download cancellation/resume, and an offline launch with networking disabled.

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

Tagged workflows currently retain signed builds as workflow artifacts only.
Publishing them to a GitHub release or Play remains disabled until the pinned
libmpv build and resulting APK have passed the on-device playback matrix. The
main release workflow publishes desktop artifacts only and fails if an APK or
AAB enters its artifact set.

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
