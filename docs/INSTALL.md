# Installation

Every [release](https://github.com/narrrl/proton-stream/releases/latest) has packages for Linux
and Windows, an APK for Android and an IPA for iOS, plus a `SHA256SUMS` file to check them
against. The version is part of each file name.

| Platform | File | Needs |
|---|---|---|
| Debian, Ubuntu | `proton-stream_<version>-1_amd64.deb` | x86_64 |
| Fedora | `proton-stream-<version>-1.x86_64.rpm` | x86_64 |
| Other Linux | `proton-stream-<version>-x86_64-unknown-linux-gnu.tar.gz` | x86_64, libmpv 2 |
| Arch Linux | Built from the repository with `makepkg` | |
| Windows | `proton-stream-<version>-x64.msi` or `…-x86_64-windows.zip` | x64 |
| Android, Android TV | `proton-stream-<version>-android.apk` | Android 12 or newer, arm64 or x86_64 |
| iOS, iPadOS | `proton-stream-<version>-ios.ipa` | iOS 17 or newer, SideStore or AltStore |

## Linux

The desktop app needs libmpv, WebKitGTK 4.1 (for the CAPTCHA window when you sign in) and a
Secret Service keyring such as GNOME Keyring or KWallet. The packages pull in everything except
the keyring, which most desktops already have.

### Debian and Ubuntu

Download the `.deb`, then:

```bash
sudo apt install ./proton-stream_*_amd64.deb
```

### Fedora

Download the `.rpm`, then:

```bash
sudo dnf install ./proton-stream-*.x86_64.rpm
```

### Arch Linux

The `PKGBUILD` in `packaging/` builds the checkout it sits in. It needs network access, so cargo
can fetch crates:

```bash
git clone https://github.com/narrrl/proton-stream.git
cd proton-stream/packaging
makepkg -si
```

### Other distributions

Unpack the tarball anywhere. It holds `proton-stream` (the app), `pstr` (the
[command line](CLI.md)) and the icon. Install libmpv 2 and WebKitGTK 4.1 from your
distribution first.

Both packages install:

- `/usr/bin/proton-stream`, the app, listed as **Proton Stream** in your applications menu
- `/usr/bin/pstr`, the command line

## Windows

Download either file from the release:

- **The `.msi` installer** installs to `Program Files\Proton Stream`, adds a Start menu entry
  and puts `pstr` on the PATH.
- **The portable `.zip`** runs from any folder. Keep every file in it together: the app does not
  start without `libmpv-2.dll` and `WebView2Loader.dll` beside it.

The binaries are not code-signed yet, so SmartScreen may warn the first time. Choose **More
info**, then **Run anyway**. The sign-in window uses the WebView2 runtime that comes with Windows
10 and 11.

## Android and Android TV

1. Download `proton-stream-<version>-android.apk` on the device.
2. Open it. Android asks once to allow installs from your browser or file manager.
3. Install.

The APK is signed, so later versions install over it and keep your library. On Android TV the app
appears in the launcher; installing needs a file manager or `adb install`.

## iOS and iPadOS

The `.ipa` is unsigned. [SideStore](https://sidestore.io) or [AltStore](https://altstore.io)
signs it with your own Apple ID when it installs:

1. Install SideStore or AltStore by following its own guide.
2. Download `proton-stream-<version>-ios.ipa`.
3. Open it with SideStore or AltStore.

A free Apple ID has to refresh the app every 7 days. SideStore and AltStore do that for you.

## Updating

Install the new release over the old one. Your shares, library, watch history and settings stay.
The app does not check for updates by itself; watch the
[releases](https://github.com/narrrl/proton-stream/releases) on GitHub.

## Uninstalling

Remove the package the way you installed it, for example `sudo apt remove proton-stream`.
This leaves your data behind. To remove that too:

1. In the app, remove each share on the **Shares** page. This deletes its link and password
   from the keyring and its downloads from the disk.
2. Sign out on the **Shares** page, if you signed in.
3. Delete the app's folders, listed in [Privacy](PRIVACY.md#what-stays-on-your-device).

On Android and iOS, uninstalling the app deletes everything it stored.

## Building from source

Rust 1.96 or newer, plus the libmpv (≥ 2.0) development headers, and WebKitGTK 4.1's on Linux:

```bash
git clone https://github.com/narrrl/proton-stream.git
cd proton-stream
cargo build --release
```

[Development](DEVELOPMENT.md) and `packaging/README.md` have the details, including Windows, the
Android toolchain and iOS builds on macOS.
