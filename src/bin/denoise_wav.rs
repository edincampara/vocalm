//! Offline verification tool: run a 48 kHz mono WAV through a noise engine.
//!
//! Usage: denoise-wav <in.wav> <out.wav> [df|rnnoise]

#[path = "../engine.rs"]
mod engine;

use anyhow::{bail, Context, Result};
use engine::{build_engine, EngineKind, HOP, SAMPLE_RATE};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: denoise-wav <in.wav> <out.wav> [df|rnnoise]");
    }
    let kind = match args.get(3).map(|s| s.as_str()).unwrap_or("df") {
        "rnnoise" => EngineKind::Rnnoise,
        _ => EngineKind::DeepFilter,
    };

    let mut reader = hound::WavReader::open(&args[1]).context("open input")?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE {
        bail!("input must be {} Hz (got {})", SAMPLE_RATE, spec.sample_rate);
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .step_by(spec.channels as usize)
                .map(|s| s.unwrap() as f32 / max)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .step_by(spec.channels as usize)
            .map(|s| s.unwrap())
            .collect(),
    };

    let t0 = std::time::Instant::now();
    let mut eng = build_engine(kind, 100.0)?;
    eprintln!("engine ready in {:.0} ms", t0.elapsed().as_secs_f32() * 1000.0);

    let out_spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&args[2], out_spec)?;

    let mut inf = [0.0f32; HOP];
    let mut outf = [0.0f32; HOP];
    let t1 = std::time::Instant::now();
    let mut frames = 0u64;
    for chunk in samples.chunks(HOP) {
        inf[..chunk.len()].copy_from_slice(chunk);
        inf[chunk.len()..].fill(0.0);
        eng.process(&inf, &mut outf)?;
        for s in &outf[..chunk.len()] {
            writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        frames += 1;
    }
    writer.finalize()?;

    let audio_secs = samples.len() as f32 / SAMPLE_RATE as f32;
    let proc_secs = t1.elapsed().as_secs_f32();
    eprintln!(
        "processed {:.1} s of audio in {:.2} s ({:.1}x realtime, {:.2} ms/frame)",
        audio_secs,
        proc_secs,
        audio_secs / proc_secs,
        proc_secs * 1000.0 / frames as f32
    );
    Ok(())
}
