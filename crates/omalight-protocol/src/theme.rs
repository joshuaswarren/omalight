use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// sRGB channel triple parsed from an Omarchy `colors.toml` hex value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    /// Path the colors were read from, or "built-in" when Omarchy is not installed.
    pub source: String,
    pub background: Color,
    pub darker_background: Color,
    pub lighter_background: Color,
    pub foreground: Color,
    pub dark_foreground: Color,
    pub accent: Color,
    pub red: Color,
    pub muted: Color,
}

impl Theme {
    pub fn builtin() -> Self {
        Self {
            name: "built-in".into(),
            source: "built-in".into(),
            background: Color::rgb(22, 22, 22),
            darker_background: Color::rgb(14, 14, 14),
            lighter_background: Color::rgb(36, 36, 36),
            foreground: Color::rgb(230, 230, 230),
            dark_foreground: Color::rgb(138, 138, 138),
            accent: Color::rgb(214, 161, 90),
            red: Color::rgb(210, 91, 74),
            muted: Color::rgb(92, 92, 92),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawTheme {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    bg: Option<String>,
    #[serde(default)]
    darker_background: Option<String>,
    #[serde(default)]
    darker_bg: Option<String>,
    #[serde(default)]
    lighter_background: Option<String>,
    #[serde(default)]
    lighter_bg: Option<String>,
    #[serde(default)]
    foreground: Option<String>,
    #[serde(default)]
    fg: Option<String>,
    #[serde(default)]
    dark_foreground: Option<String>,
    #[serde(default)]
    dark_fg: Option<String>,
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    color4: Option<String>,
    #[serde(default)]
    red: Option<String>,
    #[serde(default)]
    color1: Option<String>,
    #[serde(default)]
    muted: Option<String>,
    #[serde(default)]
    color8: Option<String>,
}

pub fn parse_colors_toml(text: &str) -> Result<Theme, String> {
    let raw: RawTheme = toml::from_str(text).map_err(|e| e.to_string())?;
    let base = Theme::builtin();
    let pick = |canonical: &Option<String>, legacy: &Option<String>, fallback: Color| -> Color {
        canonical
            .as_deref()
            .or(legacy.as_deref())
            .and_then(parse_hex)
            .unwrap_or(fallback)
    };
    Ok(Theme {
        name: raw.mode.unwrap_or_else(|| "omarchy".into()),
        source: "colors.toml".into(),
        background: pick(&raw.background, &raw.bg, base.background),
        darker_background: pick(
            &raw.darker_background,
            &raw.darker_bg,
            base.darker_background,
        ),
        lighter_background: pick(
            &raw.lighter_background,
            &raw.lighter_bg,
            base.lighter_background,
        ),
        foreground: pick(&raw.foreground, &raw.fg, base.foreground),
        dark_foreground: pick(&raw.dark_foreground, &raw.dark_fg, base.dark_foreground),
        accent: pick(&raw.accent, &raw.color4, base.accent),
        red: pick(&raw.red, &raw.color1, base.red),
        muted: pick(&raw.muted, &raw.color8, base.muted),
    })
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color { r, g, b })
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Omarchy writes the active theme here when `omarchy-theme-set` runs. Older checkouts used
/// `~/.config/omarchy/current/theme`. A `theme.name` file points at a theme directory under
/// `~/.config/omarchy/themes` or `/usr/share/omarchy/themes`.
pub fn active_theme_paths() -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let mut paths = vec![
        home.join(".local/state/omarchy/current/theme/colors.toml"),
        home.join(".config/omarchy/current/theme/colors.toml"),
    ];
    let name_file = home.join(".local/state/omarchy/current/theme.name");
    if let Ok(name) = fs::read_to_string(&name_file) {
        let name = name.trim();
        if !name.is_empty() && !name.contains(['/', '\\']) {
            paths.push(
                home.join(".config/omarchy/themes")
                    .join(name)
                    .join("colors.toml"),
            );
            paths.push(
                PathBuf::from("/usr/share/omarchy/themes")
                    .join(name)
                    .join("colors.toml"),
            );
        }
    }
    paths
}

pub fn load_theme() -> Theme {
    for path in active_theme_paths() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(mut theme) = parse_colors_toml(&text) {
                theme.source = path.display().to_string();
                if let Some(name) = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                {
                    if name != "theme" {
                        theme.name = name.to_string();
                    }
                }
                return theme;
            }
        }
    }
    Theme::builtin()
}

/// Polls the active Omarchy theme. Theme switches replace files, so a modification time is enough.
pub struct ThemeWatch {
    pub theme: Theme,
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    last: Instant,
}

impl ThemeWatch {
    pub fn new() -> Self {
        let theme = load_theme();
        let path = active_theme_paths().into_iter().find(|p| p.is_file());
        let mtime = path.as_deref().and_then(mtime_of);
        Self {
            theme,
            path,
            mtime,
            last: Instant::now(),
        }
    }

    /// Returns true when the palette changed.
    pub fn poll(&mut self) -> bool {
        if self.last.elapsed() < std::time::Duration::from_millis(400) {
            return false;
        }
        self.last = Instant::now();
        let path = active_theme_paths().into_iter().find(|p| p.is_file());
        let mtime = path.as_deref().and_then(mtime_of);
        if path == self.path && mtime == self.mtime {
            return false;
        }
        self.path = path;
        self.mtime = mtime;
        self.theme = load_theme();
        true
    }
}

impl Default for ThemeWatch {
    fn default() -> Self {
        Self::new()
    }
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_and_legacy_keys() {
        let text = r##"
mode = "dark"
accent = "#7aa2f7"
background = "#1a1b26"
fg = "#a9b1d6"
red = "#f7768e"
"##;
        let theme = parse_colors_toml(text).unwrap();
        assert_eq!(theme.accent, Color::rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(theme.background, Color::rgb(0x1a, 0x1b, 0x26));
        assert_eq!(theme.foreground, Color::rgb(0xa9, 0xb1, 0xd6));
        assert_eq!(theme.red, Color::rgb(0xf7, 0x76, 0x8e));
        assert_eq!(theme.muted, Theme::builtin().muted);
    }
}
