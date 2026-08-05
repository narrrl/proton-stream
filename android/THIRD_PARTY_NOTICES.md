# Third-Party Notices

The Android distribution combines components under compatible open-source
licenses. Preserve their notices and license texts in source and binary
distributions.

| Component | License |
|---|---|
| mpv/libmpv | GPL-2.0-or-later by default; verify the pinned build configuration |
| LLVM libc++ shared runtime (Android NDK r29) | Apache-2.0 WITH LLVM-exception |
| AndroidX and Jetpack Compose | Apache-2.0 |
| Kotlin | Apache-2.0 |
| JNA | Apache-2.0 or LGPL-2.1-or-later |
| UniFFI | MPL-2.0 |
| proton-stream shared Rust crates | MIT |

This summary is not a replacement for dependency license files. Before a
release, generate an inventory from the resolved Cargo and Gradle lock data,
review it, and package the complete license texts in the application. A bundled
libmpv release must also identify the exact upstream source revision, local
patches, configuration flags, and reproducible build instructions, and make the
corresponding source available as the GPL requires.
