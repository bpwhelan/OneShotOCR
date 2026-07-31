//! Tiny persisted configuration: the global hotkey and OCR language.
//!
//! Stored as plain `key=value` lines at `%APPDATA%\oneshot-ocr\config.txt`.
//! The hotkey round-trips through [`HotKey::into_string`] / [`HotKey::from_str`],
//! which supports F1-F24 (including F13-F24).

use std::path::PathBuf;
use std::str::FromStr;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

const CONFIG_DIRECTORY: &str = "oneshot-ocr";
const LEGACY_CONFIG_DIRECTORY: &str = "lens_adhoc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrEngine {
    GoogleLens,
    #[cfg(feature = "oneocr")]
    OneOcr,
    #[cfg(feature = "screenai")]
    ScreenAi,
}

impl OcrEngine {
    pub const fn config_name(self) -> &'static str {
        match self {
            Self::GoogleLens => "google_lens",
            #[cfg(feature = "oneocr")]
            Self::OneOcr => "oneocr",
            #[cfg(feature = "screenai")]
            Self::ScreenAi => "screenai",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::GoogleLens => "Google Lens",
            #[cfg(feature = "oneocr")]
            Self::OneOcr => "OneOCR",
            #[cfg(feature = "screenai")]
            Self::ScreenAi => "ScreenAI",
        }
    }

    pub fn from_config_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google_lens" | "lens" | "glens" => Some(Self::GoogleLens),
            #[cfg(feature = "oneocr")]
            "oneocr" => Some(Self::OneOcr),
            #[cfg(feature = "screenai")]
            "screenai" | "screen_ai" => Some(Self::ScreenAi),
            _ => None,
        }
    }
}

pub struct Config {
    pub hotkey: HotKey,
    pub language: String,
    pub engine: OcrEngine,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyO),
            language: "ja".to_string(),
            engine: OcrEngine::GoogleLens,
        }
    }
}

impl Config {
    /// Load config, falling back to defaults for anything missing or invalid.
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let path = config_path();
        let path = if path.is_file() {
            path
        } else {
            legacy_config_path()
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return cfg;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "hotkey" => {
                    if let Ok(hk) = HotKey::from_str(value.trim()) {
                        cfg.hotkey = hk;
                    }
                }
                "language" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        cfg.language = v.to_string();
                    }
                }
                "engine" => {
                    if let Some(engine) = OcrEngine::from_config_name(value) {
                        cfg.engine = engine;
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    /// Persist the config, creating the directory if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = format!(
            "hotkey={}\nlanguage={}\nengine={}\n",
            self.hotkey.into_string(),
            self.language,
            self.engine.config_name(),
        );
        std::fs::write(path, body)
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.join(CONFIG_DIRECTORY).join("config.txt")
}

/// The original project name's settings location, kept as a read-only fallback
/// so a rename does not discard an existing hotkey or OCR preference.
fn legacy_config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.join(LEGACY_CONFIG_DIRECTORY).join("config.txt")
}

/// Human-friendly label for a hotkey, e.g. `Ctrl+Shift+O`.
pub fn pretty_hotkey(hk: &HotKey) -> String {
    (*hk)
        .into_string()
        .split('+')
        .map(|t| match t {
            "shift" => "Shift".to_string(),
            "control" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "super" => "Win".to_string(),
            other => {
                if let Some(rest) = other.strip_prefix("Key") {
                    rest.to_string()
                } else if let Some(rest) = other.strip_prefix("Digit") {
                    rest.to_string()
                } else {
                    other.to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}
