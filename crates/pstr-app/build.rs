//! Compiles `proton-stream.rc` into the Windows exe; nothing on any other
//! target. `rc.exe` on an MSVC build, mingw's `windres` on the cross build
//! from Linux.

fn main() {
    println!("cargo:rerun-if-changed=proton-stream.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/proton-stream.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    export_gpu_preference();
    // Only the GUI gets the icon; `pstr` is a console tool.
    embed_resource::compile_for("proton-stream.rc", ["proton-stream"], embed_resource::NONE)
        .manifest_optional()
        .expect("compile the icon resource");
}

/// Export `NvOptimusEnablement` and `AmdPowerXpressRequestHighPerformance`
/// from `proton-stream.exe` (see `src/main.rs`). An executable exports nothing
/// by default, so the symbols go through a module-definition file: `link.exe`
/// takes it as `/DEF:`, and GNU ld for PE takes a `.def` as a plain input.
fn export_gpu_preference() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let def = out.join("proton-stream-exports.def");
    std::fs::write(
        &def,
        "EXPORTS\n    NvOptimusEnablement DATA\n    AmdPowerXpressRequestHighPerformance DATA\n",
    )
    .expect("write the exports .def");
    let arg = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        format!("/DEF:{}", def.display())
    } else {
        def.display().to_string()
    };
    println!("cargo:rustc-link-arg-bin=proton-stream={arg}");
}
