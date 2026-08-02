# Packaging

Everything here is driven by two scripts. `scripts/build.sh` covers Linux and
macOS; `scripts/build.ps1` covers Windows, because the `.exe` and the `.msi`
need MSVC and WiX and neither is reachable from a Unix host.

| Artifact | Command | Host it must run on |
|---|---|---|
| `dist/proton-stream-<ver>-<triple>.tar.gz` | `scripts/build.sh tarball` | Linux or macOS |
| `dist/proton-stream_<ver>-1_amd64.deb` | `scripts/build.sh deb` | any Linux |
| `dist/proton-stream-<ver>-1.x86_64.rpm` | `scripts/build.sh rpm` | any Linux with `rpmbuild` |
| `packaging/*.pkg.tar.zst` | `cd packaging && makepkg -fi` | Arch |
| `dist/Proton Stream.app` + `.dmg` | `scripts/build.sh dmg` | macOS |
| `dist/windows/proton-stream.exe` + `.zip` | `scripts\build.ps1 -Artifacts zip` | Windows |
| `dist/proton-stream-<ver>-x64.msi` | `scripts\build.ps1 -Artifacts msi` | Windows |

`scripts/build.sh` with no arguments produces everything the host can.

## How it fits together

One cargo build feeds every Linux and macOS artifact. The binaries are compiled
once, staged into a filesystem image at `dist/stage` — rooted so that
`dist/stage/usr/bin/pstr` is exactly where the file lands on the target system —
and each packager copies out of that image. A `.deb` and an `.rpm` from the same
run are therefore the same bytes, and neither `dpkg-deb` nor `rpmbuild` needs a
Rust toolchain or libmpv headers.

`--skip-build` reuses `dist/stage`, which is what you want when iterating on a
control file or a spec.

The Arch package is the exception: `makepkg` compiles in place from the
checkout, by Arch convention and to match `../proton-drive-linux`. It ignores
`dist/stage` entirely. `scripts/build.sh arch` runs it for you and copies the
result into `dist/`, but it is not part of `all` — it recompiles, and most hosts
are not Arch.

## Runtime dependencies, and why they are what they are

| | Why |
|---|---|
| **libmpv ≥ 0.34** | The player. `pstr-player` links it directly and drives it through `stream_cb`; nothing here shells out to `mpv(1)`, so packages depend on the library, not the player binary. |
| **libsecret / Secret Service** | The share URL fragment and the custom password are secrets and live in the OS credential store, never in `shares.json`. |
| **libGL, libxkbcommon, libX11, libwayland-client** | eframe on glow, with both display backends compiled in. |

Not dependencies, deliberately: **SQLite** (rusqlite is `bundled`) and
**OpenSSL** (TLS is rustls with webpki-roots). Both are inside the binary.

Per packager that is:

- Debian — `libmpv2 | libmpv1` (trixie | bookworm), `libsecret-1-0`, `libgl1`,
  `libxkbcommon0`, `libx11-6`, `libwayland-client0`.
- RPM — `mpv-libs`, `libsecret`, `mesa-libGL`, `libxkbcommon`, `libX11`,
  `libwayland-client`.
- Arch — `mpv` (which carries both `libmpv.so` and its headers), `libsecret`.

The `.deb` and `.rpm` are built from binaries compiled on the packaging host, so
they inherit its glibc. A package built on Arch installs on Fedora rawhide but
not on an older distribution — build on the oldest one you intend to support, or
in that distribution's container.

`AutoReqProv: no` in the spec is for the same reason: the ELF scan would generate
soname requirements against whatever the packaging host happens to have, so the
explicit `Requires` list stands instead.

## Windows

libmpv is the whole difficulty. `libmpv2-sys` emits `cargo:rustc-link-lib=mpv`,
so the linker wants an import library named `mpv.lib` and the process wants
`libmpv-2.dll` at run time. Neither ships with mpv's Windows *player* build —
both come from the separate **mpv-dev** archive:

<https://sourceforge.net/projects/mpv-player-windows/files/libmpv/>

Unpack it and point the script at it:

```powershell
scripts\build.ps1 -MpvDev C:\mpv-dev
```

If that directory has `mpv.def` but no `mpv.lib`, the script generates the import
library with `lib.exe` (`/name:libmpv-2.dll`, so it references the DLL that
actually ships). It finds `lib.exe` on `PATH` or imports the MSVC environment via
`vswhere`, so a plain PowerShell session works.

There is no static libmpv worth shipping, so **"standalone exe" means the exe
plus `libmpv-2.dll` beside it** — that pair, plus `pstr.exe` and the README, is
what the portable `.zip` contains and what the `.msi` installs.

The MSI needs the WiX toolset; the script installs it (`dotnet tool install
--global wix`) and adds the UI extension if they are missing. It installs to
`Program Files\Proton Stream`, puts a Start Menu shortcut on the GUI, and adds
the install directory to the system `PATH` so `pstr` works from a shell — that
`PATH` entry is tied to `pstr.exe`'s component, so uninstalling removes it.

`UpgradeCode` in `proton-stream.wxs` is fixed forever: it is what makes 0.2.0
replace 0.1.0 rather than install beside it. Never regenerate it.

Signing is opt-in: `-SignCertThumbprint <thumbprint>` signs both executables and
the MSI.

## macOS

`scripts/build.sh dmg` produces `Proton Stream.app` inside a drag-to-Applications
disk image.

By default it copies libmpv and its non-system dependency closure into
`Contents/Frameworks` and rewrites every install name, because a `.dmg` that
requires the recipient to have run `brew install mpv` is not a `.dmg` anyone can
use. The bundle is then ad-hoc signed — every `install_name_tool` edit
invalidates the signature, and an unsigned-but-modified binary is killed outright
on arm64. Pass `--no-bundle-libs` to link against the host's libmpv instead.

The `.icns` is rasterized from `io.narl.proton-stream.svg` when `rsvg-convert` or
`magick` is available; without either, the bundle ships without an icon.

## Files

| | |
|---|---|
| `PKGBUILD` | Arch, in-tree `makepkg` build |
| `proton-stream.spec` | RPM, installs from `dist/stage` |
| `io.narl.proton-stream.desktop` / `.svg` | Freedesktop entry and icon |
| `macos/Info.plist.in` | `.app` bundle plist; `@VERSION@`/`@BIN@`/`@APP_ID@` substituted at build time |
| `windows/proton-stream.wxs` | WiX v4+ installer definition |
| `windows/proton-stream.ico` | MSI and shortcut icon, rasterized from the SVG |
| `windows/license.rtf` | Fallback for the installer's license dialog; the repo's own `LICENSE` is preferred when it exists |

The Debian `control` file has no template — `scripts/build.sh` writes it, because
almost every field in it is computed.

## Two things to know before releasing

- **The `[patch.crates-io]` block in `Cargo.toml` is development only.** It points
  at `../proton-sdk-rs`, so both build scripts refuse to start when it is present
  and that checkout is missing. A release must build against the published
  crates — drop the block once proton-sdk 0.3.3 is on crates.io.
- **There is no `LICENSE` file in the repository root** even though `Cargo.toml`
  declares MIT. Every packager warns and ships without the license text; the MSI
  falls back to `windows/license.rtf`. Adding the file fixes all of them at once.
