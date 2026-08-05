//! The look: one palette, chosen once and swapped at runtime.
//!
//! Deliberately dark and low-contrast around the edges — this is a window full
//! of video stills, and every bit of chrome brightness competes with them. The
//! accent is the only strong colour in the app, so anything wearing it is
//! something to click.
//!
//! ## Why the palette is a function call and not a constant
//!
//! It used to be a wall of `const`s, which is what a theme wants to be right up
//! until there is more than one of them. The colours now come from
//! [`palette()`], which reads a process-wide palette that [`apply`] writes.
//! Every alternative was worse: threading a `&Palette` through the fifty call
//! sites in `ui/` would have put a lifetime on every drawing signature in the
//! app, and stashing it in `egui::Context` would have made the colour of a
//! label depend on which `Ui` happened to be in scope. There is one window and
//! one palette in it; a global is what that is.
//!
//! The palette is only ever written by [`apply`], from the UI thread, between
//! frames.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{
    Color32, CornerRadius, Rect, Rgba, Stroke, SystemTheme, TextureHandle, TextureOptions, Theme,
    ThemePreference, ViewportCommand, Visuals,
};
use parking_lot::RwLock;
use pstr_core::appearance::{Accent, Appearance, Flavor};

/// Card geometry. The grid is built from these, so a change here moves
/// everything together.
pub const CARD_WIDTH: f32 = 232.0;
/// Video stills are 16:9. A poster shape would letterbox every one of them.
pub const CARD_ASPECT: f32 = 9.0 / 16.0;
pub const CARD_GAP: f32 = 14.0;

/// Install a real CJK fallback before the default egui fonts.  egui ships a
/// compact Latin font, but it intentionally does not bundle the multi-megabyte
/// CJK families.  Native desktops already provide one, so use it when present.
pub fn install_font_fallbacks(ctx: &egui::Context) {
    let Some((path, index)) = system_cjk_font() else {
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    let name = "system-cjk-fallback".to_owned();
    fonts.font_data.insert(name.clone(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    ctx.set_fonts(fonts);
}

#[cfg(target_os = "linux")]
fn system_cjk_font() -> Option<(std::path::PathBuf, u32)> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\\n%{index}", "Noto Sans CJK JP"])
        .output()
        .ok()?;
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let mut lines = text.lines();
    let path = std::path::PathBuf::from(lines.next()?);
    let index = lines
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    path.is_file().then_some((path, index))
}

#[cfg(target_os = "windows")]
fn system_cjk_font() -> Option<(std::path::PathBuf, u32)> {
    let path = std::path::PathBuf::from(r"C:\Windows\Fonts\YuGothR.ttc");
    path.is_file().then_some((path, 0))
}

#[cfg(target_os = "macos")]
fn system_cjk_font() -> Option<(std::path::PathBuf, u32)> {
    let path = std::path::PathBuf::from("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc");
    path.is_file().then_some((path, 0))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn system_cjk_font() -> Option<(std::path::PathBuf, u32)> {
    None
}

/// Every colour the app draws with, resolved from a [`Flavor`] and an
/// [`Accent`].
///
/// `Copy`, and small, so reading it is a lock and a memcpy rather than anything
/// a drawing loop has to think about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Behind everything: the page.
    pub background: Color32,
    /// The bars above and below it.
    pub surface: Color32,
    /// The deepest colour in the flavour — text fields, wells.
    pub sunken: Color32,
    /// Tiles, rows, forms.
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    /// The one strong colour.
    pub accent: Color32,
    /// What a gradient in the accent runs into. Equal to [`Self::accent`] when
    /// the accent is a single hue and gradients would have nothing to do.
    pub accent_alt: Color32,
    /// The accent taken down into the background, for a pressed control and for
    /// selection behind text.
    pub accent_dim: Color32,
    /// Ink that stays readable on top of the accent. Chosen by the accent's
    /// luminance, because Catppuccin's dark flavours have *pale* accents and
    /// white-on-pastel is not readable at 13 px.
    pub on_accent: Color32,
    pub danger: Color32,
    /// Whether this is a light theme.
    pub light: bool,
}

/// Which way a gradient runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

/// The palette as it stands.
pub fn palette() -> Palette {
    ACTIVE.read().palette
}

pub fn background() -> Color32 {
    palette().background
}
pub fn surface() -> Color32 {
    palette().surface
}
pub fn card() -> Color32 {
    palette().card
}
pub fn card_hover() -> Color32 {
    palette().card_hover
}
pub fn text() -> Color32 {
    palette().text
}
pub fn muted() -> Color32 {
    palette().muted
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn accent_dim() -> Color32 {
    palette().accent_dim
}
pub fn on_accent() -> Color32 {
    palette().on_accent
}
pub fn danger() -> Color32 {
    palette().danger
}

/// Whether the accent is currently drawn as a gradient.
///
/// True only when the viewer has gradients on *and* the accent is a pair of
/// hues rather than one: a "gradient" from a colour to itself is a flat fill
/// that costs a texture lookup.
pub fn gradients() -> bool {
    let active = ACTIVE.read();
    active.gradients && active.palette.accent != active.palette.accent_alt
}

/// Fill `rect` with the accent, as a gradient when there is one to draw.
///
/// `lift` brightens the result for a hovered control and darkens it for a
/// pressed one — `0.0` is the resting state. It is a veil over the top rather
/// than a change to the colours, because the gradient is a texture and tinting
/// a texture can only ever darken it.
pub fn accent_fill(
    painter: &egui::Painter,
    rect: Rect,
    corner_radius: impl Into<CornerRadius>,
    lift: f32,
) {
    let corner_radius = corner_radius.into();
    fill(painter, rect, corner_radius, &palette());
    veil(painter, rect, corner_radius, lift);
}

/// The same, for a palette that is not the one on screen: the accent swatches
/// on the settings page, each of which previews the theme clicking it would
/// give.
pub fn swatch_fill(
    painter: &egui::Painter,
    rect: Rect,
    corner_radius: impl Into<CornerRadius>,
    palette: &Palette,
) {
    fill(painter, rect, corner_radius.into(), palette);
}

/// Paint `palette`'s accent — gradient if it has two hues and gradients are on,
/// flat otherwise.
fn fill(painter: &egui::Painter, rect: Rect, corner_radius: CornerRadius, palette: &Palette) {
    if !ACTIVE.read().gradients || palette.accent == palette.accent_alt {
        painter.rect_filled(rect, corner_radius, palette.accent);
        return;
    }
    let texture = ramp(
        painter.ctx(),
        palette.accent,
        palette.accent_alt,
        Direction::Horizontal,
    );
    painter.add(textured(rect, corner_radius, &texture));
}

/// The one gradient that is not the accent: the top bar, which runs from the
/// surface colour down into the page so that the join between them is a fade
/// rather than an edge.
///
/// Handed back as a shape rather than painted, because a panel's background has
/// to be drawn *behind* content whose size is only known once it is laid out —
/// the caller reserves an index before drawing and fills it in after.
pub fn bar_shape(ctx: &egui::Context, rect: Rect) -> egui::Shape {
    let palette = palette();
    if !ACTIVE.read().gradients {
        return egui::epaint::RectShape::filled(rect, CornerRadius::ZERO, palette.surface).into();
    }
    let texture = ramp(
        ctx,
        palette.surface,
        // Not all the way to the page colour: a bar that ends in exactly the
        // background has no bottom edge, and the shadow under a top panel then
        // reads as the only thing separating the chrome from the content.
        mix(palette.surface, palette.background, 0.7),
        Direction::Vertical,
    );
    textured(rect, CornerRadius::ZERO, &texture)
}

/// A white or black film over `rect`, for hover and press states. A no-op at
/// zero, which is the case that matters — most controls are at rest.
fn veil(painter: &egui::Painter, rect: Rect, corner_radius: CornerRadius, lift: f32) {
    if lift == 0.0 {
        return;
    }
    let alpha = (lift.abs() * 255.0).clamp(0.0, 255.0) as u8;
    let film = if lift > 0.0 {
        Color32::from_white_alpha(alpha)
    } else {
        Color32::from_black_alpha(alpha)
    };
    painter.rect_filled(rect, corner_radius, film);
}

/// A rounded rectangle painted with a gradient texture.
///
/// The fill is white because `Brush` *multiplies* the texture by it, so
/// anything else would tint the ramp.
fn textured(rect: Rect, corner_radius: CornerRadius, texture: &TextureHandle) -> egui::Shape {
    let mut shape = egui::epaint::RectShape::filled(rect, corner_radius, Color32::WHITE);
    shape.brush = Some(Arc::new(egui::epaint::Brush {
        fill_texture_id: texture.id(),
        uv: Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
    }));
    shape.into()
}

/// Install a palette, and tell egui and the window manager which of the two
/// kinds of theme it is.
pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    let palette = Palette::resolve(appearance);

    // egui keeps a separate style per theme and follows the OS by default, and
    // `set_visuals` only writes the one that is active *right now*. On the
    // first call this runs before the platform has reported its theme, so on a
    // Windows desktop set to light the palette below would land on the unused
    // dark style and then be swapped out — panels stayed dark because they are
    // painted by hand, but every stock widget (buttons, the search field, the
    // volume slider) came back white. Pin the preference and write both styles.
    let theme = if palette.light {
        Theme::Light
    } else {
        Theme::Dark
    };
    ctx.set_theme(match theme {
        Theme::Light => ThemePreference::Light,
        Theme::Dark => ThemePreference::Dark,
    });
    // The native title bar follows the *window's* theme, not egui's. Without
    // this, Windows draws a light caption above a dark window.
    ctx.send_viewport_cmd(ViewportCommand::SetTheme(match theme {
        Theme::Light => SystemTheme::Light,
        Theme::Dark => SystemTheme::Dark,
    }));

    let mut visuals = if palette.light {
        Visuals::light()
    } else {
        Visuals::dark()
    };

    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.sunken;
    visuals.faint_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.selection.bg_fill = palette.accent_dim;
    visuals.selection.stroke = Stroke::new(1.0, palette.text);
    visuals.hyperlink_color = palette.accent;

    let radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.bg_fill = palette.surface;
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);

    visuals.widgets.inactive.bg_fill = palette.card;
    visuals.widgets.inactive.weak_bg_fill = palette.card;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;

    visuals.widgets.hovered.bg_fill = palette.card_hover;
    visuals.widgets.hovered.weak_bg_fill = palette.card_hover;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent);

    visuals.widgets.active.bg_fill = palette.accent_dim;
    visuals.widgets.active.weak_bg_fill = palette.accent_dim;
    visuals.widgets.active.corner_radius = radius;

    // Both, not just the active one: a menu or a tooltip that opens while the
    // platform is mid-way through telling us its theme should not flash the
    // stock palette.
    ctx.set_visuals_of(Theme::Dark, visuals.clone());
    ctx.set_visuals_of(Theme::Light, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 10.0;

        use egui::FontFamily::Proportional;
        use egui::TextStyle::{Body, Button, Heading, Monospace, Small};
        style.text_styles = [
            (Heading, egui::FontId::new(26.0, Proportional)),
            (Body, egui::FontId::new(14.0, Proportional)),
            (Button, egui::FontId::new(14.0, Proportional)),
            (Small, egui::FontId::new(11.5, Proportional)),
            (
                Monospace,
                egui::FontId::new(12.5, egui::FontFamily::Monospace),
            ),
        ]
        .into();
    });

    // Dropped, not rebuilt: the textures are only wanted if something draws a
    // gradient this frame, and building them here would load two textures on
    // every theme change including the ones nothing gradient-filled follows.
    let mut active = ACTIVE.write();
    active.palette = palette;
    active.gradients = appearance.gradients;
    active.ramps.clear();
    drop(active);

    ctx.request_repaint();
}

/// The palette, and the gradient textures drawn from it.
struct Active {
    palette: Palette,
    gradients: bool,
    /// Gradient ramps by the pair of colours and the direction they run,
    /// emptied whenever the palette changes.
    ///
    /// Keyed on the colours rather than on what they are *for* because the
    /// settings page previews every accent at once: eight swatches in a
    /// flavour the window is not wearing are eight ramps that belong to no
    /// palette in particular. Bounded by the number of flavours times the
    /// number of accents, and only the ones actually drawn.
    ramps: BTreeMap<(u32, u32, bool), TextureHandle>,
}

static ACTIVE: RwLock<Active> = RwLock::new(Active {
    palette: Palette::PROTON,
    gradients: true,
    ramps: BTreeMap::new(),
});

/// A ramp between two colours, built once and kept.
fn ramp(ctx: &egui::Context, from: Color32, to: Color32, direction: Direction) -> TextureHandle {
    let key = (
        u32::from_le_bytes(from.to_array()),
        u32::from_le_bytes(to.to_array()),
        direction == Direction::Vertical,
    );
    if let Some(texture) = ACTIVE.read().ramps.get(&key) {
        return texture.clone();
    }

    let pixels: Vec<Color32> = (0..RAMP_STOPS)
        .map(|stop| mix(from, to, stop as f32 / (RAMP_STOPS - 1) as f32))
        .collect();
    let size = match direction {
        Direction::Horizontal => [RAMP_STOPS, 1],
        Direction::Vertical => [1, RAMP_STOPS],
    };
    let texture = ctx.load_texture(
        "pstr-ramp",
        egui::ColorImage::new(size, pixels),
        // Clamped and filtered: the ramp is stretched across a whole seek bar,
        // and nearest-neighbour would show every one of its stops.
        TextureOptions::LINEAR,
    );
    ACTIVE.write().ramps.insert(key, texture.clone());
    texture
}

/// How many texels a ramp is drawn from.
///
/// Sampled linearly, so this is not the number of visible steps — it is how
/// closely the ramp follows sRGB. Interpolation happens in the texture's own
/// gamma space, and a two-texel ramp between distant hues takes a visibly
/// different path through colour space than one built stop by stop in linear
/// light, which is what this does.
const RAMP_STOPS: usize = 64;

/// Blend two colours in linear light. Mixing in sRGB darkens the middle of a
/// ramp between saturated hues, which is exactly where a gradient is looked at.
fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let from = Rgba::from(from);
    let to = Rgba::from(to);
    Color32::from(from * (1.0 - t) + to * t)
}

/// Relative luminance, in linear light, as WCAG defines it.
fn luminance(color: Color32) -> f32 {
    let rgba = Rgba::from(color);
    0.2126 * rgba.r() + 0.7152 * rgba.g() + 0.0722 * rgba.b()
}

/// WCAG contrast between two opaque colours: 1.0 for a colour against itself,
/// 21.0 for black against white.
fn contrast(one: Color32, other: Color32) -> f32 {
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The least contrast a label gets anywhere along a gradient between two
/// colours. Sampled rather than solved: the worst point is not always an end,
/// because the ink can be brighter than one end and darker than the other.
fn worst_contrast(from: Color32, to: Color32, ink: Color32) -> f32 {
    (0..=4)
        .map(|step| contrast(mix(from, to, step as f32 / 4.0), ink))
        .fold(f32::INFINITY, f32::min)
}

/// The least contrast the app will accept between a label and the fill under
/// it: WCAG AA for large text, which is what wears the accent — button labels,
/// a tab, a play glyph, never body copy.
const MIN_CONTRAST: f32 = 3.0;

/// One flavour's raw colours, named as Catppuccin names them.
///
/// Stored rather than computed: these are somebody else's palette, and the
/// point of using it is to use it exactly.
#[derive(Clone, Copy)]
struct Ramp {
    base: Color32,
    mantle: Color32,
    crust: Color32,
    surface0: Color32,
    surface1: Color32,
    subtext0: Color32,
    text: Color32,
    pink: Color32,
    mauve: Color32,
    sky: Color32,
    sapphire: Color32,
    blue: Color32,
    lavender: Color32,
    teal: Color32,
    green: Color32,
    peach: Color32,
    yellow: Color32,
    red: Color32,
    light: bool,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// The palette this app shipped with, extended to the hues an accent can be.
///
/// Only `mauve` — Proton purple — and `red` are from the original; the rest are
/// chosen to sit on a near-black base at roughly the saturation that one does,
/// which is a good deal hotter than any Catppuccin flavour.
const PROTON: Ramp = Ramp {
    base: rgb(0x0e0e12),
    mantle: rgb(0x17171d),
    crust: rgb(0x0a0a0d),
    surface0: rgb(0x1e1e26),
    surface1: rgb(0x2a2a35),
    subtext0: rgb(0x8e8e9c),
    text: rgb(0xeaeaf0),
    pink: rgb(0xff5fbe),
    mauve: rgb(0x7d4dff),
    sky: rgb(0x4dd2ff),
    sapphire: rgb(0x3a8fc4),
    blue: rgb(0x3f6fe0),
    lavender: rgb(0xa68cff),
    teal: rgb(0x3fd6b8),
    green: rgb(0x56d364),
    peach: rgb(0xff9a4d),
    yellow: rgb(0xffd166),
    red: rgb(0xe05561),
    light: false,
};

const LATTE: Ramp = Ramp {
    base: rgb(0xeff1f5),
    mantle: rgb(0xe6e9ef),
    crust: rgb(0xdce0e8),
    surface0: rgb(0xccd0da),
    surface1: rgb(0xbcc0cc),
    subtext0: rgb(0x6c6f85),
    text: rgb(0x4c4f69),
    pink: rgb(0xea76cb),
    mauve: rgb(0x8839ef),
    sky: rgb(0x04a5e5),
    sapphire: rgb(0x209fb5),
    blue: rgb(0x1e66f5),
    lavender: rgb(0x7287fd),
    teal: rgb(0x179299),
    green: rgb(0x40a02b),
    peach: rgb(0xfe640b),
    yellow: rgb(0xdf8e1d),
    red: rgb(0xd20f39),
    light: true,
};

const FRAPPE: Ramp = Ramp {
    base: rgb(0x303446),
    mantle: rgb(0x292c3c),
    crust: rgb(0x232634),
    surface0: rgb(0x414559),
    surface1: rgb(0x51576d),
    subtext0: rgb(0xa5adce),
    text: rgb(0xc6d0f5),
    pink: rgb(0xf4b8e4),
    mauve: rgb(0xca9ee6),
    sky: rgb(0x99d1db),
    sapphire: rgb(0x85c1dc),
    blue: rgb(0x8caaee),
    lavender: rgb(0xbabbf1),
    teal: rgb(0x81c8be),
    green: rgb(0xa6d189),
    peach: rgb(0xef9f76),
    yellow: rgb(0xe5c890),
    red: rgb(0xe78284),
    light: false,
};

const MACCHIATO: Ramp = Ramp {
    base: rgb(0x24273a),
    mantle: rgb(0x1e2030),
    crust: rgb(0x181926),
    surface0: rgb(0x363a4f),
    surface1: rgb(0x494d64),
    subtext0: rgb(0xa5adcb),
    text: rgb(0xcad3f5),
    pink: rgb(0xf5bde6),
    mauve: rgb(0xc6a0f6),
    sky: rgb(0x91d7e3),
    sapphire: rgb(0x7dc4e4),
    blue: rgb(0x8aadf4),
    lavender: rgb(0xb7bdf8),
    teal: rgb(0x8bd5ca),
    green: rgb(0xa6da95),
    peach: rgb(0xf5a97f),
    yellow: rgb(0xeed49f),
    red: rgb(0xed8796),
    light: false,
};

const MOCHA: Ramp = Ramp {
    base: rgb(0x1e1e2e),
    mantle: rgb(0x181825),
    crust: rgb(0x11111b),
    surface0: rgb(0x313244),
    surface1: rgb(0x45475a),
    subtext0: rgb(0xa6adc8),
    text: rgb(0xcdd6f4),
    pink: rgb(0xf5c2e7),
    mauve: rgb(0xcba6f7),
    sky: rgb(0x89dceb),
    sapphire: rgb(0x74c7ec),
    blue: rgb(0x89b4fa),
    lavender: rgb(0xb4befe),
    teal: rgb(0x94e2d5),
    green: rgb(0xa6e3a1),
    peach: rgb(0xfab387),
    yellow: rgb(0xf9e2af),
    red: rgb(0xf38ba8),
    light: false,
};

impl Ramp {
    const fn of(flavor: Flavor) -> Self {
        match flavor {
            Flavor::Proton => PROTON,
            Flavor::Latte => LATTE,
            Flavor::Frappe => FRAPPE,
            Flavor::Macchiato => MACCHIATO,
            Flavor::Mocha => MOCHA,
        }
    }

    /// The accent, and the hue a gradient in it runs into.
    ///
    /// The partners are neighbours on the wheel rather than complements: a
    /// gradient across half the spectrum passes through a colour that belongs
    /// to neither end, and on a seek bar that reads as a bug. The exception is
    /// [`Accent::PinkSky`], which is the whole point of that entry.
    const fn accents(&self, accent: Accent) -> (Color32, Color32) {
        match accent {
            Accent::Mauve => (self.mauve, self.blue),
            Accent::Pink => (self.pink, self.mauve),
            Accent::Sky => (self.sky, self.sapphire),
            Accent::PinkSky => (self.pink, self.sky),
            Accent::Lavender => (self.lavender, self.blue),
            Accent::Blue => (self.blue, self.sapphire),
            Accent::Teal => (self.teal, self.green),
            Accent::Peach => (self.peach, self.yellow),
        }
    }
}

impl Palette {
    /// The default, spelled out as a constant so the global has something to
    /// hold before [`apply`] first runs — a `Ui` built during startup, or a
    /// panic message painted before the engine exists, still gets colours.
    const PROTON: Self = Self {
        background: PROTON.base,
        surface: PROTON.mantle,
        sunken: PROTON.crust,
        card: PROTON.surface0,
        card_hover: PROTON.surface1,
        border: rgb(0x262630),
        text: PROTON.text,
        muted: PROTON.subtext0,
        accent: PROTON.mauve,
        accent_alt: PROTON.blue,
        accent_dim: rgb(0x5333ad),
        on_accent: PROTON.text,
        danger: PROTON.red,
        light: false,
    };

    /// Resolve a choice into colours.
    pub fn resolve(appearance: Appearance) -> Self {
        let ramp = Ramp::of(appearance.flavor);
        let (accent, partner) = ramp.accents(appearance.accent);
        // Catppuccin's Latte accents are tuned to be *read*, on a light page,
        // at body-text weight — which makes them mid-luminance, and a
        // mid-luminance fill is one that neither black nor white sits on. They
        // are taken down in value here, and only here: the hue and the
        // saturation are Latte's, so it still looks like Latte, and a label on
        // a button is legible.
        let deepen = |color: Color32| {
            if ramp.light {
                mix(color, Color32::BLACK, 0.30)
            } else {
                color
            }
        };
        let accent = deepen(accent);
        let accent_alt = if appearance.gradients {
            deepen(partner)
        } else {
            accent
        };

        // Ink that survives on the accent. The flavour's own reading first —
        // pale text on a dark theme — and the opposite when that does not clear
        // the bar, which on Catppuccin's dark flavours is most of the time:
        // their accents are *pastel*, and white on #f5c2e7 is not a label.
        //
        // Latte's darkest colour is its body text, and even that is only #4c4f69,
        // so the dark side is taken a third of the way to black.
        let ink_dark = if ramp.light {
            mix(ramp.text, Color32::BLACK, 0.35)
        } else {
            ramp.crust
        };
        let ink_light = if ramp.light { ramp.base } else { ramp.text };
        let on_accent = if worst_contrast(accent, accent_alt, ink_light) >= MIN_CONTRAST {
            ink_light
        } else {
            ink_dark
        };

        Self {
            background: ramp.base,
            surface: ramp.mantle,
            sunken: ramp.crust,
            card: ramp.surface0,
            card_hover: ramp.surface1,
            border: ramp.surface1,
            text: ramp.text,
            muted: ramp.subtext0,
            accent,
            accent_alt,
            // Behind text, so it is the accent taken most of the way back to
            // the page: a selection in a saturated hue is a selection nobody
            // can read through.
            accent_dim: mix(accent, ramp.base, if ramp.light { 0.55 } else { 0.45 }),
            on_accent,
            danger: ramp.red,
            light: ramp.light,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_flavour_and_accent_resolves_to_readable_ink() {
        for flavor in Flavor::ALL {
            for accent in Accent::ALL {
                let palette = Palette::resolve(Appearance {
                    flavor,
                    accent,
                    gradients: true,
                });
                let ratio = worst_contrast(palette.accent, palette.accent_alt, palette.on_accent);
                assert!(
                    ratio >= MIN_CONTRAST,
                    "{flavor:?}/{accent:?}: contrast {ratio:.2}",
                );
            }
        }
    }

    #[test]
    fn only_the_light_flavour_reports_itself_light() {
        for flavor in Flavor::ALL {
            let palette = Palette::resolve(Appearance {
                flavor,
                ..Appearance::default()
            });
            assert_eq!(palette.light, flavor.is_light());
            // And the page is on the right side of the middle either way.
            assert_eq!(luminance(palette.background) > 0.5, flavor.is_light());
        }
    }

    #[test]
    fn turning_gradients_off_leaves_one_colour_to_draw() {
        let flat = Palette::resolve(Appearance {
            accent: Accent::PinkSky,
            gradients: false,
            ..Appearance::default()
        });
        assert_eq!(flat.accent, flat.accent_alt);
    }

    #[test]
    fn the_shipped_default_is_the_palette_that_was_here_before() {
        let resolved = Palette::resolve(Appearance::default());
        assert_eq!(resolved.background, Palette::PROTON.background);
        assert_eq!(resolved.accent, Palette::PROTON.accent);
        assert_eq!(resolved.text, Palette::PROTON.text);
        assert_eq!(resolved.muted, Palette::PROTON.muted);
    }

    #[test]
    fn a_ramp_runs_from_one_colour_to_the_other() {
        let from = Color32::from_rgb(0, 0, 0);
        let to = Color32::from_rgb(255, 255, 255);
        assert_eq!(mix(from, to, 0.0), from);
        assert_eq!(mix(from, to, 1.0), to);
        assert!(luminance(mix(from, to, 0.5)) > luminance(from));
        assert!(luminance(mix(from, to, 0.5)) < luminance(to));
    }
}
