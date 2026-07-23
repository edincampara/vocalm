//! Records cleaned 48 kHz audio to WAV on a dedicated writer thread.
//!
//! Mono mode: just the cleaned microphone.
//! Stereo mode: left = your mic, right = incoming meeting audio — both denoised.
//! Each DSP worker feeds frames through a bounded channel (never blocks audio);
//! the writer pads with silence if one side stalls, so channels stay aligned.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;

use crate::engine::{HOP, SAMPLE_RATE};

pub type FrameTx = SyncSender<Vec<f32>>;

pub struct Recorder {
    pub mic_tx: FrameTx,
    pub spk_tx: Option<FrameTx>,
    writer: Option<JoinHandle<Result<f32>>>,
}

impl Recorder {
    pub fn start(wav_path: &Path, stereo: bool) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: if stereo { 2 } else { 1 },
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::create(wav_path, spec)?;
        let (mic_tx, mic_rx): (FrameTx, Receiver<Vec<f32>>) = sync_channel(1024);
        let (spk_tx, spk_rx) = if stereo {
            let (t, r) = sync_channel(1024);
            (Some(t), Some(r))
        } else {
            (None, None)
        };

        let writer = std::thread::Builder::new()
            .name("vocalm-rec".into())
            .spawn(move || -> Result<f32> {
                let mut frames: u64 = 0;
                match spk_rx {
                    None => {
                        while let Ok(frame) = mic_rx.recv() {
                            for s in &frame {
                                wav.write_sample(to_i16(*s))?;
                            }
                            frames += 1;
                        }
                    }
                    Some(spk_rx) => {
                        let mut micf: VecDeque<f32> = VecDeque::new();
                        let mut spkf: VecDeque<f32> = VecDeque::new();
                        loop {
                            // Mic is the clock; stop when its channel closes.
                            match mic_rx.recv_timeout(Duration::from_millis(200)) {
                                Ok(f) => micf.extend(f),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(_) => break,
                            }
                            while let Ok(f) = spk_rx.try_recv() {
                                spkf.extend(f);
                            }
                            while micf.len() >= HOP {
                                for _ in 0..HOP {
                                    let l = micf.pop_front().unwrap();
                                    let r = spkf.pop_front().unwrap_or(0.0);
                                    wav.write_sample(to_i16(l))?;
                                    wav.write_sample(to_i16(r))?;
                                }
                                frames += 1;
                                // Keep the incoming side from drifting far ahead
                                if spkf.len() > HOP * 20 {
                                    spkf.drain(..spkf.len() - HOP * 4);
                                }
                            }
                        }
                    }
                }
                wav.finalize()?;
                Ok(frames as f32 * HOP as f32 / SAMPLE_RATE as f32)
            })?;

        Ok(Self {
            mic_tx,
            spk_tx,
            writer: Some(writer),
        })
    }

    /// Stop recording; returns the recorded duration in seconds.
    pub fn stop(mut self) -> f32 {
        // Dropping the senders disconnects the writer's receivers
        drop(std::mem::replace(&mut self.mic_tx, sync_channel(1).0));
        self.spk_tx = None;
        self.writer
            .take()
            .and_then(|w| w.join().ok())
            .and_then(|r| r.ok())
            .unwrap_or(0.0)
    }
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Non-blocking send used from the DSP threads.
pub fn send_frame(tx: &FrameTx, frame: &[f32]) {
    match tx.try_send(frame.to_vec()) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_wav(path: &Path) -> (hound::WavSpec, Vec<i16>) {
        let mut r = hound::WavReader::open(path).unwrap();
        let spec = r.spec();
        let samples = r.samples::<i16>().map(|s| s.unwrap()).collect();
        (spec, samples)
    }

    #[test]
    fn mono_recording_writes_all_frames() {
        let dir = std::env::temp_dir().join("vocalm-test-mono");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audio.wav");
        let rec = Recorder::start(&path, false).unwrap();
        for _ in 0..100 {
            send_frame(&rec.mic_tx, &[0.5f32; HOP]);
        }
        let dur = rec.stop();
        assert!((dur - 1.0).abs() < 0.02, "expected ~1s, got {dur}");
        let (spec, samples) = read_wav(&path);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, SAMPLE_RATE);
        assert_eq!(samples.len(), 100 * HOP);
        assert!(samples.iter().all(|s| (*s - 16383).abs() <= 1));
    }

    #[test]
    fn stereo_recording_keeps_channels_separate_and_pads() {
        let dir = std::env::temp_dir().join("vocalm-test-stereo");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audio.wav");
        let rec = Recorder::start(&path, true).unwrap();
        let spk_tx = rec.spk_tx.clone().unwrap();
        // mic sends 50 frames; speaker only 20 → right channel must be zero-padded
        for i in 0..50 {
            send_frame(&rec.mic_tx, &[0.25f32; HOP]);
            if i < 20 {
                send_frame(&spk_tx, &[-0.25f32; HOP]);
            }
        }
        // give the writer time to drain before disconnecting
        std::thread::sleep(std::time::Duration::from_millis(300));
        let dur = rec.stop();
        assert!((dur - 0.5).abs() < 0.02, "expected ~0.5s, got {dur}");
        let (spec, samples) = read_wav(&path);
        assert_eq!(spec.channels, 2);
        assert_eq!(samples.len(), 50 * HOP * 2);
        // left channel: constant mic value everywhere
        assert!((samples[0] - 8191).abs() <= 1);
        assert!((samples[samples.len() - 2] - 8191).abs() <= 1);
        // right channel: speaker value early, silence after it stopped
        assert!((samples[1] + 8192).abs() <= 1);
        assert_eq!(samples[samples.len() - 1], 0);
    }
}
