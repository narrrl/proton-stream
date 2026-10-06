//! Compiles `proton-stream.rc` into the Windows exe; nothing on any other
//! target. `rc.exe` on an MSVC build, mingw's `windres` on the cross build
//! from Linux.

fn main() {
    println!("cargo:rerun-if-changed=proton-stream.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/proton-stream.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Only the GUI gets the icon; `pstr` is a console tool.
    embed_resource::compile_for("proton-stream.rc", ["proton-stream"], embed_resource::NONE)
        .manifest_optional()
        .expect("compile the icon resource");
}
