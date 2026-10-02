//! `proton-stream` — the desktop application.
//!
//! Everything is in the library beside this file; what is here is the process:
//! logging, the tokio runtime, the window, and the app inside it. See
//! [`pstr_app`] for how the three threads divide the work.

// A GUI binary linked against the Windows *console* subsystem gets a console
// allocated for it, so launching from the Start menu flashes up a cmd window
// that lives as long as the app. Only release builds are switched: with the
// windows subsystem there is no stdout for `tracing_subscriber` to write to, and
// a developer running `cargo run` on Windows wants the log more than the tidy
// window. `pstr-cli` stays on the console subsystem, which is correct for it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::Context;
use pstr_core::config::AppDirs;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pstr_app=info,pstr_core=info,pstr_stream=info".into()),
        )
        .init();

    let runtime = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("build the tokio runtime")?,
    );
    let dirs = AppDirs::ensure().context("resolve app directories")?;

    let window = pstr_app::window::WindowState::load(&dirs.window_file());
    let mut viewport = egui::ViewportBuilder::default();
    // X11 and Windows take the icon from the window. Wayland ignores this and
    // looks the app id up in the installed desktop entry instead.
    match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(error) => tracing::debug!("window icon: {error}"),
    }
    let mut options = eframe::NativeOptions {
        viewport: viewport
            .with_title("proton-stream")
            .with_inner_size(window.map_or([1180.0, 780.0], |state| [state.width, state.height]))
            .with_maximized(window.is_some_and(|state| state.maximized))
            .with_min_inner_size([720.0, 480.0])
            .with_app_id("io.narl.proton-stream"),
        ..Default::default()
    };
    // Off on Wayland, where a swap on a window nobody is looking at never
    // returns and the compositor decides the app has hung. `pstr_app::pacing`
    // has the whole of it, and caps the frame rate in vsync's place.
    options.glow_options.vsync = pstr_app::pacing::vsync();

    eframe::run_native(
        "proton-stream",
        options,
        Box::new(move |cc| {
            pstr_app::app::App::new(cc, runtime, dirs)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|error| error.into())
        }),
    )
    // `eframe::Error` is not `Send + Sync`, which `anyhow` wants of a source.
    .map_err(|error| anyhow::anyhow!("run the window: {error}"))
}
