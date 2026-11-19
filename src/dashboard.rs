//! Pip's dashboard: the window you get when you left-click the tray icon.
//! It runs as its own process (`thermo-companion.exe --dashboard`) so the tray stays tiny.

use crate::app::{self, Notify, Shared};
use crate::config::{self, Config};
use crate::data::{self, Data};
use crate::escpos::{self, TextOpts};
use crate::pip::{self, Mood};
use crate::{autostart, gcal, printer, raster};
use chrono::{Local, Timelike};
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Ui};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const WINDOW_TITLE: &str = "Thermo Companion";

// --- palette: night-desk engineer
const BG: Color32 = Color32::from_rgb(10, 14, 21);
const PANEL: Color32 = Color32::from_rgb(15, 21, 31);
const CARD: Color32 = Color32::from_rgb(20, 28, 41);
const CARD2: Color32 = Color32::from_rgb(28, 39, 56);
const BORDER: Color32 = Color32::from_rgb(38, 52, 74);
const CYAN: Color32 = Color32::from_rgb(92, 225, 230);
const AMBER: Color32 = Color32::from_rgb(255, 184, 77);
const GREEN: Color32 = Color32::from_rgb(120, 230, 140);
const RED: Color32 = Color32::from_rgb(255, 110, 110);
const PINK: Color32 = Color32::from_rgb(255, 120, 170);
const DIM: Color32 = Color32::from_rgb(130, 146, 170);
const TEXT: Color32 = Color32::from_rgb(222, 230, 242);
const INK: Color32 = Color32::from_rgb(14, 18, 26);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Print,
    Schedule,
    Tasks,
    Alerts,
    Settings,
}

impl Page {
    const ALL: [Page; 6] = [Page::Home, Page::Print, Page::Schedule, Page::Tasks, Page::Alerts, Page::Settings];

    fn path(self) -> &'static str {
        match self {
            Page::Home => "~/home",
            Page::Print => "~/print",
            Page::Schedule => "~/schedule",
            Page::Tasks => "~/tasks",
            Page::Alerts => "~/alerts",
            Page::Settings => "~/settings",
        }
    }

    fn from_arg(s: &str) -> Page {
        match s {
            "print" => Page::Print,
            "schedule" => Page::Schedule,
            "tasks" => Page::Tasks,
            "alerts" => Page::Alerts,
            "settings" => Page::Settings,
            _ => Page::Home,
        }
    }
}

struct Textures {
    happy: egui::TextureHandle,
    wink: egui::TextureHandle,
    sleepy: egui::TextureHandle,
    busy: egui::TextureHandle,
    party: egui::TextureHandle,
    sad: egui::TextureHandle,
    logo: egui::TextureHandle,
}

impl Textures {
    fn get(&self, m: Mood) -> &egui::TextureHandle {
        match m {
            Mood::Happy => &self.happy,
            Mood::Wink => &self.wink,
            Mood::Sleepy => &self.sleepy,
            Mood::Busy => &self.busy,
            Mood::Party => &self.party,
            Mood::Sad => &self.sad,
        }
    }
}

fn load_tex(ctx: &egui::Context, name: &str, bytes: &[u8]) -> egui::TextureHandle {
    let img = image::load_from_memory(bytes).expect("built-in image").to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(7, 10, 15);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(38, 110, 120);
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.hyperlink_color = CYAN;
    let r = CornerRadius::same(8);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = CARD2;
    v.widgets.inactive.weak_bg_fill = CARD2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = Color32::from_rgb(36, 52, 74);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(36, 52, 74);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.active.bg_fill = Color32::from_rgb(60, 48, 22);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(60, 48, 22);
    v.widgets.active.bg_stroke = Stroke::new(1.0, AMBER);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(10.0, 10.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.text_styles.insert(TextStyle::Heading, FontId::new(26.0, FontFamily::Monospace));
        s.text_styles.insert(TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(15.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace));
        s.text_styles.insert(TextStyle::Small, FontId::new(12.0, FontFamily::Proportional));
    });
}

// ------------------------------------------------------------ small widgets

fn mono(text: &str, color: Color32) -> RichText {
    RichText::new(text).monospace().color(color)
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    // the frame's margins and stroke come on top of the width we ask for
    let inner = (ui.available_width() - 34.0).max(100.0);
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(inner);
            add(ui)
        });
}

fn section(ui: &mut Ui, tag: &str, title: &str, add: impl FnOnce(&mut Ui)) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(mono(tag, CYAN));
            ui.label(RichText::new(title).strong().size(17.0));
        });
        ui.add_space(2.0);
        add(ui);
    });
}

fn page_title(ui: &mut Ui, path: &str, sub: &str) {
    ui.label(RichText::new(format!("$ cd {path}")).monospace().size(24.0).color(AMBER));
    ui.label(mono(sub, DIM));
    ui.add_space(6.0);
}

fn action(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).monospace().color(INK).strong())
            .fill(color)
            .corner_radius(CornerRadius::same(8)),
    )
}

fn ghost(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).monospace()))
}

fn toggle(ui: &mut Ui, on: &mut bool, label: &str) {
    ui.checkbox(on, label);
}

fn time_edit(ui: &mut Ui, h: &mut u32, m: &mut u32) {
    ui.add(egui::DragValue::new(h).range(0..=23).custom_formatter(|n, _| format!("{:02}", n as u32)));
    ui.label(mono(":", DIM));
    ui.add(egui::DragValue::new(m).range(0..=59).custom_formatter(|n, _| format!("{:02}", n as u32)));
}

fn text_field(ui: &mut Ui, s: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add(egui::TextEdit::singleline(s).hint_text(hint).desired_width(width))
}

fn pill(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 38))
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().size(12.0).color(color));
        });
}

fn wifi_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn fmt_countdown(secs: i64) -> String {
    let s = secs.max(0);
    format!("{:02}:{:02}", s / 60, s % 60)
}

// -------------------------------------------------------------------- app

struct Dash {
    sh: Arc<Shared>,
    notify: Notify,
    page: Page,
    tex: Textures,
    cfg: Config,
    data: Data,
    data_at: Instant,
    printers: Vec<String>,
    printers_at: Instant,
    status: String,
    status_at: Instant,
    just_done: Option<Instant>,
    started: Instant,

    // print page
    txt: String,
    t_bold: bool,
    t_big: bool,
    t_center: bool,
    t_stamp: bool,
    t_check: bool,
    img_path: String,
    img_bright: f32,
    img_smooth: bool,
    img_caption: String,
    img_preview: Option<egui::TextureHandle>,
    img_key: (String, i32, bool),
    qr_text: String,
    wifi_ssid: String,
    wifi_pass: String,
    wifi_open: bool,
    sticker: usize,

    // schedule / tasks / alerts
    new_reminder: String,
    reminder_min: u32,
    pomo_task: String,
    new_todo: String,
    new_habit: String,
    new_url: String,
    new_proc: String,

    // google
    google: Arc<Mutex<Option<Result<String, String>>>>,
    google_busy: bool,
    connect_mail: bool,
}

const STICKERS: [(&str, Mood); 5] = [
    ("happy", Mood::Happy),
    ("party", Mood::Party),
    ("sleepy", Mood::Sleepy),
    ("busy", Mood::Busy),
    ("sad", Mood::Sad),
];

impl Dash {
    fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>, page: Option<String>) -> Dash {
        let ctx = cc.egui_ctx.clone();
        apply_theme(&ctx);
        let tex = Textures {
            happy: load_tex(&ctx, "pip_happy", pip::HAPPY),
            wink: load_tex(&ctx, "pip_wink", pip::WINK),
            sleepy: load_tex(&ctx, "pip_sleepy", pip::SLEEPY),
            busy: load_tex(&ctx, "pip_busy", pip::BUSY),
            party: load_tex(&ctx, "pip_party", pip::PARTY),
            sad: load_tex(&ctx, "pip_sad", pip::SAD),
            logo: load_tex(&ctx, "logo", pip::LOGO),
        };
        let notify_ctx = ctx.clone();
        let notify: Notify = Arc::new(move || notify_ctx.request_repaint());
        let cfg = shared.cfg();

        // Fetch the calendar in the background so the dashboard has something to show.
        if !cfg.google_refresh_token.is_empty() {
            app::spawn_job(&shared, notify.clone(), "Calendar updated", |s| s.refresh_events());
        }

        Dash {
            sh: shared,
            notify,
            page: page.as_deref().map(Page::from_arg).unwrap_or(Page::Home),
            tex,
            cfg,
            data: data::load(),
            data_at: Instant::now(),
            printers: printer::list_printers(),
            printers_at: Instant::now(),
            status: "ready. pip is awake.".into(),
            status_at: Instant::now(),
            just_done: None,
            started: Instant::now(),
            txt: String::new(),
            t_bold: false,
            t_big: false,
            t_center: false,
            t_stamp: true,
            t_check: false,
            img_path: String::new(),
            img_bright: 0.0,
            img_smooth: true,
            img_caption: String::new(),
            img_preview: None,
            img_key: (String::new(), 0, true),
            qr_text: String::new(),
            wifi_ssid: String::new(),
            wifi_pass: String::new(),
            wifi_open: false,
            sticker: 0,
            new_reminder: String::new(),
            reminder_min: 10,
            pomo_task: String::new(),
            new_todo: String::new(),
            new_habit: String::new(),
            new_url: String::new(),
            new_proc: String::new(),
            google: Arc::new(Mutex::new(None)),
            google_busy: false,
            connect_mail: false,
        }
    }

    // ------------------------------------------------------------ plumbing

    fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_at = Instant::now();
    }

    fn job(&self, ok: &'static str, f: impl FnOnce(&Shared) -> Result<(), String> + Send + 'static) {
        app::spawn_job(&self.sh, self.notify.clone(), ok, f);
    }

    fn refresh_data(&mut self) {
        self.data = data::load();
        self.data_at = Instant::now();
    }

    fn change_data(&mut self, f: impl FnOnce(&mut Data)) {
        data::update(f);
        self.refresh_data();
    }

    fn paper_dots(&self) -> u32 {
        (self.cfg.paper_chars as u32 * 12).clamp(192, 576)
    }

    fn printer_ok(&self) -> bool {
        self.printers.contains(&self.cfg.printer)
    }

    fn mood(&self) -> Mood {
        if self.sh.jobs.load(Ordering::Relaxed) > 0 {
            return Mood::Busy;
        }
        if !self.printer_ok() {
            return Mood::Sad;
        }
        if self.just_done.map_or(false, |t| t.elapsed() < Duration::from_secs(8)) {
            return Mood::Party;
        }
        let todos = &self.data.todos;
        if !todos.is_empty() && todos.iter().all(|t| t.done) {
            return Mood::Party;
        }
        let hour = Local::now().hour();
        if hour >= 23 || hour < 5 {
            return Mood::Sleepy;
        }
        if (Local::now().timestamp() / 30) % 4 == 0 {
            Mood::Wink
        } else {
            Mood::Happy
        }
    }

    fn tick(&mut self, ctx: &egui::Context) {
        // settings changed by the tray (toggles) or the scheduler
        if self.sh.reload_if_changed() {
            self.cfg = self.sh.cfg();
        }
        if self.data_at.elapsed() > Duration::from_millis(700) {
            self.refresh_data();
        }
        if self.printers_at.elapsed() > Duration::from_secs(10) {
            self.printers = printer::list_printers();
            self.printers_at = Instant::now();
        }
        for m in self.sh.take_msgs() {
            if m.starts_with("Printed") || m.contains("printed") {
                self.just_done = Some(Instant::now());
            }
            self.say(m);
        }
        // finished Google sign-in
        let finished = self.google.lock().unwrap().take();
        if let Some(r) = finished {
            self.google_busy = false;
            match r {
                Ok(token) => {
                    self.cfg.google_refresh_token = token;
                    self.cfg.google_has_mail = self.connect_mail;
                    self.say("Google connected. Hi calendar!");
                    let c = self.cfg.clone();
                    self.sh.set_cfg(c);
                    app::spawn_job(&self.sh, self.notify.clone(), "Calendar updated", |s| s.refresh_events());
                }
                Err(e) => self.say(format!("Google sign-in failed: {e}")),
            }
        }
        // pictures dropped on the window
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            {
                let p = f.path().to_path_buf();
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if ["png", "jpg", "jpeg", "bmp", "gif", "webp"].contains(&ext.as_str()) {
                    self.img_path = p.to_string_lossy().to_string();
                    self.page = Page::Print;
                    self.say("Picture loaded. Check the preview, then print.");
                }
            }
        }
    }

    // ---------------------------------------------------------------- nav

    fn nav(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::from_texture(&self.tex.logo).fit_to_exact_size(egui::vec2(38.0, 38.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("Thermo").strong().size(17.0));
                ui.label(RichText::new("Companion").strong().size(17.0).color(AMBER));
            });
        });
        ui.add_space(14.0);
        for p in Page::ALL {
            let active = self.page == p;
            let label = if active { format!("> {}", p.path()) } else { format!("  {}", p.path()) };
            let color = if active { AMBER } else { DIM };
            let resp = ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::new(RichText::new(label).monospace().size(15.0).color(color))
                    .fill(if active { CARD2 } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(8)),
            );
            if resp.clicked() {
                self.page = p;
            }
        }
        ui.add_space(20.0);
        ui.separator();
        ui.label(mono(&format!("v{}", env!("CARGO_PKG_VERSION")), DIM));
        ui.horizontal_wrapped(|ui| {
            pill(ui, "rust", AMBER);
            pill(ui, "esc/pos", CYAN);
        });
        ui.label(mono("ctrl+alt+p", DIM));
        ui.label(mono("= print selection", DIM));
    }

    fn status_bar(&self, ui: &mut Ui) {
        let fresh = self.status_at.elapsed() < Duration::from_secs(12);
        ui.horizontal_centered(|ui| {
            ui.label(mono("pip@desk:~$", GREEN));
            ui.label(mono(&self.status, if fresh { TEXT } else { DIM }));
        });
    }

    // --------------------------------------------------------------- Home

    fn tile(&self, ui: &mut Ui, width: f32, tag: &str, value: &str, sub: &str, color: Color32) {
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.set_min_height(84.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(mono(tag, DIM));
                    ui.add(egui::Label::new(RichText::new(value).size(22.0).strong().color(color)).truncate());
                    ui.add(egui::Label::new(RichText::new(sub).size(12.0).color(DIM)).truncate());
                });
            });
    }

    fn page_home(&mut self, ui: &mut Ui) {
        let mood = self.mood();
        let now = Local::now();
        let slot = (now.timestamp() / 12) as u64;
        let said = pip::say(mood, now.hour(), slot);

        page_title(ui, "~/home", "// your desk buddy, live");

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::from_texture(self.tex.get(mood)).fit_to_exact_size(egui::vec2(190.0, 190.0)));
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(7, 10, 15))
                        .stroke(Stroke::new(1.0, CYAN))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width((ui.available_width() - 30.0).max(260.0));
                            ui.label(mono("pip@desk:~$ say", GREEN));
                            let cursor = if (self.started.elapsed().as_millis() / 500) % 2 == 0 { "_" } else { " " };
                            ui.label(RichText::new(format!("> {said}{cursor}")).monospace().size(16.0).color(CYAN));
                        });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        pill(ui, if self.printer_ok() { "printer online" } else { "printer missing" }, if self.printer_ok() { GREEN } else { RED });
                        pill(ui, if self.sh.connected() { "calendar linked" } else { "calendar off" }, if self.sh.connected() { GREEN } else { DIM });
                        if self.cfg.hotkey {
                            pill(ui, "hotkey on", CYAN);
                        }
                        if self.cfg.track_usage {
                            pill(ui, "tracking: local", PINK);
                        }
                    });
                });
            });
        });

        // status tiles
        let (focus_value, focus_sub, focus_color) = match self.countdown() {
            Some((phase, secs)) => (fmt_countdown(secs), format!("{phase} - running"), AMBER),
            None => ("idle".to_string(), "no focus timer".to_string(), DIM),
        };
        let open = self.data.todos.iter().filter(|t| !t.done).count();
        let next = self
            .sh
            .events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.start > now)
            .cloned();
        // four tiles that share the row evenly (each adds 28 px margin + 2 px stroke + 10 px gap)
        let tw = ((ui.available_width() - 4.0 * 30.0 - 3.0 * 10.0) / 4.0).max(110.0);
        ui.horizontal(|ui| {
            self.tile(
                ui,
                tw,
                "PRINTER",
                if self.printer_ok() { "online" } else { "offline" },
                &self.cfg.printer,
                if self.printer_ok() { GREEN } else { RED },
            );
            self.tile(ui, tw, "FOCUS", &focus_value, &focus_sub, focus_color);
            self.tile(ui, tw, "TASKS", &format!("{open} open"), &format!("{} total", self.data.todos.len()), CYAN);
            match &next {
                Some(e) => self.tile(ui, tw, "NEXT EVENT", &e.start.format("%a %H:%M").to_string(), &e.summary, AMBER),
                None => self.tile(ui, tw, "NEXT EVENT", "none", if self.sh.connected() { "calendar is clear" } else { "connect Google" }, DIM),
            }
        });

        // quick actions
        section(ui, "$", "quick actions", |ui| {
            ui.horizontal_wrapped(|ui| {
                if action(ui, "print agenda", AMBER).clicked() {
                    self.job("Printed today's agenda", |s| s.print_agenda(true));
                }
                if action(ui, "morning briefing", CYAN).clicked() {
                    self.job("Printed your morning briefing", |s| s.print_briefing());
                }
                if action(ui, "end-of-day receipt", PINK).clicked() {
                    self.job("Printed your end-of-day receipt", |s| s.print_end_of_day());
                }
                if action(ui, "to-do list", GREEN).clicked() {
                    let todos = self.data.todos.clone();
                    self.job("Printed your to-do list", move |s| s.print(escpos::todo_list(&s.cfg(), &todos)));
                }
                if action(ui, "test slip", DIM).clicked() {
                    self.job("Test slip printed", |s| s.print(escpos::test_slip(&s.cfg())));
                }
            });
        });

        // fake terminal log
        section(ui, "$", "tail -f activity.log", |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(7, 10, 15))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 20.0);
                    let lines = config::log_tail(8);
                    if lines.is_empty() {
                        ui.label(mono("(nothing yet. print something!)", DIM));
                    }
                    for l in lines {
                        ui.label(RichText::new(l).monospace().size(12.5).color(GREEN));
                    }
                });
        });
    }

    /// (phase, seconds left) while the focus timer runs.
    fn countdown(&self) -> Option<(String, i64)> {
        let p = &self.data.pomodoro;
        if !p.running {
            return None;
        }
        let ends = chrono::DateTime::parse_from_rfc3339(&p.ends_at).ok()?;
        Some((p.phase.clone(), (ends.with_timezone(&Local) - Local::now()).num_seconds()))
    }

    // -------------------------------------------------------------- Print

    fn page_print(&mut self, ui: &mut Ui) {
        page_title(ui, "~/print", "// text, pictures, QR codes and stickers");

        section(ui, "01", "text", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.txt)
                    .hint_text("type anything, or press ctrl+alt+p in any app to print what you selected")
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                toggle(ui, &mut self.t_bold, "bold");
                toggle(ui, &mut self.t_big, "big");
                toggle(ui, &mut self.t_center, "centered");
                toggle(ui, &mut self.t_stamp, "date and time");
                toggle(ui, &mut self.t_check, "checklist");
            });
            ui.horizontal(|ui| {
                if action(ui, "print text", AMBER).clicked() && !self.txt.trim().is_empty() {
                    let text = self.txt.clone();
                    let o = TextOpts {
                        bold: self.t_bold,
                        big: self.t_big,
                        center: self.t_center,
                        stamp: self.t_stamp,
                        checklist: self.t_check,
                    };
                    self.job("Printed", move |s| s.print(escpos::text_slip(&s.cfg(), &text, &o)));
                }
                if ghost(ui, "clear").clicked() {
                    self.txt.clear();
                }
            });
        });

        section(ui, "02", "picture", |ui| {
            ui.label(mono("drop a png / jpg / gif / webp anywhere on this window, or paste a path", DIM));
            ui.horizontal(|ui| {
                text_field(ui, &mut self.img_path, r"C:\Pictures\cat.png", 420.0);
            });
            ui.horizontal(|ui| {
                ui.label("brightness");
                ui.add(egui::Slider::new(&mut self.img_bright, -0.5..=0.5));
                toggle(ui, &mut self.img_smooth, "dither (photos)");
            });
            text_field(ui, &mut self.img_caption, "caption (optional)", 420.0);

            // preview of what the printer will draw
            let key = (self.img_path.clone(), (self.img_bright * 100.0) as i32, self.img_smooth);
            if key != self.img_key {
                self.img_key = key;
                self.img_preview = None;
                if !self.img_path.trim().is_empty() {
                    if let Ok(bm) = raster::from_file(self.img_path.trim(), 240, self.img_bright, self.img_smooth) {
                        let pixels: Vec<Color32> = bm
                            .dots
                            .iter()
                            .map(|d| if *d { Color32::BLACK } else { Color32::WHITE })
                            .collect();
                        let img = egui::ColorImage { size: [bm.w, bm.h], source_size: egui::vec2(bm.w as f32, bm.h as f32), pixels };
                        self.img_preview = Some(ui.ctx().load_texture("preview", img, egui::TextureOptions::NEAREST));
                    }
                }
            }
            if let Some(t) = &self.img_preview {
                ui.add(egui::Image::from_texture(t).max_size(egui::vec2(240.0, 260.0)));
            }
            if action(ui, "print picture", AMBER).clicked() && !self.img_path.trim().is_empty() {
                let (path, bright, smooth, caption, width) =
                    (self.img_path.trim().to_string(), self.img_bright, self.img_smooth, self.img_caption.clone(), self.paper_dots());
                self.job("Printed the picture", move |s| {
                    let bm = raster::from_file(&path, width, bright, smooth)?;
                    s.print(escpos::image_slip(&s.cfg(), &bm, &caption))
                });
            }
        });

        section(ui, "03", "qr codes", |ui| {
            ui.horizontal(|ui| {
                text_field(ui, &mut self.qr_text, "a link or any text", 360.0);
                if action(ui, "print qr", AMBER).clicked() && !self.qr_text.trim().is_empty() {
                    let t = self.qr_text.trim().to_string();
                    self.job("Printed the QR code", move |s| {
                        s.print(escpos::qr_slip(&s.cfg(), "SCAN ME", &t, &[t.clone()])?)
                    });
                }
            });
            ui.separator();
            ui.label(mono("wi-fi slip for guests", DIM));
            ui.horizontal(|ui| {
                text_field(ui, &mut self.wifi_ssid, "network name", 200.0);
                ui.add_enabled(!self.wifi_open, egui::TextEdit::singleline(&mut self.wifi_pass).hint_text("password").desired_width(200.0));
                toggle(ui, &mut self.wifi_open, "open network");
                if action(ui, "print wi-fi", CYAN).clicked() && !self.wifi_ssid.trim().is_empty() {
                    let (ssid, pass, open) = (self.wifi_ssid.trim().to_string(), self.wifi_pass.clone(), self.wifi_open);
                    self.job("Printed the Wi-Fi slip", move |s| {
                        let code = if open {
                            format!("WIFI:T:nopass;S:{};;", wifi_escape(&ssid))
                        } else {
                            format!("WIFI:T:WPA;S:{};P:{};;", wifi_escape(&ssid), wifi_escape(&pass))
                        };
                        let mut caption = vec![format!("Network: {ssid}")];
                        if !open {
                            caption.push(format!("Password: {pass}"));
                        }
                        s.print(escpos::qr_slip(&s.cfg(), "WI-FI", &code, &caption)?)
                    });
                }
            });
        });

        section(ui, "04", "pip stickers", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, (name, mood)) in STICKERS.iter().enumerate() {
                    let selected = self.sticker == i;
                    ui.vertical(|ui| {
                        ui.add(egui::Image::from_texture(self.tex.get(*mood)).fit_to_exact_size(egui::vec2(84.0, 84.0)));
                        if ui.selectable_label(selected, mono(name, if selected { AMBER } else { DIM })).clicked() {
                            self.sticker = i;
                        }
                    });
                }
            });
            if action(ui, "print sticker", PINK).clicked() {
                let mood = STICKERS[self.sticker].1;
                self.job("Sticker printed", move |s| s.print(escpos::sticker_slip(&s.cfg(), mood)));
            }
        });
    }

    // ----------------------------------------------------------- Schedule

    fn page_schedule(&mut self, ui: &mut Ui) {
        page_title(ui, "~/schedule", "// calendar, briefings, reminders and focus");

        section(ui, "01", "calendar", |ui| {
            if !self.sh.connected() {
                ui.label("Not connected. Link your Google Calendar in settings to see events here and on your slips.");
                if action(ui, "go to settings", AMBER).clicked() {
                    self.page = Page::Settings;
                }
                return;
            }
            let events: Vec<_> = self.sh.events.lock().unwrap().iter().filter(|e| e.start >= Local::now() - chrono::Duration::hours(1)).take(7).cloned().collect();
            if events.is_empty() {
                ui.label(mono("nothing coming up. suspicious.", DIM));
            }
            for e in &events {
                ui.horizontal(|ui| {
                    ui.label(mono(&e.start.format("%a %d %b %H:%M").to_string(), AMBER));
                    ui.label(&e.summary);
                });
            }
            if ghost(ui, "refresh").clicked() {
                self.job("Calendar updated", |s| s.refresh_events());
            }
        });

        section(ui, "02", "agenda and stubs", |ui| {
            toggle(ui, &mut self.cfg.auto_agenda, "print the agenda every day at");
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                let (mut h, mut m) = (self.cfg.agenda_hour, self.cfg.agenda_minute);
                time_edit(ui, &mut h, &mut m);
                self.cfg.agenda_hour = h;
                self.cfg.agenda_minute = m;
                ui.label("covering");
                ui.add(egui::DragValue::new(&mut self.cfg.agenda_days).range(1..=14));
                ui.label("day(s)");
            });
            toggle(ui, &mut self.cfg.show_locations, "show event locations");
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.event_stubs, "print a ticket stub");
                ui.add(egui::DragValue::new(&mut self.cfg.stub_minutes).range(1..=240));
                ui.label("minutes before each event");
            });
        });

        section(ui, "03", "morning briefing", |ui| {
            ui.label(mono("weather + agenda + to-dos + a line from pip, all on one slip", DIM));
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.briefing, "print every day at");
                let (mut h, mut m) = (self.cfg.briefing_hour, self.cfg.briefing_minute);
                time_edit(ui, &mut h, &mut m);
                self.cfg.briefing_hour = h;
                self.cfg.briefing_minute = m;
            });
            ui.horizontal(|ui| {
                ui.label("city for the weather");
                let before = self.cfg.city.clone();
                text_field(ui, &mut self.cfg.city, "Athens", 200.0);
                if self.cfg.city != before {
                    self.cfg.lat = 0.0;
                    self.cfg.lon = 0.0;
                }
            });
            if action(ui, "print briefing now", CYAN).clicked() {
                self.job("Printed your morning briefing", |s| s.print_briefing());
            }
        });

        section(ui, "04", "reminders and timers", |ui| {
            ui.horizontal_wrapped(|ui| {
                text_field(ui, &mut self.new_reminder, "remind me to...", 260.0);
                ui.label("in");
                ui.add(egui::DragValue::new(&mut self.reminder_min).range(1..=1440));
                ui.label("min");
                if action(ui, "add", AMBER).clicked() && !self.new_reminder.trim().is_empty() {
                    let (text, mins) = (self.new_reminder.trim().to_string(), self.reminder_min);
                    self.add_reminder(text, mins);
                    self.new_reminder.clear();
                }
            });
            ui.horizontal(|ui| {
                ui.label(mono("quick timers:", DIM));
                for mins in [5u32, 10, 15, 30, 60] {
                    if ghost(ui, &format!("{mins}m")).clicked() {
                        self.add_reminder("Timer finished".into(), mins);
                    }
                }
            });
            let upcoming: Vec<_> = self.data.reminders.iter().filter(|r| !r.fired).cloned().collect();
            for r in upcoming {
                ui.horizontal(|ui| {
                    let when = chrono::DateTime::parse_from_rfc3339(&r.due)
                        .map(|d| d.with_timezone(&Local).format("%H:%M").to_string())
                        .unwrap_or_default();
                    ui.label(mono(&when, AMBER));
                    ui.label(&r.text);
                    if ghost(ui, "x").clicked() {
                        let id = r.id;
                        self.change_data(|d| d.reminders.retain(|x| x.id != id));
                    }
                });
            }
        });

        section(ui, "05", "focus timer (pomodoro)", |ui| {
            let sessions = self.data.pomo_sessions.get(&Local::now().format("%Y-%m-%d").to_string()).copied().unwrap_or(0);
            match self.countdown() {
                Some((phase, secs)) => {
                    ui.label(RichText::new(fmt_countdown(secs)).monospace().size(40.0).color(if phase == "work" { AMBER } else { GREEN }));
                    ui.label(mono(&format!("{phase}  |  {}", self.data.pomodoro.task), DIM));
                    if action(ui, "stop", RED).clicked() {
                        self.change_data(|d| d.pomodoro.running = false);
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(400));
                }
                None => {
                    text_field(ui, &mut self.pomo_task, "what are you working on?", 320.0);
                    ui.horizontal(|ui| {
                        ui.label("work");
                        ui.add(egui::DragValue::new(&mut self.cfg.pomo_work).range(1..=180));
                        ui.label("min   break");
                        ui.add(egui::DragValue::new(&mut self.cfg.pomo_break).range(1..=60));
                        ui.label("min");
                    });
                    if action(ui, "start focus", AMBER).clicked() {
                        let task = self.pomo_task.trim().to_string();
                        let mins = self.cfg.pomo_work;
                        self.change_data(|d| {
                            d.pomodoro.running = true;
                            d.pomodoro.phase = "work".into();
                            d.pomodoro.task = task;
                            d.pomodoro.ends_at = (Local::now() + chrono::Duration::minutes(mins as i64)).to_rfc3339();
                        });
                    }
                }
            }
            ui.label(mono(&format!("sessions today: {sessions}  (a stamp card prints after each one)"), DIM));
        });

        section(ui, "06", "end-of-day receipt", |ui| {
            ui.label(mono("a funny receipt of your day: meetings, focus, tasks, time at the pc", DIM));
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.eod, "print every day at");
                let (mut h, mut m) = (self.cfg.eod_hour, self.cfg.eod_minute);
                time_edit(ui, &mut h, &mut m);
                self.cfg.eod_hour = h;
                self.cfg.eod_minute = m;
            });
            toggle(ui, &mut self.cfg.track_usage, "include time at the pc and top programs (stays on this computer, off by default)");
            if action(ui, "print receipt now", PINK).clicked() {
                self.job("Printed your end-of-day receipt", |s| s.print_end_of_day());
            }
        });
    }

    fn add_reminder(&mut self, text: String, minutes: u32) {
        let due = (Local::now() + chrono::Duration::minutes(minutes as i64)).to_rfc3339();
        self.change_data(|d| {
            let id = d.new_id();
            d.reminders.push(data::Reminder { id, text, due, fired: false });
        });
        self.say(format!("Reminder set for {minutes} minutes from now"));
    }

    // -------------------------------------------------------------- Tasks

    fn page_tasks(&mut self, ui: &mut Ui) {
        page_title(ui, "~/tasks", "// to-dos and habits");

        section(ui, "01", "to-do", |ui| {
            ui.horizontal(|ui| {
                let resp = text_field(ui, &mut self.new_todo, "add a task and press enter", 380.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (action(ui, "add", AMBER).clicked() || enter) && !self.new_todo.trim().is_empty() {
                    let text = self.new_todo.trim().to_string();
                    self.change_data(|d| {
                        let id = d.new_id();
                        d.todos.push(data::Todo { id, text, done: false });
                    });
                    self.new_todo.clear();
                }
            });
            let todos = self.data.todos.clone();
            if todos.is_empty() {
                ui.label(mono("empty. enjoy it while it lasts.", DIM));
            }
            for t in todos {
                ui.horizontal(|ui| {
                    let mut done = t.done;
                    if ui.checkbox(&mut done, "").changed() {
                        let id = t.id;
                        self.change_data(|d| {
                            if let Some(x) = d.todos.iter_mut().find(|x| x.id == id) {
                                x.done = done;
                            }
                        });
                    }
                    let text = if t.done { RichText::new(&t.text).strikethrough().color(DIM) } else { RichText::new(&t.text) };
                    ui.label(text);
                    if ghost(ui, "x").clicked() {
                        let id = t.id;
                        self.change_data(|d| d.todos.retain(|x| x.id != id));
                    }
                });
            }
            ui.horizontal(|ui| {
                if action(ui, "print list", AMBER).clicked() {
                    let todos = self.data.todos.clone();
                    self.job("Printed your to-do list", move |s| s.print(escpos::todo_list(&s.cfg(), &todos)));
                }
                if ghost(ui, "clear finished").clicked() {
                    self.change_data(|d| d.todos.retain(|t| !t.done));
                }
            });
        });

        section(ui, "02", "habits", |ui| {
            let today = Local::now().date_naive();
            let key = today.format("%Y-%m-%d").to_string();
            ui.horizontal(|ui| {
                let resp = text_field(ui, &mut self.new_habit, "new habit (read, walk, water...)", 320.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (action(ui, "add habit", GREEN).clicked() || enter) && !self.new_habit.trim().is_empty() {
                    let name = self.new_habit.trim().to_string();
                    self.change_data(|d| {
                        let id = d.new_id();
                        d.habits.push(data::Habit { id, name, days: vec![] });
                    });
                    self.new_habit.clear();
                }
            });
            let habits = self.data.habits.clone();
            if habits.is_empty() {
                ui.label(mono("no habits yet. start with something tiny.", DIM));
            }
            for h in habits {
                let streak = data::streak(&h, today);
                let ticked = h.days.contains(&key);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&h.name).strong());
                    ui.label(mono(&format!("streak {streak}"), if streak >= 3 { AMBER } else { DIM }));
                    if ui.selectable_label(ticked, mono(if ticked { "[X] today" } else { "[ ] today" }, if ticked { GREEN } else { DIM })).clicked() {
                        let (id, key) = (h.id, key.clone());
                        self.change_data(|d| {
                            if let Some(x) = d.habits.iter_mut().find(|x| x.id == id) {
                                if let Some(p) = x.days.iter().position(|v| *v == key) {
                                    x.days.remove(p);
                                } else {
                                    x.days.push(key);
                                }
                            }
                        });
                    }
                    if ghost(ui, "print card").clicked() {
                        let hh = h.clone();
                        self.job("Printed the habit card", move |s| {
                            let streak = data::streak(&hh, Local::now().date_naive());
                            s.print(escpos::habit_card(&s.cfg(), &hh, Local::now().date_naive(), streak))
                        });
                    }
                    if ghost(ui, "x").clicked() {
                        let id = h.id;
                        self.change_data(|d| d.habits.retain(|x| x.id != id));
                    }
                });
            }
        });
    }

    // ------------------------------------------------------------- Alerts

    fn page_alerts(&mut self, ui: &mut Ui) {
        page_title(ui, "~/alerts", "// pip taps you on the shoulder, on paper");

        section(ui, "01", "downloads", |ui| {
            toggle(ui, &mut self.cfg.alert_downloads, "print a slip when a download finishes");
            ui.horizontal(|ui| {
                ui.label("folder");
                text_field(ui, &mut self.cfg.download_dir, "empty = your Downloads folder", 380.0);
            });
        });

        section(ui, "02", "battery and disk", |ui| {
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.alert_battery, "battery drops to");
                ui.add(egui::DragValue::new(&mut self.cfg.battery_pct).range(5..=90).suffix("%"));
            });
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.alert_disk, "drive C: has less than");
                ui.add(egui::DragValue::new(&mut self.cfg.disk_free_pct).range(1..=50).suffix("% free"));
            });
        });

        section(ui, "03", "programs that finish", |ui| {
            ui.label(mono("name a program (cargo, ffmpeg, blender). pip prints when it stops running.", DIM));
            ui.horizontal(|ui| {
                let resp = text_field(ui, &mut self.new_proc, "program name", 220.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (action(ui, "watch", AMBER).clicked() || enter) && !self.new_proc.trim().is_empty() {
                    self.cfg.watch_procs.push(self.new_proc.trim().to_string());
                    self.new_proc.clear();
                }
            });
            let mut remove = None;
            for (i, p) in self.cfg.watch_procs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(mono(p, CYAN));
                    if ghost(ui, "x").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.cfg.watch_procs.remove(i);
            }
        });

        section(ui, "04", "pages that change", |ui| {
            ui.label(mono("pip checks every 10 minutes and prints when the page is different", DIM));
            ui.horizontal(|ui| {
                let resp = text_field(ui, &mut self.new_url, "https://...", 380.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (action(ui, "watch", AMBER).clicked() || enter) && self.new_url.trim().starts_with("http") {
                    self.cfg.watch_urls.push(self.new_url.trim().to_string());
                    self.new_url.clear();
                }
            });
            let mut remove = None;
            for (i, u) in self.cfg.watch_urls.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(mono(u, CYAN));
                    if ghost(ui, "x").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.cfg.watch_urls.remove(i);
            }
        });

        section(ui, "05", "mail slips", |ui| {
            ui.label(mono("prints sender and subject for new mail with a label you choose. read-only.", DIM));
            ui.horizontal(|ui| {
                toggle(ui, &mut self.cfg.mail, "print new mail labelled");
                text_field(ui, &mut self.cfg.mail_label, "IMPORTANT", 140.0);
            });
            ui.label(mono("labels: IMPORTANT, STARRED, INBOX or a label id", DIM));
            if self.cfg.mail && !self.cfg.google_has_mail {
                ui.colored_label(AMBER, "Gmail access isn't granted yet: in settings tick \"also allow Gmail\" and connect Google again.");
                if ghost(ui, "go to settings").clicked() {
                    self.page = Page::Settings;
                }
            }
        });
    }

    // ----------------------------------------------------------- Settings

    fn page_settings(&mut self, ui: &mut Ui) {
        page_title(ui, "~/settings", "// printer, pip and google");

        section(ui, "01", "printer", |ui| {
            let printers = self.printers.clone();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("printer")
                    .width(320.0)
                    .selected_text(self.cfg.printer.clone())
                    .show_ui(ui, |ui| {
                        for p in &printers {
                            ui.selectable_value(&mut self.cfg.printer, p.clone(), p);
                        }
                    });
                if ghost(ui, "refresh").clicked() {
                    self.printers = printer::list_printers();
                }
                pill(ui, if self.printer_ok() { "found" } else { "not found" }, if self.printer_ok() { GREEN } else { RED });
            });
            ui.horizontal(|ui| {
                ui.label("characters per line");
                ui.add(egui::DragValue::new(&mut self.cfg.paper_chars).range(16..=64));
                ui.label(mono("(48 = 80 mm paper, 32 = 58 mm; check the ruler on the test slip)", DIM));
            });
            toggle(ui, &mut self.cfg.greeklish, "print Greek letters as Latin (works on every printer)");
            ui.horizontal(|ui| {
                ui.label("Greek code page number");
                let mut cp = self.cfg.codepage.map(|n| n.to_string()).unwrap_or_default();
                if text_field(ui, &mut cp, "blank", 50.0).changed() {
                    self.cfg.codepage = cp.trim().parse::<u8>().ok();
                }
                ui.label(mono("advanced", DIM));
            });
            toggle(ui, &mut self.cfg.pip_stickers, "add a tiny Pip sticker to the bottom of every slip");
            if action(ui, "test print", AMBER).clicked() {
                let cfg = self.cfg.clone();
                self.job("Test slip printed", move |_| printer::print_raw(&cfg.printer, &escpos::test_slip(&cfg)));
            }
        });

        section(ui, "02", "system", |ui| {
            toggle(ui, &mut self.cfg.start_with_windows, "start Thermo Companion with Windows");
            toggle(ui, &mut self.cfg.hotkey, "Ctrl+Alt+P prints the selected text from any app");
        });

        section(ui, "03", "google", |ui| {
            let linked = self.sh.connected();
            ui.horizontal(|ui| {
                pill(ui, if linked { "connected" } else { "not connected" }, if linked { GREEN } else { DIM });
                if linked && self.cfg.google_has_mail {
                    pill(ui, "gmail ok", CYAN);
                }
            });
            ui.label(mono("create your own free OAuth client once (steps are in the README)", DIM));
            ui.horizontal(|ui| {
                ui.label("client id    ");
                text_field(ui, &mut self.cfg.google_client_id, "xxxx.apps.googleusercontent.com", 380.0);
            });
            ui.horizontal(|ui| {
                ui.label("client secret");
                ui.add(egui::TextEdit::singleline(&mut self.cfg.google_client_secret).password(true).desired_width(380.0));
            });
            toggle(ui, &mut self.connect_mail, "also allow reading Gmail (for mail slips)");
            ui.horizontal(|ui| {
                let can = !self.cfg.google_client_id.trim().is_empty() && !self.cfg.google_client_secret.trim().is_empty() && !self.google_busy;
                if ui.add_enabled(can, egui::Button::new(mono(if self.google_busy { "waiting for browser..." } else { "connect google" }, INK)).fill(AMBER)).clicked() {
                    self.connect_google(ui.ctx().clone());
                }
                if linked && ghost(ui, "disconnect").clicked() {
                    self.cfg.google_refresh_token.clear();
                    self.cfg.google_has_mail = false;
                    self.sh.events.lock().unwrap().clear();
                    *self.sh.last_fetch.lock().unwrap() = None;
                }
            });
        });

        section(ui, "04", "about", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::from_texture(self.tex.get(Mood::Happy)).fit_to_exact_size(egui::vec2(72.0, 72.0)));
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("Thermo Companion v{}", env!("CARGO_PKG_VERSION"))).strong());
                    ui.label(mono("pip says hi. made with rust. mit license.", DIM));
                    ui.label(mono(&format!("settings: {}", config::dir().display()), DIM));
                });
            });
        });
    }

    fn connect_google(&mut self, ctx: egui::Context) {
        let id = self.cfg.google_client_id.trim().to_string();
        let secret = self.cfg.google_client_secret.trim().to_string();
        let with_mail = self.connect_mail;
        let slot = self.google.clone();
        self.google_busy = true;
        self.say("Opening your browser for Google sign-in...");
        std::thread::spawn(move || {
            let r = gcal::connect(&id, &secret, with_mail);
            *slot.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for Dash {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(&ctx);
        let before = self.cfg.clone();

        egui::Panel::left("nav")
            .exact_size(210.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.nav(ui));

        egui::Panel::bottom("status")
            .exact_size(30.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(14, 4)))
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(22)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    match self.page {
                        Page::Home => self.page_home(ui),
                        Page::Print => self.page_print(ui),
                        Page::Schedule => self.page_schedule(ui),
                        Page::Tasks => self.page_tasks(ui),
                        Page::Alerts => self.page_alerts(ui),
                        Page::Settings => self.page_settings(ui),
                    }
                    ui.add_space(24.0);
                });
            });

        // settings edited on screen: save them, and apply "start with Windows" right away
        if self.cfg != before {
            if self.cfg.start_with_windows != before.start_with_windows {
                if let Err(e) = autostart::set(self.cfg.start_with_windows) {
                    self.say(format!("Couldn't change start-with-Windows: {e}"));
                }
            }
            self.sh.set_cfg(self.cfg.clone());
        }

        // keep the clock, cursor and countdown moving
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

pub fn run(page: Option<String>) {
    let (cfg, _) = config::load();
    let shared = Shared::new(cfg);

    let icon = image::load_from_memory(pip::LOGO).expect("logo").to_rgba8();
    let (w, h) = (icon.width(), icon.height());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 720.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title(WINDOW_TITLE)
            .with_drag_and_drop(true)
            .with_icon(egui::IconData { rgba: icon.into_raw(), width: w, height: h }),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(Dash::new(cc, shared, page))))) {
        config::log(&format!("dashboard failed to start: {e}"));
    }
}
