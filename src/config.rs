use std::{fs, path::PathBuf};

pub struct Keymaps {
    pub quit: String,
    pub create_file: String,
    pub create_folder: String,
    pub rename: String,
    pub open: String,
    pub copy: String,
    pub cut: String,
    pub paste: String,
    pub trash: String,
    pub sort: String,
    pub toggle_hidden: String,
    pub focus: String,
    pub pin: String,
    pub unpin: String,
}

pub struct Config {
    pub keymaps: Keymaps,
}

impl Config {
    // Create ~/.config/alice/config.toml if missing
    pub fn ensure_config_exists() {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fren");

        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        }

        let config_path = config_dir.join("config.toml");

        if !config_path.exists() {
            let default_config = "quit = \"q\"\n\
                 open = \"enter\"\n\
                 focus = \"tab\"\n\
                 copy = \"c\"\n\
                 cut = \"x\"\n\
                 paste = \"v\"\n\
                 trash = \"d\"\n\
                 sort = \"s\"\n\
                 toggle_hidden = \".\"\n\
                 create_file = \"n\"\n\
                 create_folder = \"f\"\n\
                 rename = \"r\"\n\
                 pin = \"p\"\n\
                 unpin = \"u\"\n";

            fs::write(&config_path, default_config).expect("Failed to create default config.toml");
        }
    }

    // Load config from ~/.config/alice/config.toml
    pub fn load() -> Self {
        Self::ensure_config_exists();

        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fren")
            .join("config.toml");

        let mut config = Self::default();

        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                let line = line.trim();

                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"');

                    match key {
                        "quit" => config.keymaps.quit = value.to_string(),
                        "create_file" => config.keymaps.create_file = value.to_string(),
                        "create_folder" => config.keymaps.create_folder = value.to_string(),
                        "rename" => config.keymaps.rename = value.to_string(),
                        "open" => config.keymaps.open = value.to_string(),
                        "copy" => config.keymaps.copy = value.to_string(),
                        "cut" => config.keymaps.cut = value.to_string(),
                        "paste" => config.keymaps.paste = value.to_string(),
                        "trash" => config.keymaps.trash = value.to_string(),
                        "sort" => config.keymaps.sort = value.to_string(),
                        "toggle_hidden" => config.keymaps.toggle_hidden = value.to_string(),
                        "focus" => config.keymaps.focus = value.to_string(),
                        "pin" => config.keymaps.pin = value.to_string(),
                        "unpin" => config.keymaps.unpin = value.to_string(),
                        _ => {}
                    }
                }
            }
        }

        config
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keymaps: Keymaps {
                quit: "q".into(),
                create_file: "n".into(),
                create_folder: "f".into(),
                rename: "r".into(),
                open: "enter".into(),
                copy: "c".into(),
                cut: "x".into(),
                paste: "v".into(),
                trash: "d".into(),
                sort: "s".into(),
                toggle_hidden: ".".into(),
                focus: "tab".into(),
                pin: "p".into(),
                unpin: "u".into(),
            },
        }
    }
}
