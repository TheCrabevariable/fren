use ratatui::style::Color;
use std::{collections::HashMap, fs, path::PathBuf};

pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub border: Color,
    pub focus_border: Color,
    pub directory: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub muted: Color,
}

impl Theme {
    /// Ensure ~/.config/fren/theme.toml exists
    pub fn ensure_config_exists() {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fren");

        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        }

        let theme_path = config_dir.join("theme.toml");

        if !theme_path.exists() {
            let default_theme = r##"
                background = "#0f1419"
                foreground = "#e6edf3"

                border = "#26323d"
                focus_border = "#00d4ff"
                muted = "#5c6a72"

                directory = "#4fc3f7"

                status_bg = "#0b1014"
                status_fg = "#9fb3c8"
            "##;

            fs::write(&theme_path, default_theme.trim())
                .expect("Failed to create default theme.toml");
        }
    }

    /// Load theme from ~/.config/fren/theme.toml
    pub fn load() -> Self {
        Self::ensure_config_exists();

        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fren")
            .join("theme.toml");

        let mut theme = Self::default();

        if let Ok(content) = fs::read_to_string(path) {
            let values = parse_toml_like(&content);

            if let Some(v) = values.get("background") {
                theme.background = parse_color(v);
            }
            if let Some(v) = values.get("foreground") {
                theme.foreground = parse_color(v);
            }
            if let Some(v) = values.get("border") {
                theme.border = parse_color(v);
            }
            if let Some(v) = values.get("focus_border") {
                theme.focus_border = parse_color(v);
            }
            if let Some(v) = values.get("directory") {
                theme.directory = parse_color(v);
            }
            if let Some(v) = values.get("status_bg") {
                theme.status_bg = parse_color(v);
            }
            if let Some(v) = values.get("status_fg") {
                theme.status_fg = parse_color(v);
            }
            if let Some(v) = values.get("muted") {
                theme.muted = parse_color(v);
            }
        }

        theme
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color::Black,
            foreground: Color::White,
            border: Color::Gray,
            focus_border: Color::Yellow,
            directory: Color::Blue,
            status_bg: Color::DarkGray,
            status_fg: Color::White,
            muted: Color::Blue,
        }
    }
}

/// key = "value"
fn parse_toml_like(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            map.insert(key, value);
        }
    }

    map
}

// Supports:
// - Hex (#RRGGBB)
// - Named colors
fn parse_color(input: &str) -> Color {
    let input = input.trim().to_lowercase();

    // HEX
    if input.starts_with('#') {
        let hex = input.trim_start_matches('#');

        if hex.len() == 6 {
            if let Ok(value) = u32::from_str_radix(hex, 16) {
                let r = ((value >> 16) & 0xff) as u8;
                let g = ((value >> 8) & 0xff) as u8;
                let b = (value & 0xff) as u8;
                return Color::Rgb(r, g, b);
            }
        }
    }

    match input.as_str() {
        "black" => Color::Black,
        "white" => Color::White,
        "red" => Color::Red,
        "green" => Color::Green,
        "blue" => Color::Blue,
        "yellow" => Color::Yellow,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" => Color::Gray,
        "darkgray" => Color::DarkGray,
        _ => Color::Reset,
    }
}
