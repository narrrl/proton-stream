# Third-Party Notices

The proton-stream iOS distribution combines open-source components. The
complete GNU GPL version 3 text is packaged beside this file as `GPL-3.0.txt`.

| Component | License |
|---|---|
| mpv/libmpv (MPVKit `MPVKit-GPL` 1.0.0) | GPL-2.0-or-later; verify the pinned build configuration |
| FFmpeg (MPVKit GPL build) | GPL-2.0-or-later |
| MPVKit | LGPL-3.0 |
| MoltenVK | Apache-2.0 |
| libplacebo | LGPL-2.1-or-later |
| libass, FreeType, FriBidi, HarfBuzz | ISC, FTL, LGPL-2.1-or-later, MIT |
| UniFFI | MPL-2.0 |
| Inter typeface (bundled, `Inter-OFL.txt`) | OFL-1.1 |
| proton-stream reusable Rust crates | MIT |

Before release, replace this dependency summary with an inventory generated
from the resolved Cargo and Swift Package dependency graphs and package every
required license text. A bundled libmpv release must identify the exact source
revision, patches, and build configuration, and make the corresponding source
available under the GPL.
