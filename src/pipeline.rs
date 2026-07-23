//! Real-time audio pipeline: mic capture -> resample to 48 kHz mono -> noise engine
//! -> resample to output rate -> virtual device (BlackHole / VB-CABLE).
//!
//! cpal streams stay on the UI thread (they are !Send); DSP runs on a worker thread
//! connected through lock-free ring buffers.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use rubato::{FastFixedIn, PolynomialDegree, Resampler};

use crate::engine::{build_engine, EngineKind, HOP, SAMPLE_RATE};

/// State shared between the GUI, audio callbacks and the DSP worker.
pub struct Shared {
    /// 0 = DeepFilter, 1 = Rnnoise, 2 = Bypass
    pub engine_kind: AtomicU32,
    /// f32 bits: attenuation limit in dB
    pub atten_db_bits: AtomicU32,
    /// f32 bits: input/output RMS (linear 0..1)
    pub in_rms_bits: AtomicU32,
    pub out_rms_bits: AtomicU32,
    /// microseconds spent processing the last hop
    pub proc_us: AtomicU32,
    /// output callback had to emit silence (count)
    pub underruns: AtomicU64,
    /// worker sets this once the engine is ready
    pub engine_ready: AtomicBool,
    pub stop: AtomicBool,
    /// when set, the DSP worker sends each clean 48 kHz hop to the recorder
    pub rec_tx: Mutex<Option<crate::recorder::FrameTx>>,
}

impl Shared {
    pub fn new(kind: EngineKind, atten_db: f32) -> Arc<Self> {
        Arc::new(Self {
            engine_kind: AtomicU32::new(kind_to_u32(kind)),
            atten_db_bits: AtomicU32::new(atten_db.to_bits()),
            in_rms_bits: AtomicU32::new(0),
            out_rms_bits: AtomicU32::new(0),
            proc_us: AtomicU32::new(0),
            underruns: AtomicU64::new(0),
            engine_ready: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            rec_tx: Mutex::new(None),
        })
    }
}

pub fn kind_to_u32(k: EngineKind) -> u32 {
    match k {
        EngineKind::DeepFilter => 0,
        EngineKind::Rnnoise => 1,
        EngineKind::Bypass => 2,
    }
}

pub fn u32_to_kind(v: u32) -> EngineKind {
    match v {
        0 => EngineKind::DeepFilter,
        1 => EngineKind::Rnnoise,
        _ => EngineKind::Bypass,
    }
}

/// Keeps the streams and worker alive; dropping it stops everything.
pub struct Pipeline {
    _input_stream: cpal::Stream,
    _output_stream: Option<cpal::Stream>,
    worker: Option<std::thread::JoinHandle<()>>,
    shared: Arc<Shared>,
    pub input_name: String,
    pub output_name: String,
    pub input_rate: u32,
    pub output_rate: u32,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (inputs, outputs)
}

fn find_device(host: &cpal::Host, name: &str, input: bool) -> Result<cpal::Device> {
    let devices = if input {
        host.input_devices()?
    } else {
        host.output_devices()?
    };
    for d in devices {
        if d.name().map(|n| n == name).unwrap_or(false) {
            return Ok(d);
        }
    }
    // Windows: an *output* device can be captured directly (WASAPI loopback) —
    // no virtual cable or admin rights needed. cpal treats an output device
    // passed to build_input_stream as a loopback capture.
    if input && cfg!(target_os = "windows") {
        for d in host.output_devices()? {
            if d.name().map(|n| n == name).unwrap_or(false) {
                return Ok(d);
            }
        }
    }
    Err(anyhow!("device not found: {name}"))
}

/// Start a processing pipeline. `output_name: None` runs capture-only — audio is
/// still denoised, metered and available to the recorder, but not played anywhere
/// (used on machines without a virtual cable, e.g. no-admin Windows laptops).
pub fn start(input_name: &str, output_name: Option<&str>, shared: Arc<Shared>) -> Result<Pipeline> {
    let host = cpal::default_host();
    let input_dev = find_device(&host, input_name, true).context("input device")?;
    let output_dev = output_name
        .map(|n| find_device(&host, n, false).context("output device"))
        .transpose()?;

    // Loopback sources (Windows output devices) have no input config — use
    // their output config for capture.
    let in_cfg = input_dev
        .default_input_config()
        .or_else(|_| input_dev.default_output_config())
        .context("input config")?;
    let out_cfg = output_dev
        .as_ref()
        .map(|d| d.default_output_config().context("output config"))
        .transpose()?;
    let in_rate = in_cfg.sample_rate().0;
    let out_rate = out_cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(48_000);
    let in_ch = in_cfg.channels() as usize;
    let out_ch = out_cfg.as_ref().map(|c| c.channels() as usize).unwrap_or(1);

    // ~500 ms capacity: plenty of headroom without unbounded growth
    let (mut mic_prod, mic_cons) = HeapRb::<f32>::new(in_rate as usize / 2).split();
    let (clean_prod, mut clean_cons) = HeapRb::<f32>::new(out_rate as usize / 2).split();

    let err_fn = |e| log::error!("stream error: {e}");

    // --- Input stream: downmix to mono, push at device rate ---
    let input_stream = {
        let cfg: cpal::StreamConfig = in_cfg.clone().into();
        let shared = shared.clone();
        match in_cfg.sample_format() {
            cpal::SampleFormat::F32 => input_dev.build_input_stream(
                &cfg,
                move |data: &[f32], _| {
                    push_mono(&mut mic_prod, data, in_ch, &shared);
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => input_dev.build_input_stream(
                &cfg,
                move |data: &[i16], _| {
                    let f: Vec<f32> = data.iter().map(|s| *s as f32 / 32768.0).collect();
                    push_mono(&mut mic_prod, &f, in_ch, &shared);
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => input_dev.build_input_stream(
                &cfg,
                move |data: &[u16], _| {
                    let f: Vec<f32> =
                        data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0).collect();
                    push_mono(&mut mic_prod, &f, in_ch, &shared);
                },
                err_fn,
                None,
            )?,
            other => return Err(anyhow!("unsupported input sample format {other:?}")),
        }
    };

    // --- Output stream (absent in capture-only mode): pop mono, fan out ---
    let output_stream = if let (Some(output_dev), Some(out_cfg)) = (&output_dev, &out_cfg) {
        let cfg: cpal::StreamConfig = out_cfg.clone().into();
        let shared = shared.clone();
        Some(match out_cfg.sample_format() {
            cpal::SampleFormat::F32 => output_dev.build_output_stream(
                &cfg,
                move |data: &mut [f32], _| {
                    pull_fanout(&mut clean_cons, data, out_ch, &shared);
                },
                err_fn,
                None,
            )?,
            // WASAPI devices sometimes expose integer formats
            cpal::SampleFormat::I16 => output_dev.build_output_stream(
                &cfg,
                move |data: &mut [i16], _| {
                    let mut buf = vec![0.0f32; data.len()];
                    pull_fanout(&mut clean_cons, &mut buf, out_ch, &shared);
                    for (d, s) in data.iter_mut().zip(&buf) {
                        *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => output_dev.build_output_stream(
                &cfg,
                move |data: &mut [u16], _| {
                    let mut buf = vec![0.0f32; data.len()];
                    pull_fanout(&mut clean_cons, &mut buf, out_ch, &shared);
                    for (d, s) in data.iter_mut().zip(&buf) {
                        *d = ((s.clamp(-1.0, 1.0) * 32767.0) as i32 + 32768) as u16;
                    }
                },
                err_fn,
                None,
            )?,
            other => return Err(anyhow!("unsupported output sample format {other:?}")),
        })
    } else {
        None
    };

    // --- DSP worker ---
    let worker = {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("vocalm-dsp".into())
            .spawn(move || {
                if let Err(e) = dsp_loop(mic_cons, clean_prod, in_rate, out_rate, shared) {
                    log::error!("dsp worker exited: {e:#}");
                }
            })?
    };

    input_stream.play()?;
    if let Some(out) = &output_stream {
        out.play()?;
    }

    Ok(Pipeline {
        _input_stream: input_stream,
        _output_stream: output_stream,
        worker: Some(worker),
        shared,
        input_name: input_name.to_string(),
        output_name: output_name.unwrap_or("record-only").to_string(),
        input_rate: in_rate,
        output_rate: out_rate,
    })
}

fn push_mono(
    prod: &mut impl Producer<Item = f32>,
    interleaved: &[f32],
    channels: usize,
    shared: &Shared,
) {
    let mut sum_sq = 0.0f32;
    let frames = interleaved.len() / channels.max(1);
    for frame in interleaved.chunks_exact(channels.max(1)) {
        let mono = frame.iter().sum::<f32>() / channels.max(1) as f32;
        sum_sq += mono * mono;
        let _ = prod.try_push(mono);
    }
    if frames > 0 {
        let rms = (sum_sq / frames as f32).sqrt();
        shared.in_rms_bits.store(rms.to_bits(), Ordering::Relaxed);
    }
}

fn pull_fanout(
    cons: &mut impl Consumer<Item = f32>,
    data: &mut [f32],
    channels: usize,
    shared: &Shared,
) {
    let mut sum_sq = 0.0f32;
    let mut underrun = false;
    let frames = data.len() / channels.max(1);
    for frame in data.chunks_exact_mut(channels.max(1)) {
        let s = match cons.try_pop() {
            Some(s) => s,
            None => {
                underrun = true;
                0.0
            }
        };
        sum_sq += s * s;
        frame.fill(s);
    }
    if underrun {
        shared.underruns.fetch_add(1, Ordering::Relaxed);
    }
    if frames > 0 {
        let rms = (sum_sq / frames as f32).sqrt();
        shared.out_rms_bits.store(rms.to_bits(), Ordering::Relaxed);
    }
}

fn dsp_loop(
    mut mic: impl Consumer<Item = f32>,
    mut clean: impl Producer<Item = f32>,
    in_rate: u32,
    out_rate: u32,
    shared: Arc<Shared>,
) -> Result<()> {
    let mut kind = u32_to_kind(shared.engine_kind.load(Ordering::Relaxed));
    let mut atten = f32::from_bits(shared.atten_db_bits.load(Ordering::Relaxed));
    let mut engine = build_engine(kind, atten)?;
    shared.engine_ready.store(true, Ordering::Relaxed);

    const RS_CHUNK: usize = 480;
    let mut rs_in = (in_rate != SAMPLE_RATE)
        .then(|| {
            FastFixedIn::<f32>::new(
                SAMPLE_RATE as f64 / in_rate as f64,
                1.1,
                PolynomialDegree::Septic,
                RS_CHUNK,
                1,
            )
        })
        .transpose()?;
    let mut rs_out = (out_rate != SAMPLE_RATE)
        .then(|| {
            FastFixedIn::<f32>::new(
                out_rate as f64 / SAMPLE_RATE as f64,
                1.1,
                PolynomialDegree::Septic,
                HOP,
                1,
            )
        })
        .transpose()?;

    // FIFOs between resampler chunk sizes and the fixed HOP the engines need
    let mut raw_fifo: VecDeque<f32> = VecDeque::with_capacity(RS_CHUNK * 8);
    let mut fifo_48k: VecDeque<f32> = VecDeque::with_capacity(HOP * 8);
    let mut in_frame = [0.0f32; HOP];
    let mut out_frame = [0.0f32; HOP];
    let mut scratch = vec![0.0f32; RS_CHUNK];

    // Pre-fill output with 20 ms of silence to absorb scheduling jitter
    for _ in 0..(out_rate as usize / 50) {
        let _ = clean.try_push(0.0);
    }

    while !shared.stop.load(Ordering::Relaxed) {
        // React to live engine/attenuation changes from the GUI
        let want_kind = u32_to_kind(shared.engine_kind.load(Ordering::Relaxed));
        let want_atten = f32::from_bits(shared.atten_db_bits.load(Ordering::Relaxed));
        if want_kind != kind {
            shared.engine_ready.store(false, Ordering::Relaxed);
            engine = build_engine(want_kind, want_atten)?;
            kind = want_kind;
            atten = want_atten;
            shared.engine_ready.store(true, Ordering::Relaxed);
        } else if (want_atten - atten).abs() > f32::EPSILON {
            engine.set_atten_lim_db(want_atten);
            atten = want_atten;
        }

        // Drain mic samples into the 48 kHz FIFO
        let popped = mic.pop_slice(&mut scratch);
        match rs_in.as_mut() {
            None => fifo_48k.extend(&scratch[..popped]),
            Some(rs) => {
                raw_fifo.extend(&scratch[..popped]);
                while raw_fifo.len() >= RS_CHUNK {
                    let chunk: Vec<f32> = raw_fifo.drain(..RS_CHUNK).collect();
                    let out = rs.process(&[chunk], None)?;
                    fifo_48k.extend(&out[0]);
                }
            }
        }

        let mut worked = popped > 0;
        while fifo_48k.len() >= HOP {
            worked = true;
            for s in in_frame.iter_mut() {
                *s = fifo_48k.pop_front().unwrap();
            }
            let t0 = Instant::now();
            engine.process(&in_frame, &mut out_frame)?;
            shared
                .proc_us
                .store(t0.elapsed().as_micros() as u32, Ordering::Relaxed);

            // Recording tap (uncontended lock, ~100 Hz)
            if let Ok(guard) = shared.rec_tx.lock() {
                if let Some(tx) = guard.as_ref() {
                    crate::recorder::send_frame(tx, &out_frame);
                }
            }

            match rs_out.as_mut() {
                None => {
                    clean.push_slice(&out_frame);
                }
                Some(rs) => {
                    let out = rs.process(&[out_frame.as_slice()], None)?;
                    clean.push_slice(&out[0]);
                }
            }
        }

        if !worked {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}
