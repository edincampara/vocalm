#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod meetings;
mod pipeline;
mod recorder;
mod transcribe;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, RichText};
use engine::EngineKind;
use pipeline::{kind_to_u32, u32_to_kind, Pipeline, Shared};

const ACCENT: Color32 = Color32::from_rgb(45, 212, 191); // calm teal
const ACCENT_DIM: Color32 = Color32::from_rgb(19, 78, 74);
const CARD: Color32 = Color32::from_rgb(24, 30, 39);
const BG: Color32 = Color32::from_rgb(13, 17, 23);
const TEXT_DIM: Color32 = Color32::from_rgb(139, 148, 158);
const RED: Color32 = Color32::from_rgb(248, 81, 73);

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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 700.0])
            .with_min_inner_size([420.0, 560.0])
            .with_title("Vocalm"),
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

fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.override_text_color = Some(Color32::from_rgb(230, 237, 243));
    v.selection.bg_fill = ACCENT_DIM;
    v.hyperlink_color = ACCENT;
    v.widgets.hovered.bg_fill = ACCENT_DIM;
    v.widgets.active.bg_fill = ACCENT_DIM;
    ctx.set_style(style);
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
    fn level(&self) -> f32 {
        self.shared
            .as_ref()
            .map(|s| f32::from_bits(s.out_rms_bits.load(Ordering::Relaxed)))
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
/// mic and speaker paths never end up on the same cable (feedback of our own
/// clean mic into the "incoming" pipeline).
fn cable_base(name: &str) -> String {
    name.to_lowercase().replace("input", "").replace("output", "").trim().to_string()
}

/// Virtual output the mic path renders into (apps use it as their microphone).
/// Prefers Vocalm's own driver, falls back to BlackHole / VB-CABLE.
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
/// Must differ from the mic cable to avoid capturing our own clean mic.
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

// ---------- small widgets ----------

fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(46.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let bg = Color32::from_rgb(
            (48.0 + (ACCENT.r() as f32 - 48.0) * t) as u8,
            (54.0 + (ACCENT.g() as f32 - 54.0) * t) as u8,
            (61.0 + (ACCENT.b() as f32 - 61.0) * t) as u8,
        );
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, bg);
        let knob_x = rect.left() + radius + t * (rect.width() - 2.0 * radius);
        ui.painter().circle_filled(
            egui::pos2(knob_x, rect.center().y),
            radius - 3.0,
            Color32::WHITE,
        );
    }
    response
}

fn meter(ui: &mut egui::Ui, rms: f32) {
    let db = 20.0 * rms.max(1e-6).log10();
    let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 6.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 3.0, Color32::from_rgb(34, 41, 51));
    let mut fill = rect;
    fill.set_width(rect.width() * t);
    ui.painter().rect_filled(fill, 3.0, ACCENT);
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .fill(CARD)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| add(ui))
        .inner
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
        .width(ui.available_width() - 8.0)
        .selected_text(selected.clone().unwrap_or_else(|| "Select…".into()))
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

// ---------- app ----------

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_recording();
        self.persist();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(66));

        egui::TopBottomPanel::top("tabs")
            .frame(egui::Frame::default().fill(BG).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Vocalm").size(20.0).strong().color(ACCENT));
                    ui.add_space(12.0);
                    ui.selectable_value(&mut self.tab, Tab::Home, "Home");
                    if ui
                        .selectable_value(&mut self.tab, Tab::Meetings, "Meetings")
                        .clicked()
                    {
                        self.meetings_dirty = true;
                    }
                    ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                    if self.recording.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(RED, "● REC");
                        });
                    }
                });
            });

        if let Some((title, text)) = self.viewing_transcript.clone() {
            let mut open = true;
            egui::Window::new(format!("Transcript — {title}"))
                .open(&mut open)
                .default_size([420.0, 400.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(RichText::new(text).monospace());
                    });
                });
            if !open {
                self.viewing_transcript = None;
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Home => self.ui_home(ui),
            Tab::Meetings => self.ui_meetings(ui),
            Tab::Settings => self.ui_settings(ui),
        });
    }
}

impl App {
    fn ui_home(&mut self, ui: &mut egui::Ui) {
        // ---- master row ----
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (status, color) = if !self.master_on {
                ("Noise cancellation off", TEXT_DIM)
            } else if self.mic.active() {
                ("Your voice is protected", ACCENT)
            } else {
                ("Waiting for devices…", TEXT_DIM)
            };
            ui.label(RichText::new(status).size(16.0).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if toggle(ui, &mut self.master_on).changed() {
                    self.apply_audio_state();
                }
            });
        });
        if let Some(dl) = &self.model_dl {
            if !*dl.done.lock().unwrap() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(dl.status.lock().unwrap().clone())
                            .small()
                            .color(TEXT_DIM),
                    );
                });
            }
        }
        ui.add_space(4.0);

        // ---- your voice ----
        let inputs = self.inputs.clone();
        let outputs = self.outputs.clone();
        card(ui, |ui| {
            ui.label(RichText::new("YOUR VOICE").small().color(TEXT_DIM));
            if device_combo(ui, "mic_in", &mut self.mic_in, &inputs, is_real_mic) {
                self.apply_audio_state();
            }
            meter(ui, self.mic.level());
            match (&self.mic_cable, self.master_on && self.mic.active()) {
                (Some(cable), true) => {
                    ui.label(
                        RichText::new(format!(
                            "In your meeting app, choose microphone: “{}”",
                            app_facing_name(cable)
                        ))
                        .small()
                        .color(TEXT_DIM),
                    );
                }
                (None, _) => {
                    ui.colored_label(
                        Color32::YELLOW,
                        "Virtual device missing — install BlackHole (macOS) or VB-CABLE \
                         (Windows). See Settings → Setup.",
                    );
                }
                _ => {}
            }
            if let Some(e) = &self.mic.error {
                ui.colored_label(RED, e.clone());
            }
        });

        // ---- incoming audio ----
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("INCOMING AUDIO").small().color(TEXT_DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut on = self.spk_on;
                    if toggle(ui, &mut on).changed() {
                        self.spk_on = on;
                        self.apply_audio_state();
                    }
                });
            });
            ui.label(
                RichText::new(
                    "Removes background noise from what other people's mics pick up.",
                )
                .small()
                .color(TEXT_DIM),
            );
            if self.spk_cable.is_none() {
                ui.colored_label(
                    Color32::YELLOW,
                    "Needs a second virtual device: `brew install blackhole-16ch` (macOS) or a \
                     second VB-CABLE (Windows), then Rescan in Advanced.",
                );
            } else if self.spk_on {
                ui.label("Play on:");
                if device_combo(ui, "spk_out", &mut self.spk_out, &outputs, is_real_output) {
                    self.apply_audio_state();
                }
                meter(ui, self.spk.level());
                if let Some(cable) = &self.spk_cable {
                    ui.label(
                        RichText::new(format!(
                            "In your meeting app, choose speaker: “{}”",
                            app_facing_name(cable)
                        ))
                        .small()
                        .color(TEXT_DIM),
                    );
                }
                if let Some(e) = &self.spk.error {
                    ui.colored_label(RED, e.clone());
                }
            }
        });

        // ---- record ----
        let rec_label = match &self.recording {
            None => "⏺  Record meeting".to_string(),
            Some(ar) => format!(
                "⏹  Stop recording  {}",
                fmt_dur(ar.started.elapsed().as_secs_f32())
            ),
        };
        let rec_color = if self.recording.is_some() { RED } else { ACCENT_DIM };
        let btn = egui::Button::new(RichText::new(rec_label).size(15.0)).fill(rec_color);
        if ui.add_enabled(self.mic.active(), btn).clicked() {
            if self.recording.is_some() {
                self.stop_recording();
            } else {
                self.start_recording();
            }
        }
        if self.recording.is_some() && self.spk.active() {
            ui.label(
                RichText::new(
                    "Recording both sides: you (left channel) + others (right channel)",
                )
                .small()
                .color(TEXT_DIM),
            );
        }

        // ---- advanced ----
        ui.add_space(6.0);
        egui::CollapsingHeader::new(RichText::new("Advanced").color(TEXT_DIM))
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Engine:");
                ui.horizontal(|ui| {
                    for (kind, label) in [
                        (EngineKind::DeepFilter, "DeepFilterNet3 (best)"),
                        (EngineKind::Rnnoise, "RNNoise (lowest latency)"),
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
                ui.label("Mic routes to (virtual device):");
                if device_combo(ui, "mic_cable", &mut self.mic_cable, &outputs, |_| true) {
                    self.apply_audio_state();
                }
                ui.label("Incoming captured from (virtual device):");
                if device_combo(ui, "spk_cable", &mut self.spk_cable, &inputs, |_| true) {
                    self.apply_audio_state();
                }
                if ui.button("Rescan devices").clicked() {
                    self.rescan();
                }
                ui.separator();
                for (name, sec) in [("Mic", &self.mic), ("Incoming", &self.spk)] {
                    if let (Some(shared), Some(p)) = (&sec.shared, &sec.pipeline) {
                        ui.label(format!(
                            "{name}: {} ({} Hz) → {} ({} Hz) · DSP {:.1} ms/frame · underruns {}",
                            p.input_name,
                            p.input_rate,
                            p.output_name,
                            p.output_rate,
                            shared.proc_us.load(Ordering::Relaxed) as f32 / 1000.0,
                            shared.underruns.load(Ordering::Relaxed),
                        ));
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
            ui.heading("Meetings");
            if ui.button("⟳").clicked() {
                self.meetings_dirty = true;
            }
            if ui.button("Open folder").clicked() {
                let _ = open::that(&self.cfg.recordings_dir);
            }
        });
        ui.label(
            RichText::new(format!("{}", self.cfg.recordings_dir.display()))
                .small()
                .color(TEXT_DIM),
        );
        ui.separator();

        if self.meetings.is_empty() {
            ui.label("No recordings yet. Use ⏺ Record meeting on the Home tab.");
            return;
        }

        let meetings = self.meetings.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for m in &meetings {
                let being_recorded = self
                    .recording
                    .as_ref()
                    .is_some_and(|ar| ar.meeting.dir == m.dir);
                card(ui, |ui| {
                    if self.editing.as_ref().is_some_and(|(d, _, _)| *d == m.dir) {
                        let (dir, mut title_buf, mut parts_buf) = self.editing.take().unwrap();
                        ui.horizontal(|ui| {
                            ui.label("Title:");
                            ui.text_edit_singleline(&mut title_buf);
                        });
                        ui.label("Participants (comma-separated):");
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
                        ui.strong(&m.meta.title);
                        if being_recorded {
                            ui.colored_label(RED, "● recording");
                        }
                        if m.meta.duration_secs > 0.0 {
                            ui.label(
                                RichText::new(fmt_dur(m.meta.duration_secs))
                                    .small()
                                    .color(TEXT_DIM),
                            );
                        }
                    });
                    if !m.meta.participants.is_empty() {
                        ui.label(
                            RichText::new(format!("👥 {}", m.meta.participants.join(", ")))
                                .small()
                                .color(TEXT_DIM),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("✏ Edit").clicked() {
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
                            ui.label(j.status.lock().unwrap().clone());
                        } else if m.has_audio && !being_recorded {
                            let label = if m.has_transcript {
                                "Re-transcribe"
                            } else {
                                "Transcribe"
                            };
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
        });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.separator();

        card(ui, |ui| {
            ui.label(RichText::new("RECORDINGS").small().color(TEXT_DIM));
            ui.horizontal(|ui| {
                ui.monospace(format!("{}", self.cfg.recordings_dir.display()));
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

        card(ui, |ui| {
            ui.label(RichText::new("TRANSCRIPTION").small().color(TEXT_DIM));
            if transcribe::model_available() {
                ui.colored_label(ACCENT, "On-device Whisper model installed ✓");
            } else if let Some(dl) = &self.model_dl {
                ui.horizontal(|ui| {
                    if !*dl.done.lock().unwrap() {
                        ui.spinner();
                    }
                    ui.label(dl.status.lock().unwrap().clone());
                });
            } else {
                ui.label(
                    "The Whisper model (~148 MB) downloads automatically. Everything runs \
                     on your device.",
                );
            }
        });

        card(ui, |ui| {
            ui.label(RichText::new("SETUP").small().color(TEXT_DIM));
            ui.label("In your meeting app (Zoom, Teams, Meet, Discord):");
            ui.label("• Microphone → “Vocalm Microphone”");
            ui.label("• Speaker → “Vocalm Speaker”");
            ui.label("Then pick your real mic and speakers here in Vocalm. That's it.");
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "The Vocalm audio devices are installed by the Vocalm installer (macOS). \
                     On Windows, VB-CABLE fills that role until our driver ships.",
                )
                .small()
                .color(TEXT_DIM),
            );
        });

        ui.label(
            RichText::new(format!(
                "Vocalm v{} · config {}",
                env!("CARGO_PKG_VERSION"),
                config::config_dir().display()
            ))
            .small()
            .color(TEXT_DIM),
        );
    }
}
