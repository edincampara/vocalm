//! Meeting folders: one directory per recording with audio.wav, transcript.txt and meta.json.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const AUDIO_FILE: &str = "audio.wav";
pub const TRANSCRIPT_FILE: &str = "transcript.txt";
pub const META_FILE: &str = "meta.json";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct MeetingMeta {
    pub title: String,
    pub participants: Vec<String>,
    pub created: String,
    pub duration_secs: f32,
}

#[derive(Clone)]
pub struct Meeting {
    pub dir: PathBuf,
    pub meta: MeetingMeta,
    pub has_audio: bool,
    pub has_transcript: bool,
}

pub fn create_meeting(recordings_dir: &Path) -> Result<Meeting> {
    let now = chrono::Local::now();
    let name = now.format("%Y-%m-%d %H.%M.%S").to_string();
    let dir = recordings_dir.join(&name);
    std::fs::create_dir_all(&dir).context("create meeting dir")?;
    let meta = MeetingMeta {
        title: format!("Meeting {}", now.format("%b %-d, %H:%M")),
        participants: vec![],
        created: now.to_rfc3339(),
        duration_secs: 0.0,
    };
    let m = Meeting {
        dir,
        meta,
        has_audio: false,
        has_transcript: false,
    };
    save_meta(&m)?;
    Ok(m)
}

pub fn save_meta(m: &Meeting) -> Result<()> {
    let s = serde_json::to_string_pretty(&m.meta)?;
    std::fs::write(m.dir.join(META_FILE), s)?;
    Ok(())
}

pub fn list_meetings(recordings_dir: &Path) -> Vec<Meeting> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(recordings_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let has_audio = dir.join(AUDIO_FILE).exists();
        let meta_path = dir.join(META_FILE);
        if !has_audio && !meta_path.exists() {
            continue; // unrelated folder
        }
        let meta: MeetingMeta = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| MeetingMeta {
                title: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ..Default::default()
            });
        out.push(Meeting {
            has_transcript: dir.join(TRANSCRIPT_FILE).exists(),
            dir,
            meta,
            has_audio,
        });
    }
    // newest first (folder names sort chronologically)
    out.sort_by(|a, b| b.dir.file_name().cmp(&a.dir.file_name()));
    out
}
