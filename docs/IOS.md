# iOS Development

The iOS client lives in `ios/`. It is a SwiftUI port of the Android client, screen
for screen and string for string, over the same Rust engine: `crates/pstr-android`
builds unchanged for `aarch64-apple-ios` as a static library, and UniFFI writes its
Swift bindings (module `PstrBridge`). It targets iOS 17 and newer on iPhone and
iPad. Keep reusable behaviour in the Rust crates; when Android changes, the iOS
file of the same name is the one to change with it.

| Android | iOS |
|---|---|
| `ui/AppViewModel.kt` | `App/AppModel.swift` |
| `ui/ProtonStreamApp.kt` | `App/ProtonStreamApp.swift` (`RootView`) |
| `native/NativeRuntime.kt`, `native/KeystoreSecretStore.kt` | `App/NativeRuntime.swift`, `App/KeychainSecretStore.swift` |
| `settings/SettingsStore.kt` | `App/SettingsStore.swift` (same keys, in `UserDefaults`) |
| `ui/*Screen.kt`, `ui/AccountUi.kt`, `ui/LanguagePicker.kt` | `UI/*View.swift`, `UI/AccountUI.swift`, `UI/LanguagePicker.swift` |
| `ui/RemoteArtwork.kt`, `ui/theme/*` | `UI/RemoteArtwork.swift`, `UI/Theme/*` |
| `download/*` | `Download/*` |
| `sync/NewEpisodesWorker.kt` | `App/NewEpisodes.swift` |
| `playback/PlayerScreen.kt`, `NativeMpvHost.kt`, `PlaybackService.kt`, `StreamSession.kt` | `Player/PlayerView.swift`, `Player/PlayerHost.swift` |
| `cpp/pstr_mpv.cpp` | `Player/MpvPlayer.swift`, `Player/StreamProtocol.swift` |

Playback is libmpv from [MPVKit](https://github.com/mpvkit/MPVKit)'s GPL build,
rendering through MoltenVK into a `CAMetalLayer`, with the `pstr://` stream protocol
reading decrypted blocks straight out of Rust as on Android.

## Toolchain

macOS with Xcode 16 or newer, XcodeGen, and Rust 1.96 with the iOS targets:

```bash
brew install xcodegen
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

The Xcode project and `Info.plist` are generated from `ios/project.yml` and are not
committed, nor are the bindings under `ios/build/generated`.

```bash
scripts/build-ios.sh test    # simulator static library, bindings, unit tests
scripts/build-ios.sh all     # device static library, bindings, dist/proton-stream-<version>-ios.ipa
```

`all` is `rust`, `bindings` and `ipa` in order; each can be run on its own. After
`bindings` and an `xcodegen generate` in `ios/`, the project opens in Xcode as usual,
where a run on a device needs a development team set on the target.

The `.ipa` is unsigned. SideStore and AltStore re-sign what they install with the
viewer's own Apple ID, which is also why the app has no extensions: a free Apple ID
gets few app IDs, and each extension costs one.

`.github/workflows/ios.yml` runs the tests and builds the `.ipa` on `macos-15` for
every push to `main` and every pull request that touches the iOS sources or the Rust
crates under them. `release.yml` calls it too, and publishes the `.ipa` next to the
other artifacts.

`scripts/render-ios-icon.sh` renders `ios/ProtonStream/Resources/AppIcon.svg`, which
is Android's adaptive launcher icon with both layers on one canvas, into the asset
catalog.

## Where iOS differs

Each of these is the platform's doing, not a choice to diverge.

- **No Picture-in-Picture.** libmpv draws into a Metal layer, and AVKit's
  Picture-in-Picture only takes frames from `AVPlayerLayer` or
  `AVSampleBufferDisplayLayer`. Background audio and the lock-screen transport work
  as on Android.
- **No share sheet.** A share extension would cost a second app ID. A link reaches
  the app through its URL scheme — `protonstream://add?url=<link>`, or the link with
  `https` swapped for `protonstream` — or through **Paste link** in the Add share
  form, which finds a link anywhere in the clipboard's text.
- **Downloads run while the app does.** A transfer is a Rust task, not a
  `URLSession` task, so iOS has nothing to hand it to in the background. A download
  keeps going for the grace iOS gives a backgrounded app, then stops at a block
  boundary and is queued again; it resumes from its `.part` file when the app next
  comes forward. "Download on Wi-Fi only" stops transfers on a cellular or
  otherwise expensive network rather than holding them back.
- **Sync runs when the app leaves the screen**, inside the same grace, in
  place of Android's `WatchSyncWorker`, and every five minutes while it is open.
- **Background audio off pauses** the episode when the app leaves the screen, where
  Android stops it.
- **New-episode checks run when iOS chooses.** Android's `NewEpisodesWorker`
  crawls every six hours on a network. iOS's `BGProcessingTask` is asked for no
  sooner than six hours, and the system decides when, mostly overnight on a
  charger. An app refresh would come more often but gets about thirty seconds,
  which a crawl of a large share does not fit in.
- **Presses are drawn by hand.** Compose's ripple shows every press; SwiftUI's
  `.plain` button style and `onTapGesture` show none, which on a slow iPad reads as
  a tap that missed. `PressableStyle` and `tapAndHold` (`UI/Theme/Accent.swift`)
  stand in for the ripple, and a settings page goes back by an edge strip of its
  own, as a title page does, where Android has the system back.
- **No zoom into a title page.** iOS 18's `.navigationTransition(.zoom)` only
  runs inside a `NavigationStack`, and the title page is shown over the library
  by hand, so it keeps its slide in from the edge — Android's shared poster has
  no counterpart here.
- **Nothing is backed up**, as on Android (`allowBackup="false"`). The share
  secrets are this-device-only Keychain items, so a restored share list would
  arrive without them, and a restored `sync.json` would give two devices one sync
  identity.
