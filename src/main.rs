#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod meetings;
mod pipeline;
mod recorder;
mod transcribe;
mod ui;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, RichText};
use engine::EngineKind;
use pipeline::{kind_to_u32, u32_to_kind, Pipeline, Shared};
use ui::{
    apply_theme, big_button, card, chip, db_norm, dual_meter, orb, section_label, segmented,
    smooth_level, toggle, ACCENT, AMBER, BG, RED, SURFACE_HI, TEXT, TEXT_DIM, TEXT_FAINT,
};

fn main() -> eframe::Result {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        let (inputs, outputs) = pipeline::list_devices();
        println!("Input devices:");
        for d in inputs {
            println!("  {d}");
        }
        println!("Output devices:");
        for d in outputs {
            let tag = if is_virtual(&d) { "  (virtual)" } else { "" };
            println!("  {d}{tag}");
        }
        return Ok(());
    }
    // Headless transcription: `vocalm --transcribe <meeting-dir>` (also used for testing)
    if let Some(i) = args.iter().position(|a| a == "--transcribe") {
        let dir = std::path::PathBuf::from(args.get(i + 1).expect("--transcribe <dir>"));
        let job = transcribe::spawn(dir);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            eprintln!("{}", job.status.lock().unwrap().clone());
            if *job.done.lock().unwrap() {
                break;
            }
        }
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([440.0, 640.0])
        .with_min_inner_size([400.0, 560.0])
        .with_title("Vocalm");
    if cfg!(target_os = "macos") {
        // Dark content flows under a transparent titlebar; traffic lights float.
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Vocalm",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

/// Names that identify a virtual loopback device.
const VIRTUAL_DEV_HINTS: &[&str] = &["vocalm", "blackhole", "cable", "vb-audio", "loopback"];

fn is_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    VIRTUAL_DEV_HINTS.iter().any(|h| n.contains(h))
}

/// Real microphones (exclude loopbacks and other apps' virtual mics).
fn is_real_mic(name: &str) -> bool {
    let n = name.to_lowercase();
    !is_virtual(name)
        && !n.contains("krisp")
        && !n.contains("teams")
        && !n.contains("zoom")
        && !n.contains("nomachine")
        && !n.contains("mmaudio")
        && !n.contains("epoccam")
}

fn is_real_output(name: &str) -> bool {
    let n = name.to_lowercase();
    !is_virtual(name)
        && !n.contains("teams")
        && !n.contains("zoom")
        && !n.contains("nomachine")
        && !n.contains("mmaudio")
}

/// What the meeting app's device list shows for our cable. VB-CABLE names its
/// playback side "CABLE Input" and capture side "CABLE Output"; BlackHole and the
/// Vocalm drivers use one name for both sides (returned unchanged).
fn app_facing_name(cable: &str) -> String {
    let lc = cable.to_lowercase();
    if let Some(pos) = lc.find("input") {
        format!("{}Output{}", &cable[..pos], &cable[pos + 5..])
    } else if let Some(pos) = lc.find("output") {
        format!("{}Input{}", &cable[..pos], &cable[pos + 6..])
    } else {
        cable.to_string()
    }
}

/// Identify the physical cable regardless of which side we're naming, so the
/// mic and speaker paths never end up on the same cable.
fn cable_base(name: &str) -> String {
    name.to_lowercase().replace("input", "").replace("output", "").trim().to_string()
}

/// Virtual output the mic path renders into (apps use it as their microphone).
fn pick_mic_cable(outputs: &[String]) -> Option<String> {
    let lc = |n: &String| n.to_lowercase();
    outputs
        .iter()
        .find(|n| lc(n).contains("vocalm microphone"))
        .or_else(|| outputs.iter().find(|n| lc(n).contains("blackhole 2ch")))
        .or_else(|| outputs.iter().find(|n| lc(n).contains("cable input")))
        .or_else(|| outputs.iter().find(|n| is_virtual(n)))
        .cloned()
}

/// Virtual input the incoming path captures (apps use it as their speaker).
fn pick_spk_cable(inputs: &[String], mic_cable: Option<&str>) -> Option<String> {
    let lc = |n: &String| n.to_lowercase();
    inputs
        .iter()
        .find(|n| lc(n).contains("vocalm speaker"))
        .or_else(|| inputs.iter().find(|n| lc(n).contains("blackhole 16ch")))
        .or_else(|| inputs.iter().find(|n| lc(n).contains("blackhole 64ch")))
        .or_else(|| {
            let mic_base = mic_cable.map(cable_base);
            inputs
                .iter()
                .find(|n| is_virtual(n) && Some(cable_base(n)) != mic_base)
        })
        .cloned()
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Home,
    Meetings,
    Settings,
}

#[derive(Default)]
struct Section {
    shared: Option<Arc<Shared>>,
    pipeline: Option<Pipeline>,
    error: Option<String>,
}

impl Section {
    fn stop(&mut self) {
        self.pipeline = None;
        self.shared = None;
    }
    fn active(&self) -> bool {
        self.pipeline.is_some()
    }
    fn clean_rms(&self) -> f32 {
        self.shared
            .as_ref()
            .map(|s| f32::from_bits(s.out_rms_bits.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }
    fn raw_rms(&self) -> f32 {
        self.shared
            .as_ref()
            .map(|s| f32::from_bits(s.in_rms_bits.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }
}

struct ActiveRecording {
    meeting: meetings::Meeting,
    recorder: Option<recorder::Recorder>,
    started: std::time::Instant,
}

struct App {
    cfg: config::AppConfig,
    tab: Tab,
    inputs: Vec<String>,
    outputs: Vec<String>,

    master_on: bool,
    mic_in: Option<String>,
    mic_cable: Option<String>,
    spk_on: bool,
    spk_cable: Option<String>,
    spk_out: Option<String>,

    engine_kind: EngineKind,
    atten_db: f32,

    mic: Section,
    spk: Section,

    // display-smoothed levels (fast attack, slow release)
    disp_mic_raw: f32,
    disp_mic_clean: f32,
    disp_spk_raw: f32,
    disp_spk_clean: f32,

    recording: Option<ActiveRecording>,
    meetings: Vec<meetings::Meeting>,
    meetings_dirty: bool,
    editing: Option<(std::path::PathBuf, String, String)>,
    jobs: Vec<transcribe::Job>,
    viewing_transcript: Option<(String, String)>,
    model_dl: Option<transcribe::ModelDownload>,
}

impl App {
    fn new() -> Self {
        let cfg = config::AppConfig::load();
        let (inputs, outputs) = pipeline::list_devices();

        let host_default_in = cpal::traits::HostTrait::default_input_device(&cpal::default_host())
            .and_then(|d| cpal::traits::DeviceTrait::name(&d).ok());

        let mic_in = cfg
            .input_device
            .clone()
            .filter(|n| inputs.contains(n))
            .or_else(|| host_default_in.filter(|n| is_real_mic(n)))
            .or_else(|| inputs.iter().find(|n| is_real_mic(n)).cloned());
        let mic_cable = cfg
            .output_device
            .clone()
            .filter(|n| outputs.contains(n))
            .or_else(|| pick_mic_cable(&outputs));
        let spk_cable = cfg
            .spk_input
            .clone()
            .filter(|n| inputs.contains(n))
            .or_else(|| pick_spk_cable(&inputs, mic_cable.as_deref()));
        let host_default_out =
            cpal::traits::HostTrait::default_output_device(&cpal::default_host())
                .and_then(|d| cpal::traits::DeviceTrait::name(&d).ok());
        let spk_out = cfg
            .spk_output
            .clone()
            .filter(|n| outputs.contains(n))
            .or_else(|| host_default_out.filter(|n| is_real_output(n)))
            .or_else(|| outputs.iter().find(|n| is_real_output(n)).cloned());

        let mut app = Self {
            engine_kind: u32_to_kind(cfg.engine_kind),
            atten_db: cfg.atten_db,
            spk_on: cfg.spk_enabled && spk_cable.is_some(),
            master_on: true,
            tab: Tab::Home,
            inputs,
            outputs,
            mic_in,
            mic_cable,
            spk_cable,
            spk_out,
            mic: Section::default(),
            spk: Section::default(),
            disp_mic_raw: 0.0,
            disp_mic_clean: 0.0,
            disp_spk_raw: 0.0,
            disp_spk_clean: 0.0,
            recording: None,
            meetings: vec![],
            meetings_dirty: true,
            editing: None,
            jobs: vec![],
            viewing_transcript: None,
            model_dl: transcribe::ensure_model_background(),
            cfg,
        };
        app.apply_audio_state();
        app
    }

    fn persist(&mut self) {
        self.cfg.engine_kind = kind_to_u32(self.engine_kind);
        self.cfg.atten_db = self.atten_db;
        self.cfg.input_device = self.mic_in.clone();
        self.cfg.output_device = self.mic_cable.clone();
        self.cfg.spk_input = self.spk_cable.clone();
        self.cfg.spk_output = self.spk_out.clone();
        self.cfg.spk_enabled = self.spk_on;
        self.cfg.save();
    }

    /// (Re)start or stop both pipelines to match the UI state.
    fn apply_audio_state(&mut self) {
        self.stop_recording();
        self.mic.stop();
        self.spk.stop();
        if self.master_on {
            if let (Some(inp), Some(outp)) = (self.mic_in.clone(), self.mic_cable.clone()) {
                let shared = Shared::new(self.engine_kind, self.atten_db);
                match pipeline::start(&inp, &outp, shared.clone()) {
                    Ok(p) => {
                        self.mic.pipeline = Some(p);
                        self.mic.shared = Some(shared);
                        self.mic.error = None;
                    }
                    Err(e) => self.mic.error = Some(format!("{e:#}")),
                }
            }
            if self.spk_on {
                if let (Some(inp), Some(outp)) = (self.spk_cable.clone(), self.spk_out.clone()) {
                    let shared = Shared::new(self.engine_kind, self.atten_db);
                    match pipeline::start(&inp, &outp, shared.clone()) {
                        Ok(p) => {
                            self.spk.pipeline = Some(p);
                            self.spk.shared = Some(shared);
                            self.spk.error = None;
                        }
                        Err(e) => self.spk.error = Some(format!("{e:#}")),
                    }
                }
            }
        }
        self.persist();
    }

    fn set_engine(&mut self, kind: EngineKind) {
        self.engine_kind = kind;
        for s in [&self.mic.shared, &self.spk.shared].into_iter().flatten() {
            s.engine_kind.store(kind_to_u32(kind), Ordering::Relaxed);
        }
        self.persist();
    }

    fn set_atten(&mut self) {
        for s in [&self.mic.shared, &self.spk.shared].into_iter().flatten() {
            s.atten_db_bits
                .store(self.atten_db.to_bits(), Ordering::Relaxed);
        }
    }

    fn start_recording(&mut self) {
        if !self.mic.active() || self.recording.is_some() {
            return;
        }
        let stereo = self.spk.active();
        match meetings::create_meeting(&self.cfg.recordings_dir).and_then(|m| {
            let rec = recorder::Recorder::start(&m.dir.join(meetings::AUDIO_FILE), stereo)?;
            Ok((m, rec))
        }) {
            Ok((meeting, rec)) => {
                if let Some(s) = &self.mic.shared {
                    *s.rec_tx.lock().unwrap() = Some(rec.mic_tx.clone());
                }
                if let (Some(s), Some(tx)) = (&self.spk.shared, rec.spk_tx.clone()) {
                    *s.rec_tx.lock().unwrap() = Some(tx);
                }
                self.recording = Some(ActiveRecording {
                    meeting,
                    recorder: Some(rec),
                    started: std::time::Instant::now(),
                });
            }
            Err(e) => self.mic.error = Some(format!("recording failed: {e:#}")),
        }
    }

    fn stop_recording(&mut self) {
        if let Some(mut ar) = self.recording.take() {
            for s in [&self.mic.shared, &self.spk.shared].into_iter().flatten() {
                *s.rec_tx.lock().unwrap() = None;
            }
            if let Some(rec) = ar.recorder.take() {
                let dur = rec.stop();
                ar.meeting.meta.duration_secs = dur;
                let _ = meetings::save_meta(&ar.meeting);
            }
            self.meetings_dirty = true;
        }
    }

    fn refresh_meetings(&mut self) {
        self.meetings = meetings::list_meetings(&self.cfg.recordings_dir);
        self.meetings_dirty = false;
    }

    fn rescan(&mut self) {
        let (i, o) = pipeline::list_devices();
        self.inputs = i;
        self.outputs = o;
        if self.mic_cable.is_none() {
            self.mic_cable = pick_mic_cable(&self.outputs);
        }
        if self.spk_cable.is_none() {
            self.spk_cable = pick_spk_cable(&self.inputs, self.mic_cable.as_deref());
        }
    }
}

fn device_combo(
    ui: &mut egui::Ui,
    id: &str,
    selected: &mut Option<String>,
    devices: &[String],
    filter: impl Fn(&str) -> bool,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .selected_text(
            RichText::new(selected.clone().unwrap_or_else(|| "Select a device…".into()))
                .color(TEXT),
        )
        .show_ui(ui, |ui| {
            for name in devices.iter().filter(|n| filter(n)) {
                if ui
                    .selectable_label(selected.as_deref() == Some(name), name)
                    .clicked()
                {
                    *selected = Some(name.clone());
                    changed = true;
                }
            }
        });
    changed
}

fn fmt_dur(secs: f32) -> String {
    let m = (secs / 60.0) as u32;
    let s = (secs % 60.0) as u32;
    format!("{m:02}:{s:02}")
}

fn fmt_created(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|d| d.format("%b %-d · %H:%M").to_string())
        .unwrap_or_default()
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_recording();
        self.persist();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 30 fps keeps the orb and meters fluid; DSP runs on its own threads.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        // ⌘1/2/3 (Ctrl on Windows) switch tabs
        ctx.input(|i| {
            if i.modifiers.command {
                if i.key_pressed(egui::Key::Num1) {
                    self.tab = Tab::Home;
                }
                if i.key_pressed(egui::Key::Num2) {
                    self.tab = Tab::Meetings;
                    self.meetings_dirty = true;
                }
                if i.key_pressed(egui::Key::Num3) {
                    self.tab = Tab::Settings;
                }
            }
        });

        smooth_level(&mut self.disp_mic_raw, db_norm(self.mic.raw_rms()));
        smooth_level(&mut self.disp_mic_clean, db_norm(self.mic.clean_rms()));
        smooth_level(&mut self.disp_spk_raw, db_norm(self.spk.raw_rms()));
        smooth_level(&mut self.disp_spk_clean, db_norm(self.spk.clean_rms()));

        egui::TopBottomPanel::top("tabs")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                if cfg!(target_os = "macos") {
                    ui.add_space(22.0); // clear the floating traffic lights
                }
                ui.horizontal(|ui| {
                    // wordmark: painted accent dot + name
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, ACCENT);
                    ui.label(RichText::new("Vocalm").size(16.0).strong().color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.recording.is_some() {
                            chip(ui, "REC", RED);
                        }
                    });
                });
                ui.add_space(6.0);
                let active = match self.tab {
                    Tab::Home => 0,
                    Tab::Meetings => 1,
                    Tab::Settings => 2,
                };
                ui.vertical_centered(|ui| {
                    if let Some(i) = segmented(ui, "main_tabs", &["Home", "Meetings", "Settings"], active)
                    {
                        self.tab = match i {
                            0 => Tab::Home,
                            1 => {
                                self.meetings_dirty = true;
                                Tab::Meetings
                            }
                            _ => Tab::Settings,
                        };
                    }
                });
            });

        if let Some((title, text)) = self.viewing_transcript.clone() {
            let mut open = true;
            egui::Window::new(RichText::new(format!("Transcript — {title}")).size(14.0))
                .open(&mut open)
                .default_size([420.0, 420.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().color(TEXT_DIM));
                    });
                });
            if !open {
                self.viewing_transcript = None;
            }
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        Tab::Home => self.ui_home(ui),
                        Tab::Meetings => self.ui_meetings(ui),
                        Tab::Settings => self.ui_settings(ui),
                    });
            });
    }
}

impl App {
    fn ui_home(&mut self, ui: &mut egui::Ui) {
        // ---- hero orb ----
        ui.vertical_centered(|ui| {
            ui.add_space(2.0);
            if orb(
                ui,
                self.master_on && self.mic.active(),
                self.disp_mic_clean,
                self.disp_mic_raw,
                self.recording.is_some(),
            ) {
                self.master_on = !self.master_on;
                self.apply_audio_state();
            }
            let (status, color) = if !self.master_on {
                ("Paused — click the mic to resume".to_string(), TEXT_FAINT)
            } else if self.mic.active() {
                ("Your voice is protected".to_string(), TEXT)
            } else if self.mic_cable.is_none() {
                ("Audio devices missing".to_string(), AMBER)
            } else {
                ("Starting…".to_string(), TEXT_DIM)
            };
            ui.label(RichText::new(status).size(15.0).color(color));

            // live noise-reduction readout
            let raw = self.mic.raw_rms();
            let clean = self.mic.clean_rms();
            if self.master_on && self.mic.active() {
                let red_db = if raw > 1e-5 {
                    (20.0 * (raw / clean.max(1e-6)).log10()).clamp(0.0, 60.0)
                } else {
                    0.0
                };
                let label = if red_db >= 3.0 {
                    format!("removing {red_db:.0} dB of noise")
                } else {
                    "listening".to_string()
                };
                ui.label(RichText::new(label).small().color(ACCENT));
            } else {
                ui.label(RichText::new(" ").small());
            }
        });

        if let Some(dl) = &self.model_dl {
            if !*dl.done.lock().unwrap() {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(dl.status.lock().unwrap().clone())
                            .small()
                            .color(TEXT_FAINT),
                    );
                });
            }
        }
        ui.add_space(6.0);

        // ---- your voice ----
        let inputs = self.inputs.clone();
        let outputs = self.outputs.clone();
        card(ui, |ui| {
            section_label(ui, "YOUR VOICE");
            if device_combo(ui, "mic_in", &mut self.mic_in, &inputs, is_real_mic) {
                self.apply_audio_state();
            }
            dual_meter(
                ui,
                self.disp_mic_raw,
                self.disp_mic_clean,
                self.master_on && self.mic.active(),
            );
            match (&self.mic_cable, self.master_on && self.mic.active()) {
                (Some(cable), true) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new("Meeting app microphone →").small().color(TEXT_FAINT));
                        ui.label(
                            RichText::new(format!("“{}”", app_facing_name(cable)))
                                .small()
                                .color(TEXT_DIM),
                        );
                    });
                }
                (None, _) => {
                    ui.label(
                        RichText::new(
                            "Virtual device missing — run the Vocalm installer (macOS) or \
                             install VB-CABLE (Windows), then Rescan in Advanced.",
                        )
                        .small()
                        .color(AMBER),
                    );
                }
                _ => {}
            }
            if let Some(e) = &self.mic.error {
                ui.label(RichText::new(e.clone()).small().color(RED));
            }
        });

        ui.add_space(2.0);

        // ---- incoming audio ----
        card(ui, |ui| {
            ui.horizontal(|ui| {
                section_label(ui, "INCOMING AUDIO");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut on = self.spk_on;
                    if toggle(ui, &mut on).changed() {
                        self.spk_on = on;
                        self.apply_audio_state();
                    }
                });
            });
            ui.label(
                RichText::new("Cleans the noise arriving from everyone else's microphones.")
                    .small()
                    .color(TEXT_FAINT),
            );
            if self.spk_cable.is_none() {
                ui.label(
                    RichText::new(
                        "Needs a second virtual device — included in the Vocalm installer \
                         (macOS) or a second VB-CABLE (Windows).",
                    )
                    .small()
                    .color(AMBER),
                );
            } else if self.spk_on {
                if device_combo(ui, "spk_out", &mut self.spk_out, &outputs, is_real_output) {
                    self.apply_audio_state();
                }
                dual_meter(
                    ui,
                    self.disp_spk_raw,
                    self.disp_spk_clean,
                    self.master_on && self.spk.active(),
                );
                if let Some(cable) = &self.spk_cable {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new("Meeting app speaker →").small().color(TEXT_FAINT));
                        ui.label(
                            RichText::new(format!("“{}”", app_facing_name(cable)))
                                .small()
                                .color(TEXT_DIM),
                        );
                    });
                }
                if let Some(e) = &self.spk.error {
                    ui.label(RichText::new(e.clone()).small().color(RED));
                }
            }
        });

        ui.add_space(6.0);

        // ---- record ----
        let (label, fill, tc) = match &self.recording {
            None => ("⏺  Record meeting".to_string(), SURFACE_HI, TEXT),
            Some(ar) => (
                format!("⏹  Stop  ·  {}", fmt_dur(ar.started.elapsed().as_secs_f32())),
                RED,
                Color32::from_rgb(24, 10, 10),
            ),
        };
        if big_button(ui, &label, fill, tc, self.mic.active()).clicked() {
            if self.recording.is_some() {
                self.stop_recording();
            } else {
                self.start_recording();
            }
        }
        if self.recording.is_some() && self.spk.active() {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Recording both sides — you: left channel · others: right")
                        .small()
                        .color(TEXT_FAINT),
                );
            });
        }

        // ---- advanced ----
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("Advanced").small().color(TEXT_FAINT))
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new("Engine").small().color(TEXT_DIM));
                ui.horizontal(|ui| {
                    for (kind, label) in [
                        (EngineKind::DeepFilter, "DeepFilterNet3"),
                        (EngineKind::Rnnoise, "RNNoise"),
                        (EngineKind::Bypass, "Off"),
                    ] {
                        if ui.selectable_label(self.engine_kind == kind, label).clicked() {
                            self.set_engine(kind);
                        }
                    }
                });
                if self.engine_kind == EngineKind::DeepFilter {
                    let resp = ui.add(
                        egui::Slider::new(&mut self.atten_db, 6.0..=100.0)
                            .text("suppression (dB)")
                            .integer(),
                    );
                    if resp.changed() {
                        self.set_atten();
                    }
                    if resp.drag_stopped() {
                        self.persist();
                    }
                }
                ui.separator();
                ui.label(RichText::new("Mic routes to (virtual device)").small().color(TEXT_DIM));
                if device_combo(ui, "mic_cable", &mut self.mic_cable, &outputs, |_| true) {
                    self.apply_audio_state();
                }
                ui.label(
                    RichText::new("Incoming captured from (virtual device)")
                        .small()
                        .color(TEXT_DIM),
                );
                if device_combo(ui, "spk_cable", &mut self.spk_cable, &inputs, |_| true) {
                    self.apply_audio_state();
                }
                if ui.button("Rescan devices").clicked() {
                    self.rescan();
                }
                ui.separator();
                for (name, sec) in [("Mic", &self.mic), ("Incoming", &self.spk)] {
                    if let (Some(shared), Some(p)) = (&sec.shared, &sec.pipeline) {
                        ui.label(
                            RichText::new(format!(
                                "{name}: {} ({}) → {} ({}) · {:.1} ms/frame · underruns {}",
                                p.input_name,
                                p.input_rate,
                                p.output_name,
                                p.output_rate,
                                shared.proc_us.load(Ordering::Relaxed) as f32 / 1000.0,
                                shared.underruns.load(Ordering::Relaxed),
                            ))
                            .small()
                            .color(TEXT_FAINT),
                        );
                    }
                }
            });
    }

    fn ui_meetings(&mut self, ui: &mut egui::Ui) {
        if self.meetings_dirty {
            self.refresh_meetings();
        }
        let mut any_done = false;
        self.jobs.retain(|j| {
            let done = *j.done.lock().unwrap();
            any_done |= done;
            !done
        });
        if any_done {
            self.refresh_meetings();
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Meetings").size(17.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open folder").clicked() {
                    let _ = open::that(&self.cfg.recordings_dir);
                }
                if ui.button("⟳").clicked() {
                    self.meetings_dirty = true;
                }
            });
        });
        ui.add_space(4.0);

        if self.meetings.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No recordings yet").color(TEXT_DIM));
                ui.label(
                    RichText::new("Hit ⏺ Record meeting on Home — audio and transcripts land here.")
                        .small()
                        .color(TEXT_FAINT),
                );
            });
            return;
        }

        let meetings = self.meetings.clone();
        {
            let ui = &mut *ui;
            for m in &meetings {
                let being_recorded = self
                    .recording
                    .as_ref()
                    .is_some_and(|ar| ar.meeting.dir == m.dir);
                card(ui, |ui| {
                    if self.editing.as_ref().is_some_and(|(d, _, _)| *d == m.dir) {
                        let (dir, mut title_buf, mut parts_buf) = self.editing.take().unwrap();
                        ui.label(RichText::new("Title").small().color(TEXT_FAINT));
                        ui.text_edit_singleline(&mut title_buf);
                        ui.label(RichText::new("Participants (comma-separated)").small().color(TEXT_FAINT));
                        ui.text_edit_singleline(&mut parts_buf);
                        let mut closed = false;
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                let mut updated = m.clone();
                                updated.meta.title = title_buf.trim().to_string();
                                updated.meta.participants = parts_buf
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                let _ = meetings::save_meta(&updated);
                                self.meetings_dirty = true;
                                closed = true;
                            }
                            if ui.button("Cancel").clicked() {
                                closed = true;
                            }
                        });
                        if !closed {
                            self.editing = Some((dir, title_buf, parts_buf));
                        }
                        return;
                    }

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&m.meta.title).strong().color(TEXT));
                        if being_recorded {
                            chip(ui, "recording", RED);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if m.meta.duration_secs > 0.0 {
                                chip(ui, &fmt_dur(m.meta.duration_secs), TEXT_DIM);
                            }
                            if m.has_transcript {
                                chip(ui, "transcript", ACCENT);
                            }
                        });
                    });
                    let created = fmt_created(&m.meta.created);
                    let sub = if m.meta.participants.is_empty() {
                        created
                    } else {
                        format!("{created}   ·   {}", m.meta.participants.join(", "))
                    };
                    if !sub.trim().is_empty() {
                        ui.label(RichText::new(sub).small().color(TEXT_FAINT));
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if ui.small_button("Edit").clicked() {
                            self.editing = Some((
                                m.dir.clone(),
                                m.meta.title.clone(),
                                m.meta.participants.join(", "),
                            ));
                        }
                        if ui.small_button("Reveal").clicked() {
                            let _ = open::that(&m.dir);
                        }
                        let job = self.jobs.iter().find(|j| j.meeting_dir == m.dir);
                        if let Some(j) = job {
                            ui.spinner();
                            ui.label(
                                RichText::new(j.status.lock().unwrap().clone())
                                    .small()
                                    .color(TEXT_DIM),
                            );
                        } else if m.has_audio && !being_recorded {
                            let label = if m.has_transcript { "Re-transcribe" } else { "Transcribe" };
                            if ui.small_button(label).clicked() {
                                self.jobs.push(transcribe::spawn(m.dir.clone()));
                            }
                        }
                        if m.has_transcript && ui.small_button("View transcript").clicked() {
                            if let Ok(text) =
                                std::fs::read_to_string(m.dir.join(meetings::TRANSCRIPT_FILE))
                            {
                                self.viewing_transcript = Some((m.meta.title.clone(), text));
                            }
                        }
                    });
                });
                ui.add_space(2.0);
            }
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Settings").size(17.0).strong());
        ui.add_space(4.0);

        card(ui, |ui| {
            section_label(ui, "RECORDINGS FOLDER");
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}", self.cfg.recordings_dir.display()))
                        .small()
                        .color(TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Change…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_directory(&self.cfg.recordings_dir)
                            .pick_folder()
                        {
                            self.cfg.recordings_dir = dir;
                            self.cfg.save();
                            self.meetings_dirty = true;
                        }
                    }
                });
            });
        });

        card(ui, |ui| {
            section_label(ui, "TRANSCRIPTION");
            if transcribe::model_available() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("On-device Whisper").color(TEXT));
                    chip(ui, "ready", ACCENT);
                });
                ui.label(
                    RichText::new("Runs locally, Metal-accelerated on Apple Silicon. Nothing leaves this device.")
                        .small()
                        .color(TEXT_FAINT),
                );
            } else if let Some(dl) = &self.model_dl {
                ui.horizontal(|ui| {
                    if !*dl.done.lock().unwrap() {
                        ui.spinner();
                    }
                    ui.label(RichText::new(dl.status.lock().unwrap().clone()).small().color(TEXT_DIM));
                });
            } else {
                ui.label(
                    RichText::new("The Whisper model (~148 MB) downloads automatically on first run.")
                        .small()
                        .color(TEXT_DIM),
                );
            }
        });

        card(ui, |ui| {
            section_label(ui, "MEETING APP SETUP");
            for (k, v) in [
                ("Microphone", "Vocalm Microphone"),
                ("Speaker", "Vocalm Speaker"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).small().color(TEXT_FAINT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(v).small().color(TEXT_DIM));
                    });
                });
            }
            ui.label(
                RichText::new(
                    "Pick these once in Zoom / Teams / Meet / Discord; choose your real \
                     devices here in Vocalm. On Windows, VB-CABLE stands in for them.",
                )
                .small()
                .color(TEXT_FAINT),
            );
        });

        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(format!(
                    "Vocalm v{}  ·  free & on-device  ·  config: {}",
                    env!("CARGO_PKG_VERSION"),
                    config::config_dir().display()
                ))
                .small()
                .color(TEXT_FAINT),
            );
        });
    }
}
