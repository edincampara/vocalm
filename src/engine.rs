//! Noise-suppression engines. All operate on 48 kHz mono f32 frames of HOP (480 = 10 ms).

use anyhow::Result;
use ndarray::Array2;

/// Internal processing rate and hop. Both engines natively use 48 kHz / 480-sample hops.
pub const SAMPLE_RATE: u32 = 48_000;
pub const HOP: usize = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// DeepFilterNet3: best quality, ~40 ms algorithmic latency.
    DeepFilter,
    /// RNNoise: ultra-low latency (~10 ms), lighter suppression.
    Rnnoise,
    /// Pass audio through untouched (A/B comparison).
    Bypass,
}

// Not `Send`: tract's inference state is thread-bound, so engines are built and
// used entirely on the DSP thread.
pub trait NoiseEngine {
    /// Process exactly one HOP of mono 48 kHz audio in [-1, 1]. Returns speech probability/SNR hint if available.
    fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> Result<()>;
    /// Attenuation limit in dB (how much noise is suppressed at most). Not all engines support it.
    fn set_atten_lim_db(&mut self, _db: f32) {}
}

pub struct BypassEngine;

impl NoiseEngine for BypassEngine {
    fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> Result<()> {
        output.copy_from_slice(input);
        Ok(())
    }
}

pub struct RnnoiseEngine {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    in_scaled: [f32; HOP],
    out_scaled: [f32; HOP],
}

impl RnnoiseEngine {
    pub fn new() -> Self {
        Self {
            state: nnnoiseless::DenoiseState::new(),
            in_scaled: [0.0; HOP],
            out_scaled: [0.0; HOP],
        }
    }
}

impl NoiseEngine for RnnoiseEngine {
    fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> Result<()> {
        // nnnoiseless expects samples in i16 float range
        for (dst, src) in self.in_scaled.iter_mut().zip(input) {
            *dst = src * 32768.0;
        }
        self.state.process_frame(&mut self.out_scaled, &self.in_scaled);
        for (dst, src) in output.iter_mut().zip(&self.out_scaled) {
            *dst = (src / 32768.0).clamp(-1.0, 1.0);
        }
        Ok(())
    }
}

pub struct DeepFilterEngine {
    model: df::tract::DfTract,
    in_frame: Array2<f32>,
    out_frame: Array2<f32>,
}

impl DeepFilterEngine {
    pub fn new(atten_lim_db: f32) -> Result<Self> {
        let df_params = df::tract::DfParams::default();
        let mut r_params = df::tract::RuntimeParams::default_with_ch(1);
        r_params.atten_lim_db = atten_lim_db;
        let model = df::tract::DfTract::new(df_params, &r_params)?;
        anyhow::ensure!(
            model.sr == SAMPLE_RATE as usize && model.hop_size == HOP,
            "unexpected model geometry: sr={} hop={}",
            model.sr,
            model.hop_size
        );
        Ok(Self {
            model,
            in_frame: Array2::zeros((1, HOP)),
            out_frame: Array2::zeros((1, HOP)),
        })
    }
}

impl NoiseEngine for DeepFilterEngine {
    fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> Result<()> {
        self.in_frame
            .as_slice_mut()
            .unwrap()
            .copy_from_slice(input);
        self.model
            .process(self.in_frame.view(), self.out_frame.view_mut())?;
        output.copy_from_slice(self.out_frame.as_slice().unwrap());
        Ok(())
    }

    fn set_atten_lim_db(&mut self, db: f32) {
        let _ = self.model.set_atten_lim(db);
    }
}

pub fn build_engine(kind: EngineKind, atten_lim_db: f32) -> Result<Box<dyn NoiseEngine>> {
    Ok(match kind {
        EngineKind::Bypass => Box::new(BypassEngine),
        EngineKind::Rnnoise => Box::new(RnnoiseEngine::new()),
        EngineKind::DeepFilter => Box::new(DeepFilterEngine::new(atten_lim_db)?),
    })
}
