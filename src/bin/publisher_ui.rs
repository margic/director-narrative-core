//! egui/eframe publisher status window.
//!
//! Single 480×640 read-only window. Runs on the main thread; the publisher
//! pipeline runs on a background thread sharing state via
//! `Arc<Mutex<PublisherStatus>>`.

#![cfg(target_os = "windows")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, RichText, ScrollArea, Vec2};

use director_narrative_core::publisher_status::PublisherStatus;

// ── Colours ────────────────────────────────────────────────────────────────

const GREEN:  Color32 = Color32::from_rgb(0x4c, 0xaf, 0x50);
const AMBER:  Color32 = Color32::from_rgb(0xff, 0x98, 0x00);
const RED:    Color32 = Color32::from_rgb(0xf4, 0x43, 0x36);
const GREY:   Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
const CYAN:   Color32 = Color32::from_rgb(0x00, 0xbc, 0xd4);
const WHITE:  Color32 = Color32::WHITE;
const DIM:    Color32 = Color32::from_rgb(0x88, 0x88, 0x88);

// ── App ────────────────────────────────────────────────────────────────────

pub struct PublisherApp {
    status: Arc<Mutex<PublisherStatus>>,
    /// Shutdown flag — set when the window is closed so the pipeline thread
    /// can flush and exit.
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl PublisherApp {
    pub fn new(
        status:  Arc<Mutex<PublisherStatus>>,
        running: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self { status, running }
    }
}

impl eframe::App for PublisherApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Re-paint every 500 ms so the UI stays fresh without busy-looping.
        ctx.request_repaint_after(Duration::from_millis(500));

        let status = self.status.lock().unwrap();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_min_size(Vec2::new(460.0, 620.0));

            // ── iRacing block ────────────────────────────────────────────
            ui.add_space(4.0);
            iracing_block(ui, &status);

            ui.add_space(6.0);
            ui.separator();

            // ── Race Control block ───────────────────────────────────────
            ui.add_space(6.0);
            rc_block(ui, &status);

            ui.add_space(6.0);
            ui.separator();

            // ── Counter bar ──────────────────────────────────────────────
            ui.add_space(4.0);
            counter_bar(ui, &status);

            ui.add_space(4.0);
            ui.separator();

            // ── Event log ────────────────────────────────────────────────
            ui.add_space(4.0);
            event_log(ui, &status);

            // ── Footer ───────────────────────────────────────────────────
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(4.0);
                if let Some(path) = &status.config_path {
                    let label = egui::Label::new(
                        RichText::new(format!("publisher.toml  ·  {path}"))
                            .color(DIM)
                            .small(),
                    )
                    .sense(egui::Sense::click());
                    if ui.add(label).clicked() {
                        let _ = std::process::Command::new("notepad").arg(path).spawn();
                    }
                }
            });
        });
    }
}

// ── Block renderers ────────────────────────────────────────────────────────

/// Draw a small filled circle as a status indicator.
fn status_dot(ui: &mut egui::Ui, colour: Color32) {
    let size = egui::vec2(14.0, 14.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, colour);
}

fn iracing_block(ui: &mut egui::Ui, s: &PublisherStatus) {
    let (dot, label) = if s.iracing_connected {
        (GREEN, "CONNECTED")
    } else {
        (GREY, "WAITING FOR SIMULATOR")
    };

    ui.horizontal(|ui| {
        status_dot(ui, dot);
        ui.label(RichText::new("iRacing").strong());
        ui.label(RichText::new(label).color(dot));
    });

    if s.iracing_connected {
        let track = s.track_name.as_deref().unwrap_or("—");
        let stype = s.session_type.as_deref().unwrap_or("—");
        let laps  = s.session_laps.as_deref().unwrap_or("—");
        ui.indent("ir_indent", |ui| {
            ui.label(format!("{track}  ·  {stype}  ·  Lap {}  /  {laps}", s.current_lap));
            if let Some(sid) = s.sub_session_id {
                ui.label(
                    RichText::new(format!(
                        "SubSession #{sid}  ·  Tick {}  ·  t={:.1}s",
                        s.session_tick, s.session_time_secs
                    ))
                    .color(DIM)
                    .small(),
                );
            }
        });
    }
}

fn rc_block(ui: &mut egui::Ui, s: &PublisherStatus) {
    let (dot, headline) = rc_status(s);

    ui.horizontal(|ui| {
        status_dot(ui, dot);
        ui.label(RichText::new("Race Control").strong());
        ui.label(RichText::new(headline).color(dot));
    });

    ui.indent("rc_indent", |ui| {
        // Token expiry
        if let Some(exp) = s.token_expires_at {
            let detail = match exp.duration_since(SystemTime::now()) {
                Ok(d)  => {
                    let mins = d.as_secs() / 60;
                    format!("Token expires in {mins} min")
                }
                Err(_) => "Token expired".to_owned(),
            };
            let colour = match exp.duration_since(SystemTime::now()) {
                Ok(d) if d.as_secs() < 300 => AMBER,
                Ok(_)                       => DIM,
                Err(_)                      => RED,
            };
            ui.label(RichText::new(detail).color(colour).small());
        }

        // Last HTTP status
        if let Some(code) = s.rc_last_http_status {
            let colour = if code == 200 || code == 201 || code == 202 { DIM } else { AMBER };
            ui.label(RichText::new(format!("Last response: {code}")).color(colour).small());
        }
    });
}

fn rc_status(s: &PublisherStatus) -> (Color32, &'static str) {
    if s.calls_total == 0 {
        return (GREY, "WAITING");
    }
    if s.rc_connected {
        (GREEN, "CONNECTED")
    } else {
        match s.rc_last_http_status {
            Some(401) | Some(403) => (RED,   "AUTH FAILED"),
            Some(c) if c >= 500   => (AMBER, "RETRYING"),
            _                     => (AMBER, "DEGRADED"),
        }
    }
}

fn counter_bar(ui: &mut egui::Ui, s: &PublisherStatus) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("EVENTS").strong().small());
        ui.label(RichText::new(format!("Total: {}", s.events_enqueued_total)).small());
        ui.separator();
        ui.label(RichText::new(format!("Calls: {}", s.calls_total)).small());
        ui.separator();
        let err_colour = if s.calls_failed > 0 { AMBER } else { DIM };
        ui.label(RichText::new(format!("Errors: {}", s.calls_failed)).color(err_colour).small());
    });
}

fn event_log(ui: &mut egui::Ui, s: &PublisherStatus) {
    ScrollArea::vertical()
        .id_salt("event_log")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &s.event_log {
                let (colour, bold) = event_colour(&entry.event_type);
                let text = format!(
                    "t={:>7.1}  {:<24}  #{} {}",
                    entry.session_time, entry.event_type,
                    entry.car_number, entry.driver_name,
                );
                let rt = if bold {
                    RichText::new(text).color(colour).strong().monospace()
                } else {
                    RichText::new(text).color(colour).monospace()
                };
                ui.label(rt);
            }
        });
}

fn event_colour(event_type: &str) -> (Color32, bool) {
    if event_type.starts_with("BATTLE_") {
        (AMBER, false)
    } else if matches!(event_type, "OVERTAKE" | "POSITION_LOST") {
        (GREEN, false)
    } else if event_type.starts_with("PIT_") {
        (CYAN, false)
    } else if matches!(event_type, "RACE_GREEN" | "RACE_CHECKERED") {
        (WHITE, true)
    } else if matches!(event_type, "LAP_COMPLETED") {
        (Color32::LIGHT_GRAY, false)
    } else {
        (DIM, false)
    }
}

// ── Entry point ────────────────────────────────────────────────────────────

/// Launch the eframe window. Blocks until the window is closed.
pub fn run_ui(
    status:  Arc<Mutex<PublisherStatus>>,
    running: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SimCenter Publisher")
            .with_inner_size([480.0, 640.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "SimCenter Publisher",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(PublisherApp::new(status, running)))
        }),
    )
}
