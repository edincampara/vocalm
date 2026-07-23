//! Persistent app settings (JSON in the platform config dir).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub recordings_dir: PathBuf,
    /// 0 = DeepFilter, 1 = Rnnoise, 2 = Bypass (same encoding as pipeline)
    pub engine_kind: u32,
    pub atten_db: f32,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    #[serde(default)]
    pub spk_input: Option<String>,
    #[serde(default)]
    pub spk_output: Option<String>,
    #[serde(default)]
    pub spk_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let docs = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            recordings_dir: docs.join("Vocalm Meetings"),
            engine_kind: 0,
            atten_db: 100.0,
            input_device: None,
            output_device: None,
            spk_input: None,
            spk_output: None,
            spk_enabled: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Vocalm")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn models_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Vocalm")
        .join("models")
}

impl AppConfig {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(config_path(), s);
        }
    }
}
