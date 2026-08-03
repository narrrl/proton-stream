//! What the window wears.
//!
//! Only the *choice* lives here — the colours themselves are `pstr-app`'s, and
//! nothing outside the window has an opinion about them. This is stored beside
//! the playback preferences and read the same way: a file that will not parse is
//! a line in the log and a fall back to the defaults, because a broken theme
//! file must never be a reason the library does not open.

use crate::config::{AppDirs, read_json, write_json};
use crate::error::Result;

/// A palette family: Catppuccin's four, plus the one this app shipped with.
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
}

impl Flavor {
    pub const ALL: [Self; 5] = [
        Self::Proton,
        Self::Mocha,
        Self::Macchiato,
        Self::Frappe,
        Self::Latte,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Proton => "Proton",
            Self::Latte => "Catppuccin Latte",
            Self::Frappe => "Catppuccin Frappé",
            Self::Macchiato => "Catppuccin Macchiato",
            Self::Mocha => "Catppuccin Mocha",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Proton => "Near-black, so the artwork is the only bright thing on screen",
            Self::Latte => "Light. The only one that is",
            Self::Frappe => "Dark, warm, the lowest contrast of the three",
            Self::Macchiato => "Dark, between Frappé and Mocha",
            Self::Mocha => "Dark, the deepest of Catppuccin's three",
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
}

impl Accent {
    pub const ALL: [Self; 8] = [
        Self::Mauve,
        Self::Pink,
        Self::Sky,
        Self::PinkSky,
        Self::Lavender,
        Self::Blue,
        Self::Teal,
        Self::Peach,
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
}
