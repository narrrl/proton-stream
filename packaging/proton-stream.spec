# RPM built from an already-staged tree, not from source.
#
# `scripts/build.sh rpm` runs cargo once, stages a filesystem image under
# dist/stage, and then calls:
#
#   rpmbuild -bb packaging/proton-stream.spec \
#     --define "version 0.1.0" --define "stagedir /abs/path/to/dist/stage"
#
# Staging rather than compiling inside rpmbuild is deliberate: one cargo build
# feeds the tarball, the .deb and the .rpm, and rpmbuild does not then need
# libmpv headers or a Rust toolchain on the packaging host.

%global debug_package %{nil}
# The binaries are prebuilt, quite possibly on a different distribution, so let
# the explicit Requires below stand rather than having rpm scan the ELF and
# generate soname deps against whatever this host happens to have.
AutoReqProv:    no

%{!?version: %global version 0.1.0}

Name:           proton-stream
Version:        %{version}
Release:        1%{?dist}
Summary:        Netflix-style desktop client for Proton Drive public links
License:        MIT
URL:            https://github.com/narrrl/proton-stream
ExclusiveArch:  x86_64

# libmpv does the demuxing and decoding; libsecret is where the share fragment
# and custom password live. SQLite is bundled into the binary (rusqlite
# `bundled`) and TLS is rustls, so neither shows up here.
Requires:       mpv-libs >= 0.34
Requires:       libsecret
Requires:       mesa-libGL
Requires:       libxkbcommon
Requires:       libX11
Requires:       libwayland-client
Requires:       hicolor-icon-theme

Recommends:     gnome-keyring

Provides:       pstr = %{version}-%{release}

%description
Paste a Proton Drive share URL and its password, and get a browsable,
streamable library: a poster wall, a page per title with seasons and episodes,
resume-where-you-left-off, and an embedded libmpv player. No Proton account,
no server, no download step — a file's content blocks each decrypt on their
own, so seeking costs the blocks the seek lands on.

%prep
test -n "%{?stagedir}" || (echo 'Pass --define "stagedir /abs/path" (see scripts/build.sh)' >&2; exit 1)
test -d "%{stagedir}/usr/bin"

%build
# Nothing to do: %{stagedir} already holds release binaries.

%install
cp -a "%{stagedir}"/. %{buildroot}/

%files
%{_bindir}/proton-stream
%{_bindir}/pstr
%{_datadir}/applications/io.narl.proton-stream.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.narl.proton-stream.svg
# Directory entries, so these stay correct whether or not the checkout has a
# LICENSE yet (a listed directory takes its contents with it).
%{_datadir}/doc/proton-stream/
%{_datadir}/licenses/proton-stream/

%changelog
* Sun Aug 02 2026 Nils Pukropp <contact@narl.io> - 0.1.0-1
- Initial package.
