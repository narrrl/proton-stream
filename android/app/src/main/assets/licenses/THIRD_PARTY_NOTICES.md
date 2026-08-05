# Third-Party Notices

The proton-stream Android distribution combines open-source components. The
complete GNU GPL version 3 text is packaged beside this file as `GPL-3.0.txt`.

| Component | License |
|---|---|
| mpv/libmpv | GPL-2.0-or-later by default; verify the pinned build configuration |
| LLVM libc++ shared runtime (Android NDK r29) | Apache-2.0 WITH LLVM-exception |
| AndroidX and Jetpack Compose | Apache-2.0 |
| Kotlin | Apache-2.0 |
| JNA | Apache-2.0 or LGPL-2.1-or-later |
| UniFFI | MPL-2.0 |
| proton-stream reusable Rust crates | MIT |

Before release, replace this dependency summary with an inventory generated
from the resolved Cargo and Gradle dependency graphs and package every required
license text. A bundled libmpv release must identify the exact source revision,
patches, and build configuration, and make the corresponding source available
under the GPL.
