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
    Color32, CornerRadius, Rect, Stroke, SystemTheme, TextureHandle, TextureOptions, Theme,
    ThemePreference, ViewportCommand, Visuals,
};
use parking_lot::RwLock;
use pstr_core::appearance::Appearance;

/// Card geometry. The grid is built from these, so a change here moves
/// everything together.
pub const CARD_WIDTH: f32 = 232.0;
/// Video stills are 16:9. A poster shape would letterbox every one of them.
pub const CARD_ASPECT: f32 = 9.0 / 16.0;
pub const CARD_GAP: f32 = 14.0;
/// The poster grid's minimum width: narrower than a still, because a cover is
/// read by its art rather than by detail, and six across a laptop screen is
/// what makes a wall of covers look like a shelf.
pub const POSTER_WIDTH: f32 = 168.0;
/// Covers are 2:3.
pub const POSTER_ASPECT: f32 = 3.0 / 2.0;
/// How much a tile's picture grows under the pointer, as a fraction of its
/// size. Half of it lands on each side, which has to fit in half of
/// [`CARD_GAP`] at the widest card, or a lifted tile overlaps its neighbour.
pub const TILE_LIFT: f32 = 0.04;

/// The type ramp, named once.
///
/// Every size in the app comes from here. They used to be literals at the call
/// site — `.size(17.0)` in one place and `.size(18.0)` in another for the same
/// kind of heading — which is not a scale, it is forty independent decisions
/// that happen to be close together. Naming them makes the ramp a thing that
/// can be changed, and gives the Android client something to match: a shared
/// palette settles colour, and this settles the other half.
///
/// Sizes rather than full `FontId`s because everything here is proportional;
/// the one monospace style in the app is egui's own, set in [`apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The name in the library's banner and at the top of a title's page,
    /// where there is no logo to stand in for it. Set in Inter Display, the
    /// cut drawn for this size: tighter and heavier than the text face, which
    /// at forty points looks like body copy that was zoomed.
    Hero,
    /// The name of the thing you are looking at, once per page.
    Display,
    /// An empty state, or the heading of a page that has no display line.
    Title,
    /// The brand in the nav bar, and a dialog's heading.
    Heading,
    /// A run of content under a rule.
    Section,
    /// A row that groups the rows under it — a season header.
    Subhead,
    /// Everything that is prose.
    Body,
    /// The text on a small control.
    Label,
    /// Context rather than content: counts, sizes, errors under a field.
    Caption,
    /// The smallest thing that stays legible — a badge over artwork.
    Micro,
}

impl Role {
    pub const fn size(self) -> f32 {
        match self {
            Self::Hero => 34.0,
            Self::Display => 26.0,
            Self::Title => 20.0,
            Self::Heading => 18.0,
            Self::Section => 17.0,
            Self::Subhead => 15.0,
            Self::Body => 14.0,
            Self::Label => 13.0,
            Self::Caption => 12.0,
            Self::Micro => 11.0,
        }
    }

    /// The hero line is set in the display cut, headings in the semibold
    /// one, and everything below them in the regular one.
    pub fn family(self) -> egui::FontFamily {
        match self {
            Self::Hero => display(),
            Self::Display | Self::Title | Self::Heading | Self::Section => semibold(),
            _ => egui::FontFamily::Proportional,
        }
    }

    /// For the hand-painted parts, which lay text out through a `Painter` and
    /// never see a `RichText`.
    pub fn font(self) -> egui::FontId {
        egui::FontId::new(self.size(), self.family())
    }

    /// For the parts that go through egui's widgets.
    pub fn rich(self, text: impl Into<String>) -> egui::RichText {
        egui::RichText::new(text)
            .size(self.size())
            .family(self.family())
    }
}

/// The spacing scale. Gaps between things come from here rather than being
/// picked per call site, so two neighbouring groups are 12 apart or 16 apart
/// and never 14 in one place and 15 in the next.
pub mod space {
    /// Between lines that belong to one thing: a title and its caption.
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    /// Between controls in one cluster.
    pub const S: f32 = 6.0;
    pub const M: f32 = 8.0;
    /// Between clusters on one row, and between rows of a form.
    pub const L: f32 = 12.0;
    /// Between the groups on a page.
    pub const XL: f32 = 16.0;
    /// Between sections.
    pub const XXL: f32 = 24.0;
}

/// Corner radii, as `egui::CornerRadius::same` takes them.
pub mod radius {
    /// A progress bar, or a strip along an edge.
    pub const BAR: u8 = 3;
    /// A badge over artwork. Android's `Shape.kt` has the same number.
    pub const BADGE: u8 = 4;
    /// A small control inside something else: a menu entry, a chip.
    pub const SM: u8 = 6;
    /// Everything else either client paints: cards, buttons, the tab pill.
    pub const MD: u8 = 8;
    /// Something that floats: a row card, a dialog, a toast, the hero.
    pub const LG: u8 = 12;
}

/// How long things take to change, in seconds, for `Context::animate_*`.
pub mod motion {
    /// A press, which has to answer before the finger is off the button.
    pub const PRESS: f32 = 0.05;
    /// A hover arriving and leaving.
    pub const HOVER: f32 = 0.12;
    /// A control changing state: a switch flipping, the tab pill travelling,
    /// one page giving way to the next.
    pub const STATE: f32 = 0.15;
    /// Chrome fading in and out: the player's controls, a toast.
    pub const FADE: f32 = 0.18;
    /// Artwork arriving over its placeholder. Slower, because it replaces
    /// something rather than adding to it, and a fast swap reads as a flicker.
    pub const ARRIVE: f32 = 0.24;
    /// One pass of the highlight across a loading skeleton. Slow, because it
    /// is something to wait in front of, not something happening.
    pub const SHIMMER: f32 = 1.4;

    /// Fast out, slow in: the curve every hand-driven transition uses.
    pub fn ease_out(t: f32) -> f32 {
        1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
    }
}

/// The UI typeface, bundled rather than taken from the system so a title
/// sets the same on every desktop. Inter, under the OFL — the licence is
/// beside the files.
///
/// All three files have their private use area mappings removed, because the
/// Phosphor icons live there. Inter maps 745 codepoints in that area to its
/// own alternate glyphs, and as the first font in the family it won: the
/// refresh arrow drew as an accented letter. The glyphs themselves are still
/// in the files; only the character map entries are gone. Strip them again
/// after any Inter update (fontTools: drop every `cmap` subtable entry in
/// U+E000..=U+F8FF).
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
/// Inter Display Bold, the optical size for the hero line. Stripped like the
/// other two.
const INTER_DISPLAY: &[u8] = include_bytes!("../assets/fonts/InterDisplay-Bold.ttf");

/// The family the headings are set in. egui has no notion of weight, so the
/// semibold cut is a family of its own.
pub fn semibold() -> egui::FontFamily {
    egui::FontFamily::Name("semibold".into())
}

/// The family [`Role::Hero`] is set in.
pub fn display() -> egui::FontFamily {
    egui::FontFamily::Name("display".into())
}

/// The family the subtitle preview is drawn in: the face the chosen subtitle
/// font resolves to, ahead of the app's own fonts, which stand in until it has
/// loaded and for any glyph it lacks.
pub fn subtitle_family() -> egui::FontFamily {
    egui::FontFamily::Name("subtitle".into())
}

/// The system fonts loaded so far. Each install builds the definitions from
/// all of them, so loading one does not drop the other.
#[derive(Default)]
struct SystemFonts {
    cjk: Option<Arc<egui::FontData>>,
    subtitle: Option<(SubtitleRequest, Arc<egui::FontData>, SubtitleFace)>,
    /// The subtitle face last asked for, loading or loaded.
    wanted: Option<SubtitleRequest>,
}

static SYSTEM_FONTS: parking_lot::Mutex<SystemFonts> = parking_lot::Mutex::new(SystemFonts {
    cjk: None,
    subtitle: None,
    wanted: None,
});

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubtitleRequest {
    family: String,
    bold: bool,
    italic: bool,
}

/// What the loaded subtitle face draws by itself. A style it lacks is one
/// libass makes up, and so must the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtitleFace {
    pub bold: bool,
    pub italic: bool,
}

/// Inter, the icon font, and egui's own fonts behind them for the emoji and
/// symbols Inter does not carry. The CJK fallback goes last, when there is
/// one.
fn font_definitions(system: &SystemFonts) -> egui::FontDefinitions {
    use egui::FontFamily::{Monospace, Proportional};

    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("inter".into(), Arc::new(egui::FontData::from_static(INTER)));
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(egui::FontData::from_static(INTER_SEMIBOLD)),
    );
    fonts.font_data.insert(
        "inter-display".into(),
        Arc::new(egui::FontData::from_static(INTER_DISPLAY)),
    );
    fonts
        .families
        .entry(Proportional)
        .or_default()
        .insert(0, "inter".into());
    // Right behind Inter. The icons are in the private use area, which the
    // bundled Inter no longer maps, so neither font shadows the other.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(cjk) = &system.cjk {
        let name = "system-cjk-fallback".to_owned();
        fonts.font_data.insert(name.clone(), cjk.clone());
        for family in [Proportional, Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    let fallbacks: Vec<String> = fonts.families[&Proportional]
        .iter()
        .filter(|name| *name != "inter")
        .cloned()
        .collect();
    let chain = |first: &str| {
        std::iter::once(first.to_owned())
            .chain(fallbacks.iter().cloned())
            .collect()
    };
    fonts.families.insert(semibold(), chain("inter-semibold"));
    fonts.families.insert(display(), chain("inter-display"));
    let mut subtitle_chain = fonts.families[&Proportional].clone();
    if let Some((_, face, _)) = &system.subtitle {
        let name = "system-subtitle".to_owned();
        fonts.font_data.insert(name.clone(), face.clone());
        subtitle_chain.insert(0, name);
    }
    fonts.families.insert(subtitle_family(), subtitle_chain);
    fonts
}

/// Install the bundled fonts now, and a CJK fallback when the system has one.
///
/// The bundled ones are compiled in and cost nothing to hand over. The CJK
/// fallback is on a thread of its own: `fc-match` and reading a font file of
/// tens of megabytes held the first frame back by a visible moment, and every
/// name that needs the fallback is still loading from the catalog by then
/// anyway. egui deliberately does not bundle the multi-megabyte CJK families,
/// and native desktops already provide one.
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions(&SYSTEM_FONTS.lock()));
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("fonts".into())
        .spawn(move || install_cjk_fallback(&ctx));
    if let Err(error) = spawned {
        tracing::debug!("font fallback thread: {error}");
    }
}

fn install_cjk_fallback(ctx: &egui::Context) {
    let Some((path, index)) = system_cjk_font() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Some(data) = system_font_data(bytes, index) else {
        tracing::debug!("CJK fallback {}#{index} does not parse", path.display());
        return;
    };
    let mut system = SYSTEM_FONTS.lock();
    system.cjk = Some(Arc::new(data));
    ctx.set_fonts(font_definitions(&system));
    ctx.request_repaint();
}

/// The face the subtitle preview should be drawn in for this font, bold and
/// italic, once [`subtitle_family`] holds it.
///
/// `None` while it loads, and for good where the system cannot say which file
/// a family is — everywhere but Linux. The first call for a font starts the
/// load on a thread of its own, since `fc-match` and reading the file are too
/// slow for a frame, and asks for a repaint when it is in.
pub fn subtitle_face(
    ctx: &egui::Context,
    family: &str,
    bold: bool,
    italic: bool,
) -> Option<SubtitleFace> {
    let request = SubtitleRequest {
        family: family.to_owned(),
        bold,
        italic,
    };
    let mut system = SYSTEM_FONTS.lock();
    if let Some((loaded, _, face)) = &system.subtitle
        && *loaded == request
    {
        return Some(*face);
    }
    if system.wanted.as_ref() != Some(&request) {
        system.wanted = Some(request.clone());
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("subtitle-font".into())
            .spawn(move || install_subtitle_face(&ctx, request));
        if let Err(error) = spawned {
            tracing::debug!("subtitle font thread: {error}");
        }
    }
    None
}

fn install_subtitle_face(ctx: &egui::Context, request: SubtitleRequest) {
    let Some((path, index, face)) = system_subtitle_font(&request) else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Some(data) = system_font_data(bytes, index) else {
        tracing::debug!("subtitle font {}#{index} does not parse", path.display());
        return;
    };
    let mut system = SYSTEM_FONTS.lock();
    // Another font was picked while this one loaded.
    if system.wanted.as_ref() != Some(&request) {
        return;
    }
    system.subtitle = Some((request, Arc::new(data), face));
    ctx.set_fonts(font_definitions(&system));
    ctx.request_repaint();
}

/// A system font, if egui can read it.
///
/// egui panics on a font it cannot parse, and this one comes from whatever the
/// system has installed. So it is parsed here first, the same way egui does,
/// and a font that fails is left out rather than taking the app down.
fn system_font_data(bytes: Vec<u8>, index: u32) -> Option<egui::FontData> {
    skrifa::FontRef::from_index(&bytes, index).ok()?;
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    Some(data)
}

/// The face within a font collection that fontconfig's `%{index}` names.
///
/// Fontconfig packs a variable font's named instance into the high 16 bits —
/// Fedora's variable Noto Sans CJK answers 262144, instance 4 of face 0. The
/// instance is not a face, and passing it to egui as one stopped the app at
/// start.
#[cfg(any(target_os = "linux", test))]
fn fontconfig_face(index: u32) -> u32 {
    index & 0xFFFF
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
        .map(fontconfig_face)
        .unwrap_or(0);
    path.is_file().then_some((path, index))
}

/// The file fontconfig picks for a subtitle font, as libass asks for it: the
/// family with the weight and slant wanted, falling back as libass would to
/// whatever fontconfig likes best when the family is not installed.
#[cfg(target_os = "linux")]
fn system_subtitle_font(
    request: &SubtitleRequest,
) -> Option<(std::path::PathBuf, u32, SubtitleFace)> {
    let output = std::process::Command::new("fc-match")
        .args([
            "-f",
            "%{file}\\n%{index}\\n%{weight}\\n%{slant}",
            &fontconfig_pattern(request),
        ])
        .output()
        .ok()?;
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let mut lines = text.lines();
    let path = std::path::PathBuf::from(lines.next()?);
    let mut number = || lines.next().and_then(|value| value.parse::<u32>().ok());
    let index = number().unwrap_or(0);
    let (weight, slant) = (number().unwrap_or(0), number().unwrap_or(0));
    // A variable font's named instance is drawn by egui as the default one,
    // which is upright and regular, whatever fontconfig says it matched.
    let instance = index != fontconfig_face(index);
    let face = SubtitleFace {
        bold: !instance && weight >= FC_WEIGHT_BOLD,
        italic: !instance && slant > 0,
    };
    path.is_file()
        .then_some((path, fontconfig_face(index), face))
}

#[cfg(any(target_os = "linux", test))]
const FC_WEIGHT_BOLD: u32 = 200;

/// A fontconfig pattern for the request. A family name is free text, and `-`,
/// `:`, `,` and `\\` mean something in a pattern, so those are escaped.
#[cfg(any(target_os = "linux", test))]
fn fontconfig_pattern(request: &SubtitleRequest) -> String {
    let mut pattern = String::new();
    for character in request.family.chars() {
        if matches!(character, '-' | ':' | ',' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    if request.bold {
        pattern.push_str(&format!(":weight={FC_WEIGHT_BOLD}"));
    }
    if request.italic {
        pattern.push_str(":slant=100");
    }
    pattern
}

/// Elsewhere there is no asking which file a family is, and the preview
/// stays in the app's own font.
#[cfg(not(target_os = "linux"))]
fn system_subtitle_font(_: &SubtitleRequest) -> Option<(std::path::PathBuf, u32, SubtitleFace)> {
    None
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

/// The font families installed here, by name, for the subtitle font picker.
///
/// `None` until they have been read. The first call starts that on a thread
/// of its own and asks for a repaint when it is done: `fc-list` over a large
/// font collection takes long enough to be seen as a stalled frame.
pub fn font_families(ctx: &egui::Context) -> Option<&'static [String]> {
    static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Some(families) = FAMILIES.get() {
        return Some(families);
    }
    if !STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("font-families".into())
            .spawn(move || {
                let _ = FAMILIES.set(system_font_families());
                ctx.request_repaint();
            });
        if let Err(error) = spawned {
            tracing::debug!("font families thread: {error}");
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn system_font_families() -> Vec<String> {
    let Ok(output) = std::process::Command::new("fc-list")
        .args([":", "family"])
        .output()
    else {
        return Vec::new();
    };
    font_families_from_fc_list(&String::from_utf8_lossy(&output.stdout))
}

/// Elsewhere the picker takes a typed name only. mpv's font lookup there is
/// the system's own, so any installed family's name works.
#[cfg(not(target_os = "linux"))]
fn system_font_families() -> Vec<String> {
    Vec::new()
}

/// One family per line of `fc-list : family`, sorted and without repeats.
///
/// A line lists a family's names in every language it has, comma-separated,
/// and one family appears once per style. The first name is the one libass
/// matches on.
#[cfg(any(target_os = "linux", test))]
fn font_families_from_fc_list(listing: &str) -> Vec<String> {
    let mut families: Vec<String> = listing
        .lines()
        .filter_map(|line| line.split(',').next())
        .map(|family| family.trim().to_owned())
        .filter(|family| !family.is_empty() && !family.starts_with('.'))
        .collect();
    families.sort_by_key(|family| family.to_lowercase());
    families.dedup();
    families
}

/// Every colour the app draws with, as egui wants them.
///
/// The *choice* of colours is `pstr_core::appearance` — one resolution shared
/// with the Android client, so a flavour means the same thing on both. This is
/// only that palette converted into `Color32`, which is a type `pstr-core`
/// deliberately knows nothing about.
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
    /// Ink that stays readable on top of the accent.
    pub on_accent: Color32,
    pub danger: Color32,
    /// Whether this is a light theme.
    pub light: bool,
}

const fn color(rgb: pstr_core::appearance::Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

impl Palette {
    /// The default, spelled out so the global has something to hold before
    /// [`apply`] first runs — a `Ui` built during startup, or a panic message
    /// painted before the engine exists, still gets colours.
    const PROTON: Self = Self::of(pstr_core::appearance::Palette::PROTON);

    const fn of(palette: pstr_core::appearance::Palette) -> Self {
        Self {
            background: color(palette.background),
            surface: color(palette.surface),
            sunken: color(palette.sunken),
            card: color(palette.card),
            card_hover: color(palette.card_hover),
            border: color(palette.border),
            text: color(palette.text),
            muted: color(palette.muted),
            accent: color(palette.accent),
            accent_alt: color(palette.accent_alt),
            accent_dim: color(palette.accent_dim),
            on_accent: color(palette.on_accent),
            danger: color(palette.danger),
            light: palette.light,
        }
    }

    /// Resolve a choice into colours.
    pub fn resolve(appearance: Appearance) -> Self {
        Self::of(pstr_core::appearance::Palette::resolve(appearance))
    }
}

/// Blend two colours in linear light, through the shared implementation.
fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let rgb = |c: Color32| pstr_core::appearance::Rgb::new(c.r(), c.g(), c.b());
    color(pstr_core::appearance::mix(rgb(from), rgb(to), t))
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

/// Whether ramps are drawn at all.
///
/// Wider than [`gradients`], which also asks whether the *accent* has two hues
/// to run between. The viewer who turns gradients off is usually on a panel that
/// bands them into stripes, and that applies to every ramp in the app, not only
/// the accent's — so the bar fade and the title backdrop ask this one.
pub fn ramps_on() -> bool {
    ACTIVE.read().gradients
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

/// The accent as a shape rather than paint, for the one caller that has to
/// draw it *behind* something already laid out: the pill that slides between
/// the tabs in the navigation bar, whose position is only known once every tab
/// has been placed.
pub fn accent_shape(
    ctx: &egui::Context,
    rect: Rect,
    corner_radius: impl Into<CornerRadius>,
) -> egui::Shape {
    let corner_radius = corner_radius.into();
    let palette = palette();
    if !ACTIVE.read().gradients || palette.accent == palette.accent_alt {
        return egui::epaint::RectShape::filled(rect, corner_radius, palette.accent).into();
    }
    let texture = ramp(
        ctx,
        palette.accent,
        palette.accent_alt,
        Direction::Horizontal,
    );
    textured(rect, corner_radius, &texture)
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

/// A rounded rectangle running from one colour to another, at `opacity`.
///
/// For colours that are not the palette's — a title's own, on a tile with no
/// picture. Drawn flat when the viewer has ramps off, like every other ramp.
pub fn ramp_fill(
    painter: &egui::Painter,
    rect: Rect,
    corner_radius: CornerRadius,
    (from, to): (Color32, Color32),
    direction: Direction,
    opacity: f32,
) {
    let tint = Color32::WHITE.gamma_multiply(opacity);
    if !ramps_on() {
        painter.rect_filled(
            rect,
            corner_radius,
            mix(from, to, 0.5).gamma_multiply(opacity),
        );
        return;
    }
    let texture = ramp(painter.ctx(), from, to, direction);
    painter.add(
        egui::epaint::RectShape::filled(rect, corner_radius, tint).with_texture(
            texture.id(),
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        ),
    );
}

/// A ramp from `color` to nothing, for laying artwork into the page.
///
/// The other ramps here run between two opaque colours; this one runs down its
/// own alpha, so what is under it shows through the far end. That is what makes
/// a backdrop end in the page rather than in a horizontal line across the
/// window.
/// `arriving` is which end of `direction` the colour is solid at: `false` for a
/// scrim that starts opaque and lets the picture out, `true` for one that lets
/// the picture *into* the page — which is the one the title backdrop wants,
/// since the page is below the band.
///
/// The ramp itself is white and the colour is the shape's fill, which the GPU
/// multiplies in: one texture per direction, however many colours fade —
/// a banner tinted per title would otherwise build a ramp for every frame of
/// its cross-fade.
pub fn fade_shape(
    ctx: &egui::Context,
    rect: Rect,
    color: Color32,
    direction: Direction,
    arriving: bool,
) -> egui::Shape {
    let texture = alpha_ramp(ctx, Color32::WHITE, direction, arriving);
    let mut shape = egui::epaint::RectShape::filled(rect, CornerRadius::ZERO, color);
    shape.brush = Some(Arc::new(egui::epaint::Brush {
        fill_texture_id: texture.id(),
        uv: Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
    }));
    shape.into()
}

/// The page colour tinted by a picture's colour, for the veils over that
/// picture: see [`pstr_core::appearance::mood_veil`]. The page colour itself
/// while the picture has not arrived.
pub fn mood_veil(mood: Option<Color32>) -> Color32 {
    let palette = palette();
    let Some(mood) = mood else {
        return palette.background;
    };
    let rgb = |c: Color32| pstr_core::appearance::Rgb::new(c.r(), c.g(), c.b());
    color(pstr_core::appearance::mood_veil(
        rgb(palette.background),
        rgb(mood),
        palette.light,
    ))
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

/// The shadow under a raised tile, at `strength` of its full depth.
///
/// Depth is the one separator this app has been doing without: everything is
/// told apart by fill colour, which works until two things that should be at
/// different heights are the same colour. Kept soft and offset downwards —
/// a shadow you can see the edge of reads as a border.
///
/// Scaled rather than switched on, so it can ride the same hover value as the
/// border and arrive with it.
pub fn tile_shadow(strength: f32) -> egui::epaint::Shadow {
    shadow(strength, [0, 6], 16, 90)
}

/// The shadow a bar casts onto the page: shallower than a tile's, because a bar
/// is not floating over the page so much as sitting against it.
///
/// `cast_down` for the bar at the top of the window, off for the transport bar
/// at the bottom — a shadow that falls the same way on both would light the
/// window from two directions at once.
pub fn bar_shadow(cast_down: bool) -> egui::epaint::Shadow {
    shadow(1.0, [0, if cast_down { 4 } else { -4 }], 12, 70)
}

fn shadow(strength: f32, offset: [i8; 2], blur: u8, alpha: u8) -> egui::epaint::Shadow {
    let strength = strength.clamp(0.0, 1.0);
    egui::epaint::Shadow {
        offset,
        blur,
        spread: 0,
        // A light theme cannot carry the same shadow: the same black over a
        // near-white page is a smudge, where over a dark one it is barely
        // visible. Half strength on light, which is roughly where the two read
        // the same.
        color: Color32::from_black_alpha(
            (f32::from(alpha) * strength * if palette().light { 0.5 } else { 1.0 }) as u8,
        ),
    }
}

/// A soft highlight sweeping across a loading skeleton on a diagonal: what
/// says the shape is waiting for something rather than empty.
///
/// The phase runs off the clock rather than per tile, so every skeleton on
/// screen sweeps in step, as one surface. Asks for the next frame itself; a
/// skeleton is only ever on screen for a moment.
pub fn shimmer(painter: &egui::Painter, rect: Rect) {
    let time = painter.ctx().input(|input| input.time);
    let phase = (time / f64::from(motion::SHIMMER)).fract() as f32;
    let span = rect.width() * 0.6;
    let centre = rect.left() - span + phase * (rect.width() + 2.0 * span);
    let skew = rect.height() * 0.4;
    let highlight = if palette().light {
        Color32::from_black_alpha(10)
    } else {
        Color32::from_white_alpha(12)
    };
    let mut mesh = egui::Mesh::default();
    for (x, color) in [
        (centre - span, Color32::TRANSPARENT),
        (centre, highlight),
        (centre + span, Color32::TRANSPARENT),
    ] {
        mesh.colored_vertex(egui::pos2(x + skew, rect.top()), color);
        mesh.colored_vertex(egui::pos2(x - skew, rect.bottom()), color);
    }
    for quad in 0..2 {
        let i = quad * 2;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .add(mesh);
    painter.ctx().request_repaint();
}

/// The hover and press film on its own, for a control whose fill was painted by
/// somebody else — the selected tab, which sits on the pill.
pub fn veil_over(
    painter: &egui::Painter,
    rect: Rect,
    corner_radius: impl Into<CornerRadius>,
    lift: f32,
) {
    veil(painter, rect, corner_radius.into(), lift);
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

    let radius = CornerRadius::same(radius::MD);
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

        // egui's own style names, pointed at the ramp above so that a stock
        // widget nobody restyled still lands on it.
        use egui::TextStyle::{Body, Button, Heading, Monospace, Small};
        style.text_styles = [
            (Heading, Role::Display.font()),
            (Body, Role::Body.font()),
            (Button, Role::Body.font()),
            (Small, Role::Caption.font()),
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
    /// The last field separates a ramp between two colours from one that runs
    /// down its own alpha — the two would otherwise collide on
    /// `(colour, transparent)`.
    ramps: BTreeMap<(u32, u32, bool, u8), TextureHandle>,
}

static ACTIVE: RwLock<Active> = RwLock::new(Active {
    palette: Palette::PROTON,
    gradients: true,
    ramps: BTreeMap::new(),
});

/// A ramp between two colours, built once and kept.
fn ramp(ctx: &egui::Context, from: Color32, to: Color32, direction: Direction) -> TextureHandle {
    build(ctx, from, to, direction, 0, mix)
}

/// A ramp from `color` down to nothing.
///
/// The alpha is what is interpolated; the colour stays put. Fading the colour
/// as well would take a dark scrim through grey on its way out, which over
/// artwork reads as fog rather than as the picture ending.
fn alpha_ramp(
    ctx: &egui::Context,
    color: Color32,
    direction: Direction,
    arriving: bool,
) -> TextureHandle {
    build(
        ctx,
        color,
        Color32::TRANSPARENT,
        direction,
        1 + u8::from(arriving),
        move |from, _, t| from.gamma_multiply(if arriving { t } else { 1.0 - t }),
    )
}

/// One texture per ramp, built on demand and kept until the palette changes.
///
/// `kind` says which of the three kinds this is, so that a fade and a colour
/// ramp between the same two ends do not come out sharing one texture.
fn build(
    ctx: &egui::Context,
    from: Color32,
    to: Color32,
    direction: Direction,
    kind: u8,
    stop: impl Fn(Color32, Color32, f32) -> Color32,
) -> TextureHandle {
    let key = (
        u32::from_le_bytes(from.to_array()),
        u32::from_le_bytes(to.to_array()),
        direction == Direction::Vertical,
        kind,
    );
    if let Some(texture) = ACTIVE.read().ramps.get(&key) {
        return texture.clone();
    }

    let pixels: Vec<Color32> = (0..RAMP_STOPS)
        .map(|index| stop(from, to, index as f32 / (RAMP_STOPS - 1) as f32))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_font_family_is_listed_once_by_its_first_name() {
        let listing = "Noto Sans,Noto Sans Regular\nDejaVu Sans\nnoto sans mono\nNoto Sans,Noto Sans Bold\n\n.LastResort\n";
        assert_eq!(
            font_families_from_fc_list(listing),
            ["DejaVu Sans", "Noto Sans", "noto sans mono"]
        );
    }

    #[test]
    fn a_variable_fonts_named_instance_is_not_taken_for_a_face() {
        assert_eq!(fontconfig_face(262_144), 0);
        assert_eq!(fontconfig_face(0x0002_0001), 1);
        assert_eq!(fontconfig_face(2), 2);
    }

    #[test]
    fn a_system_font_egui_cannot_read_is_left_out() {
        assert!(system_font_data(INTER.to_vec(), 0).is_some());
        assert!(system_font_data(INTER.to_vec(), 4).is_none());
        assert!(system_font_data(b"not a font".to_vec(), 0).is_none());
    }

    #[test]
    fn a_subtitle_font_is_asked_of_fontconfig_with_its_style_and_escaped() {
        let request = |family: &str, bold, italic| SubtitleRequest {
            family: family.to_owned(),
            bold,
            italic,
        };
        assert_eq!(
            fontconfig_pattern(&request("Noto Sans", false, false)),
            "Noto Sans"
        );
        assert_eq!(
            fontconfig_pattern(&request("Noto Sans", true, true)),
            "Noto Sans:weight=200:slant=100"
        );
        assert_eq!(
            fontconfig_pattern(&request("A-B:C,D\\E", false, true)),
            "A\\-B\\:C\\,D\\\\E:slant=100"
        );
    }

    #[test]
    fn the_subtitle_preview_family_exists_before_any_font_has_loaded() {
        let fonts = font_definitions(&SystemFonts::default());
        assert_eq!(
            fonts.families[&subtitle_family()],
            fonts.families[&egui::FontFamily::Proportional]
        );
    }

    #[test]
    fn the_bundled_inter_leaves_the_icon_codepoints_to_phosphor() {
        use skrifa::MetadataProvider as _;

        for (name, bytes) in [
            ("regular", INTER),
            ("semibold", INTER_SEMIBOLD),
            ("display", INTER_DISPLAY),
        ] {
            let font = skrifa::FontRef::new(bytes).expect("bundled Inter parses");
            let shadowed: Vec<u32> = font
                .charmap()
                .mappings()
                .map(|(codepoint, _)| codepoint)
                .filter(|codepoint| (0xE000..=0xF8FF).contains(codepoint))
                .collect();
            assert!(
                shadowed.is_empty(),
                "Inter {name} maps {} private use codepoints, starting U+{:04X}",
                shadowed.len(),
                shadowed[0]
            );
        }
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
        // The blend itself is `pstr_core::appearance`'s and tested there; what
        // is checked here is that the conversion in and out of `Color32` is
        // not lossy at the ends.
        let from = Color32::from_rgb(0, 0, 0);
        let to = Color32::from_rgb(255, 255, 255);
        assert_eq!(mix(from, to, 0.0), from);
        assert_eq!(mix(from, to, 1.0), to);
        assert_ne!(mix(from, to, 0.5), from);
    }
}
