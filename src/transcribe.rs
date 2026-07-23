//! On-device transcription with whisper.cpp (Metal-accelerated on Apple Silicon).
//! The ggml model is downloaded once into the app data dir.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rubato::{FastFixedIn, PolynomialDegree, Resampler};

use crate::config::models_dir;
use crate::meetings::{AUDIO_FILE, TRANSCRIPT_FILE};

const MODEL_NAME: &str = "ggml-base.bin";
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";

#[derive(Clone)]
pub struct Job {
    pub meeting_dir: PathBuf,
    pub status: Arc<Mutex<String>>,
    pub done: Arc<Mutex<bool>>,
}

pub fn model_path() -> PathBuf {
    models_dir().join(MODEL_NAME)
}

pub fn model_available() -> bool {
    model_path().exists()
}

/// Download progress of the startup model fetch, readable by the UI.
#[derive(Clone)]
pub struct ModelDownload {
    pub status: Arc<Mutex<String>>,
    pub done: Arc<Mutex<bool>>,
}

/// Fetch the Whisper model in the background at app startup so transcription is
/// instantly ready later. Returns None if the model is already installed.
pub fn ensure_model_background() -> Option<ModelDownload> {
    if model_available() {
        return None;
    }
    let dl = ModelDownload {
        status: Arc::new(Mutex::new("Downloading transcription model…".into())),
        done: Arc::new(Mutex::new(false)),
    };
    let d = dl.clone();
    std::thread::Builder::new()
        .name("vocalm-model-dl".into())
        .spawn(move || {
            let result = download_model(&model_path());
            let mut st = d.status.lock().unwrap();
            match result {
                Ok(()) => *st = "Transcription model ready ✓".into(),
                Err(e) => *st = format!("Model download failed (will retry on transcribe): {e:#}"),
            }
            *d.done.lock().unwrap() = true;
        })
        .expect("spawn model download thread");
    Some(dl)
}

/// Spawn a background transcription job for a meeting folder.
pub fn spawn(meeting_dir: PathBuf) -> Job {
    let job = Job {
        meeting_dir: meeting_dir.clone(),
        status: Arc::new(Mutex::new("starting…".into())),
        done: Arc::new(Mutex::new(false)),
    };
    let j = job.clone();
    std::thread::Builder::new()
        .name("vocalm-transcribe".into())
        .spawn(move || {
            let result = run(&j);
            let mut st = j.status.lock().unwrap();
            match result {
                Ok(()) => *st = "done".into(),
                Err(e) => *st = format!("failed: {e:#}"),
            }
            *j.done.lock().unwrap() = true;
        })
        .expect("spawn transcribe thread");
    job
}

fn set_status(job: &Job, s: impl Into<String>) {
    *job.status.lock().unwrap() = s.into();
}

fn run(job: &Job) -> Result<()> {
    let model = model_path();
    if !model.exists() {
        set_status(job, "downloading Whisper model (~148 MB, one-time)…");
        download_model(&model)?;
    }

    set_status(job, "loading audio…");
    let samples = load_wav_16k_mono(&job.meeting_dir.join(AUDIO_FILE))?;

    set_status(job, "loading model…");
    let ctx = whisper_rs::WhisperContext::new_with_params(
        model.to_str().context("model path utf-8")?,
        whisper_rs::WhisperContextParameters::default(),
    )
    .context("load whisper model")?;
    let mut state = ctx.create_state().context("create whisper state")?;

    set_status(job, "transcribing…");
    let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy {
        best_of: 1,
    });
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    // auto-detect language
    params.set_language(None);
    state.full(params, &samples).context("whisper inference")?;

    let n = state.full_n_segments()?;
    let mut out = String::new();
    for i in 0..n {
        let t0 = state.full_get_segment_t0(i)? as f32 / 100.0;
        let t1 = state.full_get_segment_t1(i)? as f32 / 100.0;
        let text = state.full_get_segment_text(i)?;
        out.push_str(&format!(
            "[{} - {}] {}\n",
            fmt_ts(t0),
            fmt_ts(t1),
            text.trim()
        ));
    }
    std::fs::write(job.meeting_dir.join(TRANSCRIPT_FILE), out)?;
    Ok(())
}

fn fmt_ts(secs: f32) -> String {
    let m = (secs / 60.0) as u32;
    let s = secs % 60.0;
    format!("{m:02}:{s:04.1}")
}

fn download_model(dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let tmp = dest.with_extension("part");
    let resp = ureq::get(MODEL_URL).call().context("download model")?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp)?;
    std::io::copy(&mut reader, &mut file).context("write model")?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Whisper wants 16 kHz mono f32.
fn load_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).context("open recording")?;
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let mono: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .collect::<Result<Vec<_>, _>>()?
                .chunks(ch)
                .map(|c| c.iter().map(|s| *s as f32 / max).sum::<f32>() / ch as f32)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()?
            .chunks(ch)
            .map(|c| c.iter().sum::<f32>() / ch as f32)
            .collect(),
    };

    if spec.sample_rate == 16_000 {
        return Ok(mono);
    }
    let mut rs = FastFixedIn::<f32>::new(
        16_000.0 / spec.sample_rate as f64,
        1.1,
        PolynomialDegree::Septic,
        1024,
        1,
    )?;
    let mut out = Vec::with_capacity(mono.len() / 3 + 16);
    for chunk in mono.chunks(1024) {
        let mut buf = chunk.to_vec();
        buf.resize(1024, 0.0);
        let res = rs.process(&[buf], None)?;
        out.extend(&res[0]);
    }
    Ok(out)
}
