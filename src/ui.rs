//! Vocalm design system — "Calm Tech".
//!
//! Near-black blue canvas; surfaces emerge as charcoal cards with hairline
//! borders. One teal accent, used only where it carries meaning (status, live
//! audio). Red exists solely for recording. Everything animates gently.

use eframe::egui::{self, Color32, Rect, RichText, Stroke, Ui};

// ---- palette ----
pub const BG: Color32 = Color32::from_rgb(10, 13, 19); // canvas
pub const SURFACE: Color32 = Color32::from_rgb(20, 25, 38); // cards
pub const SURFACE_HI: Color32 = Color32::from_rgb(28, 35, 52); // hover / inputs
pub const BORDER: Color32 = Color32::from_rgb(34, 43, 60); // hairlines
pub const TEXT: Color32 = Color32::from_rgb(232, 237, 244);
pub const TEXT_DIM: Color32 = Color32::from_rgb(139, 149, 165);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(84, 94, 112);
pub const ACCENT: Color32 = Color32::from_rgb(45, 212, 191); // calm teal
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(16, 60, 56); // teal wash
pub const RED: Color32 = Color32::from_rgb(255, 99, 99); // record only
pub const AMBER: Color32 = Color32::from_rgb(229, 192, 123); // gentle warnings

pub fn apply_theme(ctx: &egui::Context) {
    // Vocalm is always dark — pin the theme so the OS light mode can't swap in
    // default-light widget styling (combo popups, sliders, …).
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(customize_style);
}

fn customize_style(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};

    style.text_styles = [
        (TextStyle::Heading, FontId::new(21.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(16);

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT_SOFT;
    v.hyperlink_color = ACCENT;

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = SURFACE_HI;
    v.widgets.inactive.weak_bg_fill = SURFACE_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = SURFACE_HI;
    v.widgets.hovered.weak_bg_fill = SURFACE_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(58, 72, 96));
    v.widgets.active.bg_fill = ACCENT_SOFT;
    v.widgets.active.weak_bg_fill = ACCENT_SOFT;
    v.widgets.open.bg_fill = SURFACE_HI;
    v.widgets.open.weak_bg_fill = SURFACE_HI;
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
}

// ---- primitives ----

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(egui::Margin::same(14))
        .outer_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .small()
            .color(TEXT_FAINT)
            .extra_letter_spacing(1.2),
    );
}

pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::default()
        .fill(Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 24))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

/// iOS-style animated switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let track = lerp_color(SURFACE_HI, ACCENT, t);
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, track);
        let knob_x = rect.left() + radius + t * (rect.width() - 2.0 * radius);
        let knob = egui::pos2(knob_x, rect.center().y);
        ui.painter()
            .circle_filled(knob + egui::vec2(0.0, 0.5), radius - 2.5, Color32::from_black_alpha(60));
        ui.painter().circle_filled(knob, radius - 3.0, Color32::WHITE);
    }
    response
}

/// Level meter with a ghost track for the raw signal behind the clean one —
/// the visible gap IS the removed noise.
pub fn dual_meter(ui: &mut Ui, raw_t: f32, clean_t: f32, active: bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 5.0), egui::Sense::hover());
    let r = 2.5;
    ui.painter().rect_filled(rect, r, Color32::from_rgb(15, 19, 28));
    if active {
        let mut ghost = rect;
        ghost.set_width(rect.width() * raw_t.clamp(0.0, 1.0));
        ui.painter()
            .rect_filled(ghost, r, Color32::from_rgba_unmultiplied(139, 149, 165, 46));
        let mut fill = rect;
        fill.set_width(rect.width() * clean_t.clamp(0.0, 1.0));
        ui.painter().rect_filled(fill, r, ACCENT);
    }
}

/// Normalized 0..1 position on a 60 dB meter for an RMS value.
pub fn db_norm(rms: f32) -> f32 {
    let db = 20.0 * rms.max(1e-6).log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The hero: a breathing orb lit by the cleaned voice. A faint ghost ring
/// echoes the raw mic level so you can see the noise being stripped away.
/// Click to pause / resume. Returns true when clicked.
pub fn orb(ui: &mut Ui, on: bool, clean_level: f32, raw_level: f32, recording: bool) -> bool {
    let size = 148.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let c = rect.center();
    let p = ui.painter();

    let t_on = ui.ctx().animate_bool_responsive(response.id.with("on"), on);
    let breathe = if on {
        let time = ui.input(|i| i.time) as f32;
        (time * 1.4).sin() * 0.5 + 0.5
    } else {
        0.0
    };
    let level = clean_level.clamp(0.0, 1.0);
    let raw = raw_level.clamp(0.0, 1.0);
    let base_r = 44.0;

    // ghost ring: the raw, noisy world outside
    if on && raw > 0.02 {
        p.circle_stroke(
            c,
            base_r + 10.0 + raw * 26.0,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(139, 149, 165, 40)),
        );
    }
    // teal glow: the clean voice
    let glow = (level * 0.85 + breathe * 0.15) * t_on;
    for (extra, alpha) in [(26.0, 14.0), (18.0, 22.0), (10.0, 34.0), (4.0, 48.0)] {
        p.circle_filled(
            c,
            base_r + extra * (0.35 + glow),
            Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), (alpha * glow) as u8),
        );
    }
    // core disc
    let hover = response.hovered();
    p.circle_filled(c, base_r, if hover { SURFACE_HI } else { SURFACE });
    p.circle_stroke(
        c,
        base_r,
        Stroke::new(1.5, lerp_color(BORDER, ACCENT, t_on * (0.45 + glow * 0.55))),
    );

    // microphone pictogram, drawn — capsule, cradle, stem, base
    let icon = if on {
        lerp_color(TEXT_DIM, ACCENT, 0.35 + glow * 0.65)
    } else {
        TEXT_FAINT
    };
    let s = 1.15;
    let capsule = Rect::from_center_size(c + egui::vec2(0.0, -9.0 * s), egui::vec2(16.0 * s, 26.0 * s));
    p.rect_filled(capsule, 8.0 * s, icon);
    let cradle_r = 14.0 * s;
    let cradle_c = c + egui::vec2(0.0, -2.0 * s);
    let pts: Vec<egui::Pos2> = (0..=20)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 20.0);
            cradle_c + egui::vec2(a.cos() * cradle_r, a.sin() * cradle_r)
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0 * s, icon)));
    p.line_segment(
        [cradle_c + egui::vec2(0.0, cradle_r), cradle_c + egui::vec2(0.0, cradle_r + 7.0 * s)],
        Stroke::new(2.0 * s, icon),
    );
    p.line_segment(
        [
            cradle_c + egui::vec2(-7.0 * s, cradle_r + 7.0 * s),
            cradle_c + egui::vec2(7.0 * s, cradle_r + 7.0 * s),
        ],
        Stroke::new(2.0 * s, icon),
    );

    // recording beacon
    if recording {
        let time = ui.input(|i| i.time) as f32;
        let pulse = ((time * 2.2).sin() * 0.5 + 0.5) * 0.6 + 0.4;
        p.circle_filled(
            c + egui::vec2(base_r * 0.72, -base_r * 0.72),
            5.0,
            Color32::from_rgba_unmultiplied(RED.r(), RED.g(), RED.b(), (255.0 * pulse) as u8),
        );
    }
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Animated segmented control. Returns the newly selected index if changed.
pub fn segmented(ui: &mut Ui, id: &str, labels: &[&str], active: usize) -> Option<usize> {
    let pad_x = 14.0;
    let h = 26.0;
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| {
            let galley = ui.painter().layout_no_wrap(
                (*l).to_string(),
                egui::TextStyle::Button.resolve(ui.style()),
                TEXT,
            );
            galley.rect.width() + pad_x * 2.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0 * 2.0;
    let (outer, _) = ui.allocate_exact_size(egui::vec2(total, h + 8.0), egui::Sense::hover());
    let p = ui.painter();
    let track = Rect::from_center_size(outer.center(), egui::vec2(total, h + 8.0));
    p.rect_filled(track, (h + 8.0) / 2.0, Color32::from_rgb(15, 19, 28));

    // animated pill under the active tab
    let mut x = track.left() + 4.0;
    let mut pill_x = x;
    let mut pill_w = widths[0];
    for (i, w) in widths.iter().enumerate() {
        if i == active {
            pill_x = x;
            pill_w = *w;
        }
        x += w;
    }
    let anim_x = ui.ctx().animate_value_with_time(
        egui::Id::new(id).with("pill_x"),
        pill_x,
        0.14,
    );
    let anim_w = ui.ctx().animate_value_with_time(
        egui::Id::new(id).with("pill_w"),
        pill_w,
        0.14,
    );
    let pill = Rect::from_min_size(
        egui::pos2(anim_x, track.center().y - h / 2.0),
        egui::vec2(anim_w, h),
    );
    p.rect_filled(pill, h / 2.0, SURFACE_HI);
    p.rect_stroke(pill, h / 2.0, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

    // labels + interaction
    let mut clicked = None;
    let mut x = track.left() + 4.0;
    for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(egui::pos2(x, track.top() + 4.0), egui::vec2(*w, h));
        let resp = ui.interact(r, egui::Id::new(id).with(i), egui::Sense::click());
        let color = if i == active {
            TEXT
        } else if resp.hovered() {
            TEXT_DIM
        } else {
            TEXT_FAINT
        };
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            egui::TextStyle::Button.resolve(ui.style()),
            color,
        );
        if resp.clicked() && i != active {
            clicked = Some(i);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        x += w;
    }
    clicked
}

/// Full-width pill button used for Record. Returns response.
pub fn big_button(ui: &mut Ui, text: &str, fill: Color32, text_color: Color32, enabled: bool) -> egui::Response {
    let h = 40.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h),
        if enabled { egui::Sense::click() } else { egui::Sense::hover() },
    );
    let p = ui.painter();
    let fill = if !enabled {
        Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 70)
    } else if response.hovered() {
        lerp_color(fill, Color32::WHITE, 0.06)
    } else {
        fill
    };
    p.rect_filled(rect, h / 2.0, fill);
    p.rect_stroke(rect, h / 2.0, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    let tc = if enabled { text_color } else { TEXT_FAINT };
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        tc,
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Smooth a live level: fast attack, slow release. Store per-frame in App.
pub fn smooth_level(current: &mut f32, target: f32) {
    if target > *current {
        *current += (target - *current) * 0.55;
    } else {
        *current *= 0.90;
    }
}
