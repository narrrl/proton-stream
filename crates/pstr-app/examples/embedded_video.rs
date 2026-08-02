//! Smoke test for the embedded video path, with no Proton account in the way.
//!
//! Everything from `mpv_render_context_create` to the `painter.image` call is
//! exercised here — the GL entry-point loader, the framebuffer, the orientation
//! of the texture and the fact that egui can draw over the result. What is *not*
//! exercised is anything to do with shares, blocks or decryption, which is the
//! point: when the picture is wrong, this says whether the problem is in the
//! render path or above it.
//!
//! ```bash
//! cargo run -p pstr-app --example embedded_video              # a test pattern
//! cargo run -p pstr-app --example embedded_video -- /path.mkv # a real file
//! ```
//!
//! What to look for: a moving picture inside the bordered rectangle, right way
//! up, with the chrome around it drawn normally and the frame counter climbing.
//! A stuck counter means the update callback is not firing; an upside-down
//! picture means the `flip` argument in `VideoRenderer::render` is wrong; a
//! black rectangle with a healthy counter means the framebuffer is not the one
//! being sampled.

use std::sync::Arc;

use anyhow::Context as _;

use pstr_app::video::VideoSurface;
use pstr_player::{Player, PlayerConfig, PlayerEvent, VideoOutput};

/// mpv's own test source: a moving pattern, generated with no file on disk.
const TEST_PATTERN: &str = "av://lavfi:testsrc=size=1280x720:rate=30";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pstr_app=debug,pstr_player=debug".into()),
        )
        .init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| TEST_PATTERN.into());
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("embedded video smoke test")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "embedded_video",
        options,
        Box::new(move |cc| Ok(Box::new(Harness::new(cc, runtime, url)?) as Box<dyn eframe::App>)),
    )
    .map_err(|error| anyhow::anyhow!("run the window: {error}"))
}

struct Harness {
    player: Arc<Player>,
    video: Option<VideoSurface>,
    url: String,
    /// Set once, after the first frame: the render context has to exist before
    /// the file is loaded, and it can only be built with the GL context current.
    started: bool,
    frames: u64,
    last_event: String,
}

impl Harness {
    fn new(
        cc: &eframe::CreationContext<'_>,
        runtime: Arc<tokio::runtime::Runtime>,
        url: String,
    ) -> anyhow::Result<Self> {
        let config = PlayerConfig {
            video: VideoOutput::Embedded,
            on_screen_controller: false,
            default_keybindings: false,
            ..PlayerConfig::default()
        };
        let player = Arc::new(Player::new(runtime, config)?);

        let gl = cc
            .gl
            .clone()
            .context("eframe was built without a glow context")?;
        // SAFETY: `CreationContext` is handed to us on the UI thread with the
        // context current, which is the same guarantee `App::ui` gives.
        let video = unsafe { VideoSurface::new(Arc::clone(&player), gl, cc.egui_ctx.clone()) }?;

        Ok(Self {
            player,
            video: Some(video),
            url,
            started: false,
            frames: 0,
            last_event: "—".into(),
        })
    }
}

impl eframe::App for Harness {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if !self.started {
            self.started = true;
            if let Err(error) = self.player.play_url(&self.url) {
                self.last_event = format!("load failed: {error}");
            }
        }

        // Polled from the UI thread rather than a player thread: this harness is
        // about the picture, and a zero timeout keeps the draw loop honest.
        while let Some(event) = self.player.poll_event(0.0) {
            if !matches!(event, PlayerEvent::Position(_) | PlayerEvent::Other) {
                self.last_event = format!("{event:?}");
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("embedded video");
            ui.label(&self.url);
            ui.horizontal(|ui| {
                ui.label(format!("frames drawn: {}", self.frames));
                ui.separator();
                ui.label(format!("last event: {}", self.last_event));
                ui.separator();
                ui.label(match self.player.position() {
                    Some(position) => format!("at {position:.1}s"),
                    None => "no position yet".into(),
                });
            });
            ui.add_space(8.0);

            // Deliberately inset and bordered: a full-window picture would hide
            // whether the rectangle is respected, which is the thing most likely
            // to be wrong.
            let rect = ui.available_rect_before_wrap().shrink(24.0);
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), egui::Color32::DARK_GRAY);

            let pixels_per_point = ui.ctx().pixels_per_point();
            let size = [
                (rect.width() * pixels_per_point).round() as i32,
                (rect.height() * pixels_per_point).round() as i32,
            ];
            if let Some(video) = self.video.as_mut()
                && let Some(texture) = video.texture(frame, size)
            {
                self.frames += 1;
                ui.painter().image(
                    texture,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(2.0, egui::Color32::from_rgb(0x7d, 0x4d, 0xff)),
                egui::StrokeKind::Outside,
            );
        });
    }

    /// The surface holds GL objects and mpv's render context; both have to go
    /// while the context is still current.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.video = None;
    }
}
