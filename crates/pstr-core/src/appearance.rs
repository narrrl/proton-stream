//! What the window wears.
//!
//! Only the *choice* lives here — the colours themselves are `pstr-app`'s, and
//! nothing outside the window has an opinion about them. This is stored beside
//! the playback preferences and read the same way: a file that will not parse is
//! a line in the log and a fall back to the defaults, because a broken theme
//! file must never be a reason the library does not open.

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// A palette family: Catppuccin's four, the one this app shipped with, and one
/// borrowed from a menu screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavor {
    /// Near-black and Proton purple. The default, and the only one that is not
    /// somebody else's palette.
    #[default]
    Proton,
    /// Catppuccin Latte — the light one.
    Latte,
    Frappe,
    Macchiato,
    Mocha,
    /// Black, white and one loud red — the Persona 5 menus.
    Persona5,
}

impl Flavor {
    pub const ALL: [Self; 6] = [
        Self::Proton,
        Self::Mocha,
        Self::Macchiato,
        Self::Frappe,
        Self::Latte,
        Self::Persona5,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Proton => "Proton",
            Self::Latte => "Catppuccin Latte",
            Self::Frappe => "Catppuccin Frappé",
            Self::Macchiato => "Catppuccin Macchiato",
            Self::Mocha => "Catppuccin Mocha",
            Self::Persona5 => "Persona 5",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Proton => "Near-black, so the artwork is the only bright thing on screen",
            Self::Latte => "Light. The only one that is",
            Self::Frappe => "Dark, warm, the lowest contrast of the three",
            Self::Macchiato => "Dark, between Frappé and Mocha",
            Self::Mocha => "Dark, the deepest of Catppuccin's three",
            Self::Persona5 => "Black and white with one loud red — pick the Red accent for it",
        }
    }

    /// Whether this flavour is a light theme. Latte, and only Latte.
    ///
    /// It decides more than the colours: egui keeps a style per theme and the
    /// platform draws the title bar from the window's own, so both have to be
    /// told which of the two this is.
    pub fn is_light(self) -> bool {
        matches!(self, Self::Latte)
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
    /// The flavour's warning colour, used on purpose. Every flavour has one
    /// because everything here needs a colour for a failure, and in
    /// [`Flavor::Persona5`] it is the point of the palette rather than an
    /// afterthought.
    Red,
}

impl Accent {
    pub const ALL: [Self; 9] = [
        Self::Mauve,
        Self::Pink,
        Self::Sky,
        Self::PinkSky,
        Self::Lavender,
        Self::Blue,
        Self::Teal,
        Self::Peach,
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
            Self::Red => "Red",
        }
    }
}

/// The whole of the choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
// An older or hand-edited file is missing fields rather than invalid.
#[serde(default)]
pub struct Appearance {
    pub flavor: Flavor,
    pub accent: Accent,
    /// Whether the accent is painted as a gradient — the seek bar, the play
    /// button, the top bar — or flat.
    ///
    /// A switch rather than a fixed decision because a gradient is the first
    /// thing to go wrong on a screen that cannot show it: on 6-bit panels a
    /// slow ramp across a wide bar bands visibly, and flat is better than
    /// striped.
    pub gradients: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            flavor: Flavor::default(),
            accent: Accent::default(),
            gradients: true,
        }
    }
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
    /// What a gradient in the accent runs into. Equal to [`Self::accent`] when
    /// the accent is a single hue and gradients would have nothing to do.
    pub accent_alt: Rgb,
    /// The accent taken down into the background, for a pressed control and for
    /// selection behind text.
    pub accent_dim: Rgb,
    /// Ink that stays readable on top of the accent. Chosen by the accent's
    /// luminance, because Catppuccin's dark flavours have *pale* accents and
    /// white-on-pastel is not readable at 13 px.
    pub on_accent: Rgb,
    pub danger: Rgb,
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
    sapphire: Rgb,
    blue: Rgb,
    lavender: Rgb,
    teal: Rgb,
    green: Rgb,
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
    sapphire: rgb(0x1f9bd0),
    blue: rgb(0x2f5ee0),
    lavender: rgb(0xa07bff),
    teal: rgb(0x12c2a0),
    green: rgb(0x2fbf4f),
    peach: rgb(0xff7a1a),
    yellow: rgb(0xffc400),
    red: rgb(0xe60012),
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
            Flavor::Persona5 => PERSONA5,
        }
    }

    /// The accent, and the hue a gradient in it runs into.
    ///
    /// The partners are neighbours on the wheel rather than complements: a
    /// gradient across half the spectrum passes through a colour that belongs
    /// to neither end, and on a seek bar that reads as a bug. The exception is
    /// [`Accent::PinkSky`], which is the whole point of that entry.
    const fn accents(&self, accent: Accent) -> (Rgb, Rgb) {
        match accent {
            Accent::Mauve => (self.mauve, self.blue),
            Accent::Pink => (self.pink, self.mauve),
            Accent::Sky => (self.sky, self.sapphire),
            Accent::PinkSky => (self.pink, self.sky),
            Accent::Lavender => (self.lavender, self.blue),
            Accent::Blue => (self.blue, self.sapphire),
            Accent::Teal => (self.teal, self.green),
            Accent::Peach => (self.peach, self.yellow),
            Accent::Red => (self.red, self.pink),
        }
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
        let deepen = |color: Rgb| {
            if ramp.light {
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
    fn a_first_run_gets_the_palette_the_app_shipped_with() {
        let appearance = Appearance::default();
        assert_eq!(appearance.flavor, Flavor::Proton);
        assert_eq!(appearance.accent, Accent::Mauve);
        assert!(appearance.gradients);
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
        assert!(appearance.gradients);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn only_latte_is_a_light_theme() {
        for flavor in Flavor::ALL {
            assert_eq!(flavor.is_light(), flavor == Flavor::Latte);
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
            });
            // The accent is the flavour's red — deepened on the light flavour,
            // where it would otherwise be too bright to put a label on.
            assert!(palette.accent.r > palette.accent.b, "{flavor:?}");
            assert_ne!(palette.accent, palette.background, "{flavor:?}");
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
    fn a_blend_stays_between_the_two_colours_it_blends() {
        let (from, to) = (Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert_eq!(mix(from, to, 0.0), from);
        assert_eq!(mix(from, to, 1.0), to);
        assert!(luminance(mix(from, to, 0.5)) > luminance(from));
        assert!(luminance(mix(from, to, 0.5)) < luminance(to));
    }
}
