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

<https://github.com/shinchiro/mpv-winbuild-cmake/releases/latest>

(The SourceForge project these are better known by mirrors the same builds, but
its `/download` URLs hand anything that does not look like a browser an HTML
interstitial, so `--fetch-mpv` goes to the source.)

**The cross build from Linux is the supported path, and the one CI uses** —
`scripts/build-windows.sh --fetch-mpv` fetches the archive, links against its
`libmpv.dll.a` with mingw-w64 and builds the `.msi` with `wixl`. On a real
Windows box `scripts\build.ps1 -MpvDev C:\mpv-dev` does the MSVC equivalent, but
note that it needs an `mpv.lib` or an `mpv.def` to generate one from with
`lib.exe`, and **current mpv-dev archives ship neither** — only the mingw import
library. Producing `mpv.lib` from a recent archive means writing the `.def`
yourself from `dumpbin /exports libmpv-2.dll`.

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

## Releasing

A release is a `v<version>` tag on `main`; `.github/workflows/release.yml` does
the rest. Every job runs on Linux: `scripts/build.sh tarball deb rpm` for the
Linux artifacts and `scripts/build-windows.sh --fetch-mpv` for the Windows ones,
published together with a `SHA256SUMS` to a GitHub release. So the steps are:

```bash
# 1. bump Cargo.toml's workspace version *and* packaging/PKGBUILD's pkgver
# 2. commit on main, then
git tag -a v0.1.0 -m 'proton-stream 0.1.0'
git push origin main --follow-tags
```

The workflow refuses to publish a tag whose commit is not an ancestor of `main`,
or whose version disagrees with `Cargo.toml` or the PKGBUILD — the artifacts are
named from the manifest, so a drifted tag would ship a file called something
other than the tag. It also refuses a `Cargo.toml` carrying `[patch.crates-io]`:
a release builds against the published SDK crates, never a sibling checkout.

`workflow_dispatch` runs everything except the publish step, which is how to
rehearse without burning a tag.

Not covered by the workflow: the Arch package (`makepkg` compiles in place, so
it wants an Arch host) and the macOS `.app`/`.dmg` (no runner for it here yet).
Both are still `scripts/build.sh arch` and `scripts/build.sh dmg` by hand.
