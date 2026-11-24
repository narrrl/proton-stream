//! What the window wears.
//!
//! Only the *choice* lives here — the colours themselves are `pstr-app`'s, and
//! nothing outside the window has an opinion about them. This is stored beside
//! the playback preferences and read the same way: a file that will not parse is
//! a line in the log and a fall back to the defaults, because a broken theme
//! file must never be a reason the library does not open.

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// A palette family: the suite's two, Catppuccin's four, and one borrowed from
/// a menu screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavor {
    /// Plum-black and the suite's violet, the colours Proton Drive, Proton
    /// Gallery and their websites wear. The default.
    #[default]
    Proton,
    /// The same violet on the suite's pale lavender page.
    ProtonLight,
    /// Catppuccin Latte — the light one.
    Latte,
    Frappe,
    Macchiato,
    Mocha,
    /// Black, white and one loud red — the Persona 5 menus.
    Persona5,
}

impl Flavor {
    pub const ALL: [Self; 7] = [
        Self::Proton,
        Self::ProtonLight,
        Self::Mocha,
        Self::Macchiato,
        Self::Frappe,
        Self::Latte,
        Self::Persona5,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Proton => "Proton",
            Self::ProtonLight => "Proton Light",
            Self::Latte => "Catppuccin Latte",
            Self::Frappe => "Catppuccin Frappé",
            Self::Macchiato => "Catppuccin Macchiato",
            Self::Mocha => "Catppuccin Mocha",
            Self::Persona5 => "Persona 5",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Proton => "Plum-black, so the artwork is the only bright thing on screen",
            Self::ProtonLight => "Light, in the suite's pale lavender",
            Self::Latte => "Light, Catppuccin's",
            Self::Frappe => "Dark, warm, the lowest contrast of the three",
            Self::Macchiato => "Dark, between Frappé and Mocha",
            Self::Mocha => "Dark, the deepest of Catppuccin's three",
            Self::Persona5 => "Black and white with one loud red — pick the Red accent for it",
        }
    }

    /// Whether this flavour is a light theme: Proton Light and Latte.
    ///
    /// It decides more than the colours: egui keeps a style per theme and the
    /// platform draws the title bar from the window's own, so both have to be
    /// told which of the two this is.
    pub fn is_light(self) -> bool {
        matches!(self, Self::ProtonLight | Self::Latte)
    }
}

/// The one strong colour in the window — and, where a gradient is drawn, the
/// hue it runs into.
///
/// Named for the first hue rather than for the pair, except where the pair *is*
/// the point: [`Accent::PinkSky`] is a gradient before it is a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accent {
    /// Proton purple, and the equivalent hue in every Catppuccin flavour.
    #[default]
    Mauve,
    Pink,
    Sky,
    /// Pink running into light blue.
    PinkSky,
    Lavender,
    Blue,
    Teal,
    Peach,
    /// The suite's copper, the metal behind every icon in it. On a
    /// Catppuccin flavour, its peach taken towards its red.
    Copper,
    /// The flavour's warning colour, used on purpose. Every flavour has one
    /// because everything here needs a colour for a failure, and in
    /// [`Flavor::Persona5`] it is the point of the palette rather than an
    /// afterthought.
    Red,
}

impl Accent {
    pub const ALL: [Self; 10] = [
        Self::Mauve,
        Self::Pink,
        Self::Sky,
        Self::PinkSky,
        Self::Lavender,
        Self::Blue,
        Self::Teal,
        Self::Peach,
        Self::Copper,
        Self::Red,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mauve => "Mauve",
            Self::Pink => "Pink",
            Self::Sky => "Sky",
            Self::PinkSky => "Pink → Sky",
            Self::Lavender => "Lavender",
            Self::Blue => "Blue",
            Self::Teal => "Teal",
            Self::Peach => "Peach",
            Self::Copper => "Copper",
            Self::Red => "Red",
        }
    }
}

/// What shape the library's grid is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tiles {
    /// 2:3 covers. The default, because the art a provider has for every
    /// title is a cover; a 16:9 still exists for fewer of them, and a grid of
    /// letterboxed covers was the desktop's weakest page.
    #[default]
    Posters,
    /// 16:9 stills: the provider's fanart, else a frame from the file.
    Stills,
}

impl Tiles {
    pub const ALL: [Self; 2] = [Self::Posters, Self::Stills];

    pub fn label(self) -> &'static str {
        match self {
            Self::Posters => "Posters",
            Self::Stills => "Stills",
        }
    }
}

/// The whole of the choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
// An older or hand-edited file is missing fields rather than invalid.
#[serde(default)]
pub struct Appearance {
    pub flavor: Flavor,
    pub accent: Accent,
    /// Whether an accent fill carries a little depth — the accent running into
    /// a shade of itself across a button or a seek bar — or is flat.
    ///
    /// Tonal only: a fill never runs from one hue into another, which is what
    /// made the old violet-to-orchid ramps read as decoration rather than
    /// design. [`Accent::PinkSky`] is the one exception, because a two-hue
    /// ramp is what choosing it means. Off by default; flat is the suite's
    /// look, and a slow ramp across a wide bar bands visibly on 6-bit panels.
    pub gradients: bool,
    /// The library grid's shape.
    pub tiles: Tiles,
    /// Which name a title goes by.
    pub names: crate::metadata::TitleNames,
}

fn appearance_file(dirs: &AppDirs) -> std::path::PathBuf {
    dirs.config.join("appearance.json")
}

/// The stored choice, or the defaults on a first run.
pub fn load(dirs: &AppDirs) -> Result<Appearance> {
    Ok(read_json::<Appearance>(&appearance_file(dirs))?.unwrap_or_default())
}

/// Write the choice.
pub fn save(dirs: &AppDirs, appearance: &Appearance) -> Result<()> {
    write_json(&appearance_file(dirs), appearance)
}

/// How opaque a scrim is at `t`, from its solid end (`0.0`) to its clear end
/// (`1.0`).
///
/// A scrim is what lets text sit on artwork, and the one place in the suite a
/// gradient is drawn on purpose. A linear fade ends in a visible line where it
/// meets the clear picture: the eye sees the sudden change of slope, not the
/// opacity. This curve is flat at both ends (`1 - smoothstep`), so neither end
/// has an edge.
///
/// Every client draws scrims from this curve, desktop per texel and the
/// mobile clients from [`SCRIM_STOPS`], so a banner fades the same way on each.
pub fn scrim(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// [`scrim`] sampled at nine evenly spaced stops, as `(position, opacity)`,
/// for clients whose gradient takes a list of stops (Compose, SwiftUI).
pub const SCRIM_STOPS: [(f32, f32); 9] = [
    (0.0, 1.0),
    (0.125, 0.957),
    (0.25, 0.844),
    (0.375, 0.684),
    (0.5, 0.5),
    (0.625, 0.316),
    (0.75, 0.156),
    (0.875, 0.043),
    (1.0, 0.0),
];

/// One colour, 8 bits a channel, with no alpha.
///
/// Deliberately not a front end's colour type: this module resolves a choice
/// into colours for *both* clients, and neither egui's `Color32` nor Compose's
/// `Color` may appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Packed as `0xAARRGGBB`, opaque — the form both Android and a hex literal
    /// want.
    pub const fn argb(self) -> u32 {
        0xff00_0000 | ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

/// One channel from sRGB into linear light, as the sRGB transfer function
/// defines it. Blending has to happen here: mixing sRGB values directly darkens
/// the middle of a ramp between saturated hues, which is exactly where a
/// gradient is looked at.
fn linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Back out of linear light, rounded to a channel.
fn gamma(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Blend two colours in linear light.
pub fn mix(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let blend = |from: u8, to: u8| gamma(linear(from) * (1.0 - t) + linear(to) * t);
    Rgb::new(
        blend(from.r, to.r),
        blend(from.g, to.g),
        blend(from.b, to.b),
    )
}

/// Relative luminance, in linear light, as WCAG defines it.
pub fn luminance(color: Rgb) -> f32 {
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG contrast between two opaque colours: 1.0 for a colour against itself,
/// 21.0 for black against white.
fn contrast(one: Rgb, other: Rgb) -> f32 {
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The least contrast a label gets anywhere along a gradient between two
/// colours. Sampled rather than solved: the worst point is not always an end,
/// because the ink can be brighter than one end and darker than the other.
pub fn worst_contrast(from: Rgb, to: Rgb, ink: Rgb) -> f32 {
    (0..=4)
        .map(|step| contrast(mix(from, to, step as f32 / 4.0), ink))
        .fold(f32::INFINITY, f32::min)
}

/// The least contrast the app will accept between a label and the fill under
/// it: WCAG AA for large text, which is what wears the accent — button labels,
/// a tab, a play glyph, never body copy.
pub const MIN_CONTRAST: f32 = 3.0;

/// The colour a picture leaves behind: its pixels averaged, with the vivid
/// ones counting for more. `None` for a picture with nothing opaque in it.
///
/// Weighted rather than a plain mean, because a plain mean of almost any
/// still is a brownish grey — the sky and the shadows cancel out, and the one
/// red coat that makes the poster what it is barely moves the sum. Each pixel
/// counts once plus [`MOOD_CHROMA_WEIGHT`] times its chroma, so a saturated
/// patch pulls the result towards itself without a single pixel deciding it.
///
/// Takes whatever pixels the caller has, in any order and at any size: every
/// client hands over a picture already squeezed small, so this is a few
/// hundred values, not a frame.
pub fn mood(pixels: impl IntoIterator<Item = (Rgb, u8)>) -> Option<Rgb> {
    let (mut r, mut g, mut b, mut total) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for (pixel, alpha) in pixels {
        // Half transparent and below is a logo's anti-aliasing or a poster's
        // rounded corner, not the picture.
        if alpha < 128 {
            continue;
        }
        let high = pixel.r.max(pixel.g).max(pixel.b);
        let low = pixel.r.min(pixel.g).min(pixel.b);
        let weight = 1.0 + MOOD_CHROMA_WEIGHT * f32::from(high - low) / 255.0;
        r += weight * f32::from(pixel.r);
        g += weight * f32::from(pixel.g);
        b += weight * f32::from(pixel.b);
        total += weight;
    }
    (total > 0.0).then(|| {
        let channel = |sum: f32| (sum / total).round().clamp(0.0, 255.0) as u8;
        Rgb::new(channel(r), channel(g), channel(b))
    })
}

/// How much more a fully saturated pixel counts towards [`mood`] than a grey
/// one.
pub const MOOD_CHROMA_WEIGHT: f32 = 4.0;

/// The brightest channel a mood is brought down to before it tints a dark
/// palette. Any brighter and the veil stops being the page with a hint of the
/// picture in it, and text over it loses the contrast the veil is there for.
pub const MOOD_DEPTH: u8 = 60;

/// How far a dark palette's veil moves from the page colour towards the mood.
pub const MOOD_DARK: f32 = 0.55;

/// The same for a light palette — much less, because a light page tinted as
/// hard reads as a coloured page rather than a lit one.
pub const MOOD_LIGHT: f32 = 0.12;

/// The page colour, tinted by a picture's [`mood`]: what the veils over a
/// title's backdrop are painted in, so each title's banner carries its own
/// light rather than the same flat colour.
///
/// On a dark palette the mood is first darkened to [`MOOD_DEPTH`] — only its
/// hue and saturation are wanted, never its brightness — and the page moves
/// [`MOOD_DARK`] of the way to it. A light palette moves [`MOOD_LIGHT`] of the
/// way to the mood as it is.
pub fn mood_veil(background: Rgb, mood: Rgb, light: bool) -> Rgb {
    if light {
        return mix(background, mood, MOOD_LIGHT);
    }
    let high = mood.r.max(mood.g).max(mood.b);
    if high == 0 {
        return mix(background, mood, MOOD_DARK);
    }
    let scale = f32::from(MOOD_DEPTH.min(high)) / f32::from(high);
    let deep = |channel: u8| (f32::from(channel) * scale).round() as u8;
    mix(
        background,
        Rgb::new(deep(mood.r), deep(mood.g), deep(mood.b)),
        MOOD_DARK,
    )
}

/// Every colour a client draws with, resolved from a [`Flavor`] and an
/// [`Accent`].
///
/// `Copy`, and small, so reading it is a lock and a memcpy rather than anything
/// a drawing loop has to think about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Behind everything: the page.
    pub background: Rgb,
    /// The bars above and below it.
    pub surface: Rgb,
    /// The deepest colour in the flavour — text fields, wells.
    pub sunken: Rgb,
    /// Tiles, rows, forms.
    pub card: Rgb,
    pub card_hover: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    /// The one strong colour.
    pub accent: Rgb,
    /// What a gradient in the accent runs into: a shade of the accent, never
    /// another hue (except for [`Accent::PinkSky`]). Equal to
    /// [`Self::accent`] when gradients are off.
    pub accent_alt: Rgb,
    /// The accent taken down into the background, for a pressed control and for
    /// selection behind text.
    pub accent_dim: Rgb,
    /// Ink that stays readable on top of the accent. Chosen by the accent's
    /// luminance, because Catppuccin's dark flavours have *pale* accents and
    /// white-on-pastel is not readable at 13 px.
    pub on_accent: Rgb,
    pub danger: Rgb,
    /// The small warm colour beside the accent: a new-episodes badge, a rating
    /// star, a focus ring's inner edge. The suite's copper on the Proton
    /// flavours. Never a fill under text, and never anything large.
    pub highlight: Rgb,
    /// Whether this is a light theme.
    pub light: bool,
}

/// One flavour's raw colours, named as Catppuccin names them.
///
/// Stored rather than computed: these are somebody else's palette, and the
/// point of using it is to use it exactly.
#[derive(Clone, Copy)]
struct Ramp {
    base: Rgb,
    mantle: Rgb,
    crust: Rgb,
    surface0: Rgb,
    surface1: Rgb,
    subtext0: Rgb,
    text: Rgb,
    pink: Rgb,
    mauve: Rgb,
    sky: Rgb,
    blue: Rgb,
    lavender: Rgb,
    teal: Rgb,
    peach: Rgb,
    yellow: Rgb,
    red: Rgb,
    light: bool,
}

const fn rgb(hex: u32) -> Rgb {
    Rgb::new(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// The suite's palette (`docs/DESIGN-TOKENS.md`), extended to the hues an
/// accent can be.
///
/// The neutrals are the plum-tinted blacks the suite's websites use, `mauve`
/// is its violet and `pink` the orchid its icons' faces run into, dark enough
/// that white stays readable across the whole gradient. The rest are chosen to
/// sit on the dark base at roughly the saturation the violet does, which is a
/// good deal hotter than any Catppuccin flavour.
const PROTON: Ramp = Ramp {
    base: rgb(0x100e17),
    mantle: rgb(0x17141f),
    crust: rgb(0x0b0910),
    surface0: rgb(0x1e1a29),
    surface1: rgb(0x2e283d),
    subtext0: rgb(0xaaa4bd),
    text: rgb(0xf4f2fa),
    pink: rgb(0xb65bd6),
    mauve: rgb(0x6d4aff),
    sky: rgb(0x4dd2ff),
    blue: rgb(0x3f6fe0),
    lavender: rgb(0xa48bff),
    teal: rgb(0x3fd6b8),
    peach: rgb(0xff9a4d),
    yellow: rgb(0xffd166),
    red: rgb(0xe05561),
    light: false,
};

/// The suite's light page: Proton Gallery's light scheme, with the same violet
/// and orchid as [`PROTON`], which stay readable under the pale ink.
const PROTON_LIGHT: Ramp = Ramp {
    base: rgb(0xfaf9fc),
    mantle: rgb(0xf4f1fa),
    crust: rgb(0xebe7f3),
    surface0: rgb(0xf1eef8),
    surface1: rgb(0xe3deed),
    subtext0: rgb(0x615970),
    text: rgb(0x1b1340),
    pink: rgb(0xb65bd6),
    mauve: rgb(0x6d4aff),
    sky: rgb(0x0a8fc7),
    blue: rgb(0x3b5bdb),
    lavender: rgb(0x7b5cf0),
    teal: rgb(0x0f8f7a),
    peach: rgb(0xd9711c),
    yellow: rgb(0xb7860b),
    red: rgb(0xb42339),
    light: true,
};

/// The suite's copper, as a scale (`docs/DESIGN-TOKENS.md`).
///
/// 400 is the lit band, what copper is on a dark page; 500 is copper on a light
/// one. The rest are for the clients that need a tint behind a copper label
/// (100, 200 on dark) or a copper border on a light page (600, 700). Before
/// this scale existed only the icons had the light and dark ends, and copper
/// in the app was a single flat value that went muddy wherever it was used.
pub mod copper {
    use super::{Rgb, rgb};

    pub const C100: Rgb = rgb(0xfbebdd);
    pub const C200: Rgb = rgb(0xf2c9aa);
    pub const C300: Rgb = rgb(0xe5b08d);
    pub const C400: Rgb = rgb(0xd69b77);
    pub const C500: Rgb = rgb(0xb47456);
    pub const C600: Rgb = rgb(0x8f5940);
    pub const C700: Rgb = rgb(0x703e30);
}

const COPPER_LIT: Rgb = copper::C400;
const COPPER: Rgb = copper::C500;

/// How far an accent's gradient partner is taken towards black. Enough to read
/// as light falling across the control, not enough to read as a second colour.
const TONAL_SHADE: f32 = 0.16;

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
    blue: rgb(0x1e66f5),
    lavender: rgb(0x7287fd),
    teal: rgb(0x179299),
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
    blue: rgb(0x8caaee),
    lavender: rgb(0xbabbf1),
    teal: rgb(0x81c8be),
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
    blue: rgb(0x8aadf4),
    lavender: rgb(0xb7bdf8),
    teal: rgb(0x8bd5ca),
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
    blue: rgb(0x89b4fa),
    lavender: rgb(0xb4befe),
    teal: rgb(0x94e2d5),
    peach: rgb(0xfab387),
    yellow: rgb(0xf9e2af),
    red: rgb(0xf38ba8),
    light: false,
};

/// Persona 5's menus: black, white, and a red that is not asking permission.
///
/// The three colours that matter are `crust`/`base` (black), `text` (white) and
/// `red` — everything between them is that same red bled into the black, which
/// is what keeps a card from reading as grey furniture in a palette that has no
/// grey in it. The other hues exist because an accent can be any of them; they
/// are pushed to full saturation to sit on a black page, which is the one thing
/// this palette has in common with [`PROTON`].
const PERSONA5: Ramp = Ramp {
    base: rgb(0x0a0708),
    mantle: rgb(0x140d0f),
    crust: rgb(0x000000),
    surface0: rgb(0x1d1215),
    surface1: rgb(0x33191d),
    subtext0: rgb(0xb59a9d),
    text: rgb(0xf5f2f2),
    pink: rgb(0xff2e63),
    mauve: rgb(0xb13bd6),
    sky: rgb(0x2ec7ff),
    blue: rgb(0x2f5ee0),
    lavender: rgb(0xa07bff),
    teal: rgb(0x12c2a0),
    peach: rgb(0xff7a1a),
    yellow: rgb(0xffc400),
    red: rgb(0xe60012),
    light: false,
};

impl Ramp {
    const fn of(flavor: Flavor) -> Self {
        match flavor {
            Flavor::Proton => PROTON,
            Flavor::ProtonLight => PROTON_LIGHT,
            Flavor::Latte => LATTE,
            Flavor::Frappe => FRAPPE,
            Flavor::Macchiato => MACCHIATO,
            Flavor::Mocha => MOCHA,
            Flavor::Persona5 => PERSONA5,
        }
    }

    /// The accent, and what a gradient in it runs into.
    ///
    /// A shade of the same hue, so a gradient is depth and not a second
    /// colour: a ramp from violet into orchid on every button and bar is the
    /// look of a template, not of a product. The exception is
    /// [`Accent::PinkSky`], which is the whole point of that entry.
    fn accents(&self, flavor: Flavor, accent: Accent) -> (Rgb, Rgb) {
        let color = match accent {
            Accent::PinkSky => return (self.pink, self.sky),
            Accent::Mauve => self.mauve,
            Accent::Pink => self.pink,
            Accent::Sky => self.sky,
            Accent::Lavender => self.lavender,
            Accent::Blue => self.blue,
            Accent::Teal => self.teal,
            Accent::Peach => self.peach,
            // The lit copper on a dark page, as the icons' faces are; on a
            // light page the deeper band, which holds white ink.
            Accent::Copper => match flavor {
                Flavor::Proton => COPPER_LIT,
                Flavor::ProtonLight => COPPER,
                _ => mix(self.peach, self.red, 0.25),
            },
            Accent::Red => self.red,
        };
        (color, mix(color, Rgb::BLACK, TONAL_SHADE))
    }
}

impl Palette {
    /// The default, spelled out as a constant so the global has something to
    /// hold before [`apply`] first runs — a `Ui` built during startup, or a
    /// panic message painted before the engine exists, still gets colours.
    pub const PROTON: Self = Self {
        background: PROTON.base,
        surface: PROTON.mantle,
        sunken: PROTON.crust,
        card: PROTON.surface0,
        card_hover: PROTON.surface1,
        border: PROTON.surface1,
        text: PROTON.text,
        muted: PROTON.subtext0,
        accent: PROTON.mauve,
        accent_alt: PROTON.mauve,
        accent_dim: rgb(0x5338c4),
        on_accent: PROTON.text,
        danger: PROTON.red,
        highlight: COPPER_LIT,
        light: false,
    };

    /// Resolve a choice into colours.
    pub fn resolve(appearance: Appearance) -> Self {
        let ramp = Ramp::of(appearance.flavor);
        let (accent, partner) = ramp.accents(appearance.flavor, appearance.accent);
        // Catppuccin's Latte accents are tuned to be *read*, on a light page,
        // at body-text weight — which makes them mid-luminance, and a
        // mid-luminance fill is one that neither black nor white sits on. They
        // are taken down in value here, and only here: the hue and the
        // saturation are Latte's, so it still looks like Latte, and a label on
        // a button is legible.
        let latte = appearance.flavor == Flavor::Latte;
        let deepen = |color: Rgb| {
            if latte {
                mix(color, Rgb::BLACK, 0.30)
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
            mix(ramp.text, Rgb::BLACK, 0.35)
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
            highlight: match appearance.flavor {
                Flavor::Proton => COPPER_LIT,
                Flavor::ProtonLight => COPPER,
                Flavor::Persona5 => ramp.yellow,
                _ => deepen(ramp.peach),
            },
            light: ramp.light,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &std::path::Path) -> AppDirs {
        AppDirs {
            config: root.to_path_buf(),
            data: root.to_path_buf(),
            cache: root.to_path_buf(),
        }
    }

    #[test]
    fn a_picture_of_one_colour_has_that_colour_as_its_mood() {
        let teal = Rgb::new(20, 140, 150);
        assert_eq!(mood(std::iter::repeat_n((teal, 255), 16)), Some(teal));
    }

    #[test]
    fn a_vivid_patch_outweighs_as_much_grey_in_the_mood() {
        let red = Rgb::new(220, 30, 30);
        let grey = Rgb::new(120, 120, 120);
        let found = mood([(red, 255), (grey, 255)]).unwrap();
        assert!(found.r > 180, "{found:?}");
        assert!(found.g < 60 && found.b < 60, "{found:?}");
    }

    #[test]
    fn transparent_pixels_do_not_count_towards_the_mood() {
        let blue = Rgb::new(30, 60, 200);
        assert_eq!(
            mood([(blue, 255), (Rgb::new(255, 255, 255), 0)]),
            Some(blue)
        );
        assert_eq!(mood([(blue, 10)]), None);
        assert_eq!(mood(std::iter::empty()), None);
    }

    #[test]
    fn a_dark_veil_takes_the_moods_hue_but_never_its_brightness() {
        let background = Rgb::new(11, 11, 15);
        let veil = mood_veil(background, Rgb::new(255, 240, 40), false);
        // Warmer than the page…
        assert!(veil.r > background.r && veil.g > background.g, "{veil:?}");
        // …and still dark enough to be a page under white text.
        assert!(luminance(veil) < 0.05, "{veil:?}");
        assert!(contrast(veil, Rgb::new(255, 255, 255)) > 12.0, "{veil:?}");
    }

    #[test]
    fn a_light_veil_moves_only_a_little_way_to_the_mood() {
        let background = Rgb::new(239, 241, 245);
        let veil = mood_veil(background, Rgb::new(200, 20, 20), true);
        assert!(veil.r >= veil.g, "{veil:?}");
        assert!(luminance(veil) > 0.6, "{veil:?}");
    }

    #[test]
    fn a_first_run_gets_the_palette_the_app_shipped_with() {
        let appearance = Appearance::default();
        assert_eq!(appearance.flavor, Flavor::Proton);
        assert_eq!(appearance.accent, Accent::Mauve);
        assert!(!appearance.gradients);
    }

    #[test]
    fn what_was_saved_is_what_loads() {
        let temp = std::env::temp_dir().join(format!("pstr-appearance-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let dirs = dirs(&temp);

        let appearance = Appearance {
            flavor: Flavor::Mocha,
            accent: Accent::PinkSky,
            gradients: false,
            ..Appearance::default()
        };
        save(&dirs, &appearance).unwrap();
        assert_eq!(load(&dirs).unwrap(), appearance);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn a_file_written_before_a_field_existed_keeps_the_rest() {
        let temp = std::env::temp_dir().join(format!("pstr-appearance-old-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let dirs = dirs(&temp);

        std::fs::write(appearance_file(&dirs), r#"{"flavor":"mocha"}"#).unwrap();
        let appearance = load(&dirs).unwrap();
        assert_eq!(appearance.flavor, Flavor::Mocha);
        assert_eq!(appearance.accent, Accent::default());
        assert!(!appearance.gradients);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn only_proton_light_and_latte_are_light_themes() {
        for flavor in Flavor::ALL {
            assert_eq!(
                flavor.is_light(),
                matches!(flavor, Flavor::ProtonLight | Flavor::Latte)
            );
        }
    }

    #[test]
    fn the_default_palette_is_the_one_spelled_out_for_startup() {
        assert_eq!(Palette::resolve(Appearance::default()), Palette::PROTON);
    }

    #[test]
    fn the_suites_violet_wears_white_ink_on_both_proton_pages() {
        for flavor in [Flavor::Proton, Flavor::ProtonLight] {
            let palette = Palette::resolve(Appearance {
                flavor,
                ..Appearance::default()
            });
            assert_eq!(palette.accent, rgb(0x6d4aff), "{flavor:?}");
            assert!(luminance(palette.on_accent) > 0.8, "{flavor:?}");
        }
    }

    #[test]
    fn copper_stays_a_highlight_the_page_can_see() {
        for flavor in Flavor::ALL {
            let palette = Palette::resolve(Appearance {
                flavor,
                ..Appearance::default()
            });
            assert!(
                contrast(palette.highlight, palette.background) >= MIN_CONTRAST,
                "{flavor:?}: {:.2}",
                contrast(palette.highlight, palette.background),
            );
        }
    }

    #[test]
    fn every_flavour_and_accent_resolves_to_readable_ink() {
        for flavor in Flavor::ALL {
            for accent in Accent::ALL {
                let palette = Palette::resolve(Appearance {
                    flavor,
                    accent,
                    gradients: true,
                    ..Appearance::default()
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
    fn persona_5_is_black_with_that_red_in_it() {
        let palette = Palette::resolve(Appearance {
            flavor: Flavor::Persona5,
            accent: Accent::Red,
            gradients: false,
            ..Appearance::default()
        });
        assert_eq!(palette.accent, PERSONA5.red);
        assert!(!palette.light);
        // Black enough that the artwork is the brightest thing on the page:
        // darker than the palette this app shipped with, which is the other
        // near-black one.
        assert!(luminance(palette.background) < luminance(PROTON.base));
    }

    #[test]
    fn every_flavour_can_wear_its_own_warning_colour() {
        for flavor in Flavor::ALL {
            let palette = Palette::resolve(Appearance {
                flavor,
                accent: Accent::Red,
                gradients: true,
                ..Appearance::default()
            });
            // The accent is the flavour's red — deepened on the light flavour,
            // where it would otherwise be too bright to put a label on.
            assert!(palette.accent.r > palette.accent.b, "{flavor:?}");
            assert_ne!(palette.accent, palette.background, "{flavor:?}");
        }
    }

    #[test]
    fn a_gradient_is_a_shade_of_the_accent_and_not_a_second_hue() {
        for flavor in Flavor::ALL {
            for accent in Accent::ALL {
                if accent == Accent::PinkSky {
                    continue;
                }
                let palette = Palette::resolve(Appearance {
                    flavor,
                    accent,
                    gradients: true,
                    ..Appearance::default()
                });
                let (a, b) = (palette.accent, palette.accent_alt);
                assert!(luminance(b) < luminance(a), "{flavor:?}/{accent:?}");
                // Same hue: each channel is scaled by the same factor, so
                // their ordering is unchanged.
                let order = |c: Rgb| {
                    let mut v = [(c.r, 0), (c.g, 1), (c.b, 2)];
                    v.sort();
                    v.map(|(_, i)| i)
                };
                let (oa, ob) = (order(a), order(b));
                assert_eq!(oa[2], ob[2], "{flavor:?}/{accent:?}: {a:?} → {b:?}");
            }
        }
    }

    #[test]
    fn a_scrim_falls_from_solid_to_clear_without_a_step() {
        let samples: Vec<f32> = (0..=100).map(|i| scrim(i as f32 / 100.0)).collect();
        assert_eq!(samples[0], 1.0);
        assert_eq!(samples[100], 0.0);
        assert!(samples.windows(2).all(|pair| pair[1] <= pair[0]));
        // Flat at both ends: the first and last steps are the smallest.
        let steps: Vec<f32> = samples.windows(2).map(|pair| pair[0] - pair[1]).collect();
        assert!(steps[0] < steps[50] && steps[99] < steps[50]);
    }

    #[test]
    fn the_listed_scrim_stops_are_the_curve() {
        for (position, opacity) in SCRIM_STOPS {
            assert!((scrim(position) - opacity).abs() < 0.001, "{position}");
        }
    }

    #[test]
    fn copper_as_an_accent_is_the_lit_band_on_the_dark_page() {
        let palette = Palette::resolve(Appearance {
            accent: Accent::Copper,
            ..Appearance::default()
        });
        assert_eq!(palette.accent, copper::C400);
        assert!(
            luminance(palette.on_accent) < 0.2,
            "{:?}",
            palette.on_accent
        );
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
    fn a_blend_stays_between_the_two_colours_it_blends() {
        let (from, to) = (Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert_eq!(mix(from, to, 0.0), from);
        assert_eq!(mix(from, to, 1.0), to);
        assert!(luminance(mix(from, to, 0.5)) > luminance(from));
        assert!(luminance(mix(from, to, 0.5)) < luminance(to));
    }
}
