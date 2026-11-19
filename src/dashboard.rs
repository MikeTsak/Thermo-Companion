//! Pip's dashboard: the window you get when you left-click the tray icon.
//! It runs as its own process (`thermo-companion.exe --dashboard`) so the tray stays tiny.

use crate::app::{self, Notify, Shared};
use crate::config::{self, Config};
use crate::data::{self, Data, Note, NoteItem};
use crate::escpos::{self, TextOpts};
use crate::pip::{self, Mood};
use crate::{autostart, gcal, ical, printer, raster, sysinfo, update};
use chrono::{Datelike, Local, TimeZone, Timelike};
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

/// Keep-style note colours (dark tints).
const NOTE_COLORS: [(&str, Color32); 8] = [
    ("plain", CARD),
    ("red", Color32::from_rgb(84, 40, 46)),
    ("orange", Color32::from_rgb(86, 56, 26)),
    ("yellow", Color32::from_rgb(80, 72, 26)),
    ("green", Color32::from_rgb(30, 72, 48)),
    ("teal", Color32::from_rgb(26, 68, 74)),
    ("blue", Color32::from_rgb(34, 56, 96)),
    ("purple", Color32::from_rgb(64, 44, 96)),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Notes,
    Print,
    Schedule,
    Tasks,
    Alerts,
    Settings,
}

const PRINT_TABS: [&str; 7] = ["text", "picture", "qr + wifi", "tickets", "labels", "stickers", "fun"];
const SCHED_TABS: [&str; 7] = ["calendar", "agenda", "briefing", "reminders", "focus", "end of day", "planner"];
const TASK_TABS: [&str; 3] = ["to-do", "habits", "countdowns"];
const ALERT_TABS: [&str; 5] = ["downloads", "battery + disk", "programs", "web pages", "mail"];
const SET_TABS: [&str; 4] = ["printer", "system", "calendar", "about"];

impl Page {
    const ALL: [Page; 7] = [
        Page::Home,
        Page::Notes,
        Page::Print,
        Page::Schedule,
        Page::Tasks,
        Page::Alerts,
        Page::Settings,
    ];

    fn idx(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    fn name(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Notes => "notes",
            Page::Print => "print",
            Page::Schedule => "schedule",
            Page::Tasks => "tasks",
            Page::Alerts => "alerts",
            Page::Settings => "settings",
        }
    }

    fn tabs(self) -> &'static [&'static str] {
        match self {
            Page::Print => &PRINT_TABS,
            Page::Schedule => &SCHED_TABS,
            Page::Tasks => &TASK_TABS,
            Page::Alerts => &ALERT_TABS,
            Page::Settings => &SET_TABS,
            _ => &[],
        }
    }

    fn from_arg(s: &str) -> Page {
        Page::ALL.iter().copied().find(|p| p.name() == s).unwrap_or(Page::Home)
    }
}

/// Things the command palette (Ctrl+K) can do.
#[derive(Clone, Copy)]
enum Cmd {
    Go(Page, usize),
    PrintAgenda,
    Briefing,
    EndOfDay,
    Todos,
    TestSlip,
    ToggleFocus,
    NewNote,
    Fortune,
    Joke,
    Coin,
    Planner,
    Countdowns,
    ConnectCalendar,
    CheckUpdates,
}

fn all_commands() -> Vec<(String, Cmd)> {
    let mut v: Vec<(String, Cmd)> = Vec::new();
    v.push(("new note".into(), Cmd::NewNote));
    v.push(("print today's agenda".into(), Cmd::PrintAgenda));
    v.push(("print morning briefing".into(), Cmd::Briefing));
    v.push(("print end-of-day receipt".into(), Cmd::EndOfDay));
    v.push(("print to-do list".into(), Cmd::Todos));
    v.push(("print week planner".into(), Cmd::Planner));
    v.push(("print countdowns".into(), Cmd::Countdowns));
    v.push(("print test slip".into(), Cmd::TestSlip));
    v.push(("start / stop focus timer".into(), Cmd::ToggleFocus));
    v.push(("print a fortune cookie".into(), Cmd::Fortune));
    v.push(("print a joke".into(), Cmd::Joke));
    v.push(("flip a coin".into(), Cmd::Coin));
    v.push(("connect calendar (google link)".into(), Cmd::ConnectCalendar));
    v.push(("check for updates".into(), Cmd::CheckUpdates));
    for p in Page::ALL {
        v.push((format!("go to {}", p.name()), Cmd::Go(p, 0)));
        for (i, t) in p.tabs().iter().enumerate() {
            v.push((format!("go to {} / {}", p.name(), t), Cmd::Go(p, i)));
        }
    }
    v
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

fn lighten(c: Color32) -> Color32 {
    Color32::from_rgb(c.r().saturating_add(34), c.g().saturating_add(34), c.b().saturating_add(34))
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
    ui.add_space(4.0);
}

fn tab_bar(ui: &mut Ui, current: &mut usize, labels: &[&str]) {
    ui.horizontal_wrapped(|ui| {
        for (i, l) in labels.iter().enumerate() {
            let active = *current == i;
            let resp = ui.add(
                egui::Button::new(RichText::new(*l).monospace().color(if active { AMBER } else { DIM }))
                    .fill(if active { CARD2 } else { Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, if active { AMBER } else { BORDER }))
                    .corner_radius(CornerRadius::same(20)),
            );
            if resp.clicked() {
                *current = i;
            }
        }
    });
    ui.add_space(4.0);
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

fn small_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).monospace().size(12.5)).small())
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

fn swatches(ui: &mut Ui, sel: &mut u8) {
    ui.horizontal(|ui| {
        for (i, (name, c)) in NOTE_COLORS.iter().enumerate() {
            let selected = *sel as usize == i;
            let resp = ui.add(
                egui::Button::new("")
                    .fill(*c)
                    .stroke(Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { AMBER } else { lighten(*c) }))
                    .corner_radius(CornerRadius::same(12))
                    .min_size(egui::vec2(24.0, 24.0)),
            );
            if resp.on_hover_text(*name).clicked() {
                *sel = i as u8;
            }
        }
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

fn open_url(url: &str) {
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
}

fn egui_key_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let vk = match key {
        A => b'A' as u32, B => b'B' as u32, C => b'C' as u32, D => b'D' as u32, E => b'E' as u32,
        F => b'F' as u32, G => b'G' as u32, H => b'H' as u32, I => b'I' as u32, J => b'J' as u32,
        K => b'K' as u32, L => b'L' as u32, M => b'M' as u32, N => b'N' as u32, O => b'O' as u32,
        P => b'P' as u32, Q => b'Q' as u32, R => b'R' as u32, S => b'S' as u32, T => b'T' as u32,
        U => b'U' as u32, V => b'V' as u32, W => b'W' as u32, X => b'X' as u32, Y => b'Y' as u32,
        Z => b'Z' as u32,
        Num0 => b'0' as u32, Num1 => b'1' as u32, Num2 => b'2' as u32, Num3 => b'3' as u32, Num4 => b'4' as u32,
        Num5 => b'5' as u32, Num6 => b'6' as u32, Num7 => b'7' as u32, Num8 => b'8' as u32, Num9 => b'9' as u32,
        F1 => 0x70, F2 => 0x71, F3 => 0x72, F4 => 0x73, F5 => 0x74, F6 => 0x75,
        F7 => 0x76, F8 => 0x77, F9 => 0x78, F10 => 0x79, F11 => 0x7A, F12 => 0x7B,
        F13 => 0x7C, F14 => 0x7D, F15 => 0x7E, F16 => 0x7F, F17 => 0x80, F18 => 0x81,
        F19 => 0x82, F20 => 0x83, F21 => 0x84, F22 => 0x85, F23 => 0x86, F24 => 0x87,
        Space => 0x20, Tab => 0x09, Enter => 0x0D, Backspace => 0x08,
        Insert => 0x2D, Delete => 0x2E, Home => 0x24, End => 0x23, PageUp => 0x21, PageDown => 0x22,
        ArrowLeft => 0x25, ArrowUp => 0x26, ArrowRight => 0x27, ArrowDown => 0x28,
        Minus => 0xBD, Equals | Plus => 0xBB, OpenBracket => 0xDB, CloseBracket => 0xDD,
        Backslash => 0xDC, Semicolon => 0xBA, Quote => 0xDE, Comma => 0xBC,
        Period => 0xBE, Slash => 0xBF, Backtick => 0xC0,
        _ => return None,
    };
    Some(vk)
}

/// client_id and client_secret from Google's downloaded JSON (or a path to that file).
fn parse_client(text: &str) -> Option<(String, String)> {
    let text = text.trim().trim_matches('"');
    let json = if text.starts_with('{') { text.to_string() } else { std::fs::read_to_string(text).ok()? };
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let c = if v.get("installed").is_some() { &v["installed"] } else { &v["web"] };
    Some((c["client_id"].as_str()?.to_string(), c["client_secret"].as_str()?.to_string()))
}

// -------------------------------------------------------------------- app

struct Dash {
    sh: Arc<Shared>,
    notify: Notify,
    page: Page,
    tabs: [usize; 7],
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

    // command palette
    pal_open: bool,
    pal_query: String,
    pal_sel: usize,
    pal_focus: bool,

    // connect calendar popup
    connect_open: bool,
    connect_tab: usize,
    ical_input: String,
    ical_busy: bool,
    ical_note: String,
    ical_slot: Arc<Mutex<Option<Result<(String, usize), String>>>>,
    google: Arc<Mutex<Option<Result<String, String>>>>,
    google_busy: bool,
    connect_mail: bool,
    client_note: String,

    // notes
    n_title: String,
    n_body: String,
    n_check: bool,
    n_color: u8,
    n_labels: String,
    n_focus: bool,
    n_search: String,
    n_filter: Option<String>,
    n_archived: bool,
    editing: Option<u64>,
    edit_title: String,
    edit_body: String,

    // print
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
    tk_kind: usize,
    tk: [String; 6],
    lb_text: String,
    lb_scale: u8,
    lb_copies: u8,
    fun_kind: u8,
    fun_title: String,
    fun_text: String,
    fun_question: String,
    fun_list: String,

    // schedule / tasks / alerts
    new_reminder: String,
    rem_at: bool,
    rem_min: u32,
    rem_h: u32,
    rem_m: u32,
    rem_repeat: usize,
    pomo_task: String,
    new_todo: String,
    new_habit: String,
    cd_name: String,
    cd_date: [i32; 3],
    new_url: String,
    new_proc: String,

    // updates
    upd_busy: bool,
    upd_installing: bool,
    upd_state: Option<Result<update::CheckOutcome, String>>,
    upd_err: String,
    upd_check_slot: Arc<Mutex<Option<Result<update::CheckOutcome, String>>>>,
    upd_apply_slot: Arc<Mutex<Option<Result<(), String>>>>,

    // hotkey recorder
    hotkey_listen: bool,
    hotkey_note: String,
    context_menu: bool,
}

const STICKERS: [(&str, Mood); 5] = [
    ("happy", Mood::Happy),
    ("party", Mood::Party),
    ("sleepy", Mood::Sleepy),
    ("busy", Mood::Busy),
    ("sad", Mood::Sad),
];

const REPEATS: [(&str, &str); 4] = [("once", ""), ("daily", "daily"), ("weekdays", "weekdays"), ("weekly", "weekly")];

impl Dash {
    fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>, arg: Option<String>) -> Dash {
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
        if cfg.calendar_linked() {
            app::spawn_job(&shared, notify.clone(), "Calendar updated", |s| s.refresh_events());
        }

        let upd_check_slot: Arc<Mutex<Option<Result<update::CheckOutcome, String>>>> = Arc::new(Mutex::new(None));
        let mut upd_busy = false;
        if cfg.auto_check_updates {
            upd_busy = true;
            let slot = upd_check_slot.clone();
            let n = notify.clone();
            std::thread::spawn(move || {
                *slot.lock().unwrap() = Some(update::check_latest());
                n();
            });
        }

        // launch options: "notes-new", "connect", "palette", or "page" / "page/tab-number"
        let arg = arg.unwrap_or_default();
        let (page_name, tab_no) = match arg.split_once('/') {
            Some((p, t)) => (p.to_string(), t.parse::<usize>().unwrap_or(0)),
            None => (arg.clone(), 0),
        };
        let (page, n_focus) = match page_name.as_str() {
            "notes-new" => (Page::Notes, true),
            other => (Page::from_arg(other), false),
        };
        let mut tabs = [0usize; 7];
        tabs[page.idx()] = tab_no.min(page.tabs().len().saturating_sub(1));
        let today = Local::now().date_naive() + chrono::Duration::days(30);
        let now = Local::now() + chrono::Duration::minutes(10);

        Dash {
            sh: shared,
            notify,
            page,
            tabs,
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
            pal_open: arg == "palette",
            pal_query: String::new(),
            pal_sel: 0,
            pal_focus: arg == "palette",
            connect_open: arg == "connect",
            connect_tab: 0,
            ical_input: String::new(),
            ical_busy: false,
            ical_note: String::new(),
            ical_slot: Arc::new(Mutex::new(None)),
            google: Arc::new(Mutex::new(None)),
            google_busy: false,
            connect_mail: false,
            client_note: String::new(),
            n_title: String::new(),
            n_body: String::new(),
            n_check: false,
            n_color: 0,
            n_labels: String::new(),
            n_focus,
            n_search: String::new(),
            n_filter: None,
            n_archived: false,
            editing: None,
            edit_title: String::new(),
            edit_body: String::new(),
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
            tk_kind: 0,
            tk: Default::default(),
            lb_text: String::new(),
            lb_scale: 3,
            lb_copies: 1,
            fun_kind: 0,
            fun_title: String::new(),
            fun_text: String::new(),
            fun_question: String::new(),
            fun_list: String::new(),
            new_reminder: String::new(),
            rem_at: false,
            rem_min: 10,
            rem_h: now.hour(),
            rem_m: now.minute(),
            rem_repeat: 0,
            pomo_task: String::new(),
            new_todo: String::new(),
            new_habit: String::new(),
            cd_name: String::new(),
            cd_date: [today.year(), today.month() as i32, today.day() as i32],
            new_url: String::new(),
            new_proc: String::new(),
            upd_busy,
            upd_installing: false,
            upd_state: None,
            upd_err: String::new(),
            upd_check_slot,
            upd_apply_slot: Arc::new(Mutex::new(None)),
            hotkey_listen: false,
            hotkey_note: String::new(),
            context_menu: autostart::context_menu_enabled(),
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

    fn goto(&mut self, page: Page, tab: usize) {
        self.page = page;
        self.tabs[page.idx()] = tab.min(page.tabs().len().saturating_sub(1));
    }

    fn tab(&self) -> usize {
        self.tabs[self.page.idx()]
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
        if self.data_at.elapsed() > Duration::from_millis(700) {
            self.refresh_data();
        }
        if self.printers_at.elapsed() > Duration::from_secs(10) {
            self.printers = printer::list_printers();
            self.printers_at = Instant::now();
        }
        for m in self.sh.take_msgs() {
            if m.starts_with("Printed") || m.contains("printed") || m.starts_with("Test slip") {
                self.just_done = Some(Instant::now());
            }
            if m.starts_with("Test slip") && !self.cfg.did_test {
                self.cfg.did_test = true;
            }
            self.say(m);
        }

        // finished iCal test
        let ical_done = self.ical_slot.lock().unwrap().take();
        if let Some(r) = ical_done {
            self.ical_busy = false;
            match r {
                Ok((url, n)) => {
                    self.cfg.ical_url = url;
                    self.ical_note = format!("Linked! Found {n} events in the next 2 weeks.");
                    self.say("Calendar linked. Hi calendar!");
                    let c = self.cfg.clone();
                    self.sh.set_cfg(c);
                    app::spawn_job(&self.sh, self.notify.clone(), "Calendar updated", |s| s.refresh_events());
                }
                Err(e) => self.ical_note = format!("That didn't work: {e}"),
            }
        }

        // finished Google sign-in
        let finished = self.google.lock().unwrap().take();
        if let Some(r) = finished {
            self.google_busy = false;
            match r {
                Ok(token) => {
                    self.cfg.google_refresh_token = token;
                    self.cfg.google_has_mail = self.connect_mail;
                    self.client_note = "Signed in. Hi Google!".into();
                    self.say("Google connected.");
                    let c = self.cfg.clone();
                    self.sh.set_cfg(c);
                    app::spawn_job(&self.sh, self.notify.clone(), "Calendar updated", |s| s.refresh_events());
                }
                Err(e) => self.client_note = format!("Sign-in failed: {e}"),
            }
        }

        // finished GitHub update check
        let upd_check = self.upd_check_slot.lock().unwrap().take();
        if let Some(r) = upd_check {
            self.upd_busy = false;
            match &r {
                Ok(update::CheckOutcome::UpToDate { message, .. }) => self.say(message.clone()),
                Ok(update::CheckOutcome::Available(rel)) => {
                    self.say(format!("Update {} is available on GitHub.", rel.tag));
                }
                Err(e) => self.say(format!("Update check failed: {e}")),
            }
            self.upd_state = Some(r);
        }

        // finished downloading update; exit so the updater script can swap the exe and restart Pip
        let upd_apply = self.upd_apply_slot.lock().unwrap().take();
        if let Some(r) = upd_apply {
            self.upd_installing = false;
            match r {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    self.upd_err = e.clone();
                    self.say(format!("Update failed: {e}"));
                }
            }
        }

        // files dropped on the window: pictures to print, Google's client .json
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let p = f.path().to_path_buf();
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ext == "json" {
                match parse_client(&p.to_string_lossy()) {
                    Some((id, secret)) => {
                        self.cfg.google_client_id = id;
                        self.cfg.google_client_secret = secret;
                        self.client_note = "Client loaded from the file. Now press sign in.".into();
                        self.connect_open = true;
                        self.connect_tab = 1;
                    }
                    None => self.say("That .json doesn't look like a Google OAuth client file."),
                }
            } else if ["png", "jpg", "jpeg", "bmp", "gif", "webp"].contains(&ext.as_str()) {
                self.img_path = p.to_string_lossy().to_string();
                self.goto(Page::Print, 1);
                self.say("Picture loaded. Check the preview, then print.");
            }
        }

        // keyboard: hotkey recorder, or command palette and page shortcuts
        if self.hotkey_listen {
            let captured = ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Key { key, pressed: true, modifiers, .. } = ev {
                        if *key == egui::Key::Escape && !modifiers.ctrl && !modifiers.alt && !modifiers.shift {
                            return Some(Err(()));
                        }
                        if let Some(vk) = egui_key_to_vk(*key) {
                            let mut mods = 0u32;
                            if modifiers.ctrl {
                                mods |= config::HOTKEY_MOD_CTRL;
                            }
                            if modifiers.alt {
                                mods |= config::HOTKEY_MOD_ALT;
                            }
                            if modifiers.shift {
                                mods |= config::HOTKEY_MOD_SHIFT;
                            }
                            return Some(Ok((mods, vk)));
                        }
                    }
                }
                None
            });
            match captured {
                Some(Err(())) => {
                    self.hotkey_listen = false;
                }
                Some(Ok((mut mods, vk))) => {
                    self.hotkey_listen = false;
                    if mods == 0 && !config::is_function_vk(vk) {
                        mods = if self.cfg.hotkey_mods != 0 {
                            self.cfg.hotkey_mods
                        } else {
                            config::HOTKEY_MOD_CTRL | config::HOTKEY_MOD_ALT
                        };
                        self.hotkey_note = "Kept modifier keys so normal typing still works.".into();
                    } else {
                        self.hotkey_note.clear();
                    }
                    self.cfg.hotkey_mods = mods;
                    self.cfg.hotkey_vk = vk;
                    self.cfg.hotkey = true;
                    let label = self.cfg.hotkey_label();
                    self.say(format!("Hotkey set to {label} (tray updates within 2s)."));
                }
                None => {}
            }
            return;
        }

        let (k, digit) = ctx.input(|i| {
            let k = i.modifiers.command && i.key_pressed(egui::Key::K);
            let keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
            ];
            let d = if i.modifiers.command {
                keys.iter().position(|key| i.key_pressed(*key))
            } else {
                None
            };
            (k, d)
        });
        if k {
            self.pal_open = !self.pal_open;
            self.pal_query.clear();
            self.pal_sel = 0;
            self.pal_focus = true;
        }
        if let Some(d) = digit {
            self.page = Page::ALL[d];
        }
    }

    fn run_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Go(p, t) => self.goto(p, t),
            Cmd::PrintAgenda => self.job("Printed today's agenda", |s| s.print_agenda(true)),
            Cmd::Briefing => self.job("Printed your morning briefing", |s| s.print_briefing()),
            Cmd::EndOfDay => self.job("Printed your end-of-day receipt", |s| s.print_end_of_day()),
            Cmd::Todos => {
                let todos = self.data.todos.clone();
                self.job("Printed your to-do list", move |s| s.print(escpos::todo_list(&s.cfg(), &todos)));
            }
            Cmd::TestSlip => self.job("Test slip printed", |s| {
                s.print(escpos::test_slip(&s.cfg()))?;
                s.mark_tested();
                Ok(())
            }),
            Cmd::ToggleFocus => self.toggle_focus(),
            Cmd::NewNote => {
                self.goto(Page::Notes, 0);
                self.n_focus = true;
            }
            Cmd::Fortune => self.job("Printed a fortune", |s| {
                s.print(escpos::fortune_slip(&s.cfg(), "FORTUNE", pip::fortune(), true))
            }),
            Cmd::Joke => self.job("Printed a joke", |s| s.print(escpos::fortune_slip(&s.cfg(), "JOKE", pip::joke(), false))),
            Cmd::Coin => {
                self.goto(Page::Print, 6);
                self.flip_coin();
            }
            Cmd::Planner => self.job("Printed the week planner", |s| s.print_planner(0)),
            Cmd::Countdowns => self.job("Printed your countdowns", |s| s.print_countdowns()),
            Cmd::ConnectCalendar => self.connect_open = true,
            Cmd::CheckUpdates => {
                self.goto(Page::Settings, 3);
                self.start_update_check();
            }
        }
    }

    fn start_update_check(&mut self) {
        if self.upd_busy || self.upd_installing {
            return;
        }
        self.upd_busy = true;
        self.upd_err.clear();
        self.say("Checking GitHub for updates...");
        let slot = self.upd_check_slot.clone();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let res = update::check_latest();
            *slot.lock().unwrap() = Some(res);
            notify();
        });
    }

    fn start_update_install(&mut self, rel: update::Release) {
        if self.upd_busy || self.upd_installing {
            return;
        }
        self.upd_installing = true;
        self.upd_err.clear();
        self.say(format!("Downloading {}...", rel.tag));
        let slot = self.upd_apply_slot.clone();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let res = update::download_and_apply(&rel);
            *slot.lock().unwrap() = Some(res);
            notify();
        });
    }

    fn toggle_focus(&mut self) {
        let mins = self.cfg.pomo_work;
        let task = self.pomo_task.trim().to_string();
        self.change_data(|d| {
            if d.pomodoro.running {
                d.pomodoro.running = false;
            } else {
                d.pomodoro.running = true;
                d.pomodoro.phase = "work".into();
                d.pomodoro.task = task;
                d.pomodoro.ends_at = (Local::now() + chrono::Duration::minutes(mins as i64)).to_rfc3339();
            }
        });
    }

    fn flip_coin(&mut self) {
        self.fun_kind = 0;
        self.fun_title = "coin flip".into();
        self.fun_text = if pip::rand(2) == 0 { "HEADS" } else { "TAILS" }.into();
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
        ui.add_space(10.0);
        let search = ui.add_sized(
            [ui.available_width(), 32.0],
            egui::Button::new(RichText::new("search...  ctrl+k").monospace().size(13.0).color(DIM))
                .fill(Color32::from_rgb(7, 10, 15))
                .stroke(Stroke::new(1.0, BORDER)),
        );
        if search.clicked() {
            self.pal_open = true;
            self.pal_query.clear();
            self.pal_sel = 0;
            self.pal_focus = true;
        }
        ui.add_space(8.0);
        for (i, p) in Page::ALL.iter().copied().enumerate() {
            let active = self.page == p;
            let label = if active { format!("> ~/{}", p.name()) } else { format!("  ~/{}", p.name()) };
            let resp = ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::new(RichText::new(label).monospace().size(15.0).color(if active { AMBER } else { DIM }))
                    .fill(if active { CARD2 } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(8)),
            );
            if resp.on_hover_text(format!("ctrl+{}", i + 1)).clicked() {
                self.page = p;
            }
        }
        ui.add_space(14.0);
        ui.separator();
        let open = self.data.todos.iter().filter(|t| !t.done).count();
        ui.label(mono(&format!("{open} tasks open"), DIM));
        ui.label(mono(&format!("{} notes", self.data.notes.iter().filter(|n| !n.archived).count()), DIM));
        ui.add_space(4.0);
        ui.label(mono(&format!("v{}", env!("CARGO_PKG_VERSION")), DIM));
        ui.horizontal_wrapped(|ui| {
            pill(ui, "rust", AMBER);
            pill(ui, "esc/pos", CYAN);
        });
        ui.label(mono(&self.cfg.hotkey_label().to_lowercase(), DIM));
        ui.label(mono("= print selection", DIM));
        ui.add_space(6.0);
        ui.hyperlink_to(mono("made by miketsak.gr", CYAN), "https://miketsak.gr");
    }

    fn status_bar(&self, ui: &mut Ui) {
        let fresh = self.status_at.elapsed() < Duration::from_secs(12);
        ui.horizontal_centered(|ui| {
            ui.label(mono("pip@desk:~$", GREEN));
            ui.label(mono(&self.status, if fresh { TEXT } else { DIM }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.hyperlink_to(mono("made by miketsak.gr", DIM), "https://miketsak.gr");
            });
        });
    }

    // ------------------------------------------------------ command palette

    fn palette(&mut self, ctx: &egui::Context) {
        if !self.pal_open {
            return;
        }
        let cmds = all_commands();
        let q = self.pal_query.to_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let matches: Vec<usize> = cmds
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| tokens.iter().all(|t| label.contains(t)))
            .map(|(i, _)| i)
            .take(10)
            .collect();
        self.pal_sel = self.pal_sel.min(matches.len().saturating_sub(1));

        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if down && self.pal_sel + 1 < matches.len() {
            self.pal_sel += 1;
        }
        if up && self.pal_sel > 0 {
            self.pal_sel -= 1;
        }
        let mut run: Option<Cmd> = None;
        if enter {
            run = matches.get(self.pal_sel).map(|i| cmds[*i].1);
        }
        if esc {
            self.pal_open = false;
            return;
        }

        egui::Window::new("palette")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
            .fixed_size([560.0, 0.0])
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.5, CYAN))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(mono("pip>", GREEN));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.pal_query)
                            .hint_text("type a page or an action: agenda, note, focus, coin...")
                            .desired_width(f32::INFINITY),
                    );
                    if self.pal_focus {
                        r.request_focus();
                        self.pal_focus = false;
                    }
                });
                ui.separator();
                if matches.is_empty() {
                    ui.label(mono("no match", DIM));
                }
                for (rank, i) in matches.iter().enumerate() {
                    let selected = rank == self.pal_sel;
                    let resp = ui.selectable_label(selected, mono(&cmds[*i].0, if selected { AMBER } else { TEXT }));
                    if resp.clicked() {
                        run = Some(cmds[*i].1);
                    }
                }
                ui.label(mono("enter = run   up/down = choose   esc = close", DIM));
            });

        if let Some(cmd) = run {
            self.pal_open = false;
            self.run_cmd(cmd);
        }
    }

    // ------------------------------------------------- connect calendar popup

    fn connect_window(&mut self, ctx: &egui::Context) {
        if !self.connect_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Connect your calendar")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([640.0, 0.0])
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.5, AMBER))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(18)),
            )
            .show(ctx, |ui| {
                tab_bar(ui, &mut self.connect_tab, &["quick link (easy)", "google sign-in (calendar + mail)"]);
                if self.connect_tab == 0 {
                    self.connect_link(ui);
                } else {
                    self.connect_google_tab(ui);
                }
            });
        if !open {
            self.connect_open = false;
        }
    }

    fn connect_link(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Link your Google Calendar in 3 steps. No Google Cloud, no sign-in.").strong());
        ui.label(mono("pip only reads the link; it can't change your calendar", DIM));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(mono("1", CYAN));
            if action(ui, "open google calendar settings", AMBER).clicked() {
                open_url("https://calendar.google.com/calendar/u/0/r/settings");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(mono("2", CYAN));
            ui.label("Click your calendar on the left, scroll to \"Integrate calendar\" and copy the");
            ui.label(RichText::new("Secret address in iCal format").strong().color(AMBER));
        });
        ui.horizontal(|ui| {
            ui.label(mono("3", CYAN));
            if action(ui, "paste from clipboard", CYAN).clicked() {
                if let Some(t) = sysinfo::clipboard_text() {
                    self.ical_input = t.trim().to_string();
                }
            }
            text_field(ui, &mut self.ical_input, "https://calendar.google.com/calendar/ical/.../basic.ics", 330.0);
        });
        ui.horizontal(|ui| {
            let can = !self.ical_input.trim().is_empty() && !self.ical_busy;
            let label = if self.ical_busy { "checking..." } else { "test and link" };
            if ui.add_enabled(can, egui::Button::new(mono(label, INK).strong()).fill(GREEN)).clicked() {
                let url = self.ical_input.trim().to_string();
                let slot = self.ical_slot.clone();
                let ctx = ui.ctx().clone();
                self.ical_busy = true;
                self.ical_note = "Checking the link...".into();
                std::thread::spawn(move || {
                    let r = ical::fetch(&url, 14).map(|e| (url.clone(), e.len()));
                    *slot.lock().unwrap() = Some(r);
                    ctx.request_repaint();
                });
            }
            if !self.cfg.ical_url.is_empty() && ghost(ui, "remove link").clicked() {
                self.cfg.ical_url.clear();
                self.sh.events.lock().unwrap().clear();
                *self.sh.last_fetch.lock().unwrap() = None;
                self.ical_note = "Link removed.".into();
            }
        });
        if !self.ical_note.is_empty() {
            let ok = self.ical_note.starts_with("Linked");
            ui.colored_label(if ok { GREEN } else { AMBER }, &self.ical_note);
        }
        if !self.cfg.ical_url.is_empty() {
            pill(ui, "link active", GREEN);
        }
        ui.label(mono("tip: anyone with this link can read your calendar. keep it private.", DIM));
    }

    fn connect_google_tab(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Full Google sign-in: needed for Gmail mail slips.").strong());
        ui.label(mono("a free one-time setup in Google Cloud, then it's one click", DIM));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(mono("1", CYAN));
            if ghost(ui, "open Calendar API").clicked() {
                open_url("https://console.cloud.google.com/apis/library/calendar-json.googleapis.com");
            }
            if ghost(ui, "open Gmail API").clicked() {
                open_url("https://console.cloud.google.com/apis/library/gmail.googleapis.com");
            }
            ui.label("press Enable on each");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(mono("2", CYAN));
            if ghost(ui, "open consent screen").clicked() {
                open_url("https://console.cloud.google.com/apis/credentials/consent");
            }
            ui.label("External, add yourself as a test user");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(mono("3", CYAN));
            if ghost(ui, "open credentials").clicked() {
                open_url("https://console.cloud.google.com/apis/credentials");
            }
            ui.label("Create credentials > OAuth client ID > Desktop app > Download JSON");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(mono("4", CYAN));
            ui.label("Drop the downloaded .json on this window, or");
            if action(ui, "paste path or JSON", CYAN).clicked() {
                match sysinfo::clipboard_text().and_then(|t| parse_client(&t)) {
                    Some((id, secret)) => {
                        self.cfg.google_client_id = id;
                        self.cfg.google_client_secret = secret;
                        self.client_note = "Client loaded. Now press sign in.".into();
                    }
                    None => self.client_note = "The clipboard doesn't hold the client JSON (or a path to it).".into(),
                }
            }
        });
        let loaded = !self.cfg.google_client_id.is_empty() && !self.cfg.google_client_secret.is_empty();
        ui.horizontal(|ui| {
            ui.label(mono("5", CYAN));
            toggle(ui, &mut self.connect_mail, "also allow reading Gmail");
            let label = if self.google_busy { "waiting for browser..." } else { "sign in with google" };
            if ui.add_enabled(loaded && !self.google_busy, egui::Button::new(mono(label, INK).strong()).fill(GREEN)).clicked() {
                self.connect_google(ui.ctx().clone());
            }
            if loaded {
                pill(ui, "client loaded", GREEN);
            }
        });
        if !self.client_note.is_empty() {
            ui.colored_label(AMBER, &self.client_note);
        }
        if !self.cfg.google_refresh_token.is_empty() {
            ui.horizontal(|ui| {
                pill(ui, "signed in", GREEN);
                if ghost(ui, "sign out").clicked() {
                    self.cfg.google_refresh_token.clear();
                    self.cfg.google_has_mail = false;
                }
            });
        }
    }

    fn connect_google(&mut self, ctx: egui::Context) {
        let id = self.cfg.google_client_id.trim().to_string();
        let secret = self.cfg.google_client_secret.trim().to_string();
        let with_mail = self.connect_mail;
        let slot = self.google.clone();
        self.google_busy = true;
        self.client_note = "Opening your browser for Google sign-in...".into();
        std::thread::spawn(move || {
            let r = gcal::connect(&id, &secret, with_mail);
            *slot.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
    }

    // --------------------------------------------------------------- Home

    fn tile(&self, ui: &mut Ui, width: f32, tag: &str, value: &str, sub: &str, color: Color32) -> bool {
        let r = egui::Frame::new()
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
        r.response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
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
                        ui.hyperlink_to(mono("made by miketsak.gr", CYAN), "https://miketsak.gr");
                    });
                });
            });
        });

        self.setup_checklist(ui);

        if let Some(Ok(update::CheckOutcome::Available(rel))) = &self.upd_state {
            let tag = rel.tag.clone();
            card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    pill(ui, &format!("update: {tag}"), AMBER);
                    ui.label("A new Thermo Companion version is ready on GitHub.");
                    if action(ui, "open updates", AMBER).clicked() {
                        self.goto(Page::Settings, 3);
                    }
                });
            });
        }

        if !self.data.print_queue.is_empty() {
            let count = self.data.print_queue.len();
            card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    pill(ui, &format!("queued: {count}"), AMBER);
                    ui.label("Waiting for your printer to come back online.");
                    if action(ui, "retry now", GREEN).clicked() {
                        self.job("Printed queued slips", |s| {
                            let n = app::flush_print_queue(s)?;
                            if n == 0 {
                                return Err("Printer is still offline.".into());
                            }
                            Ok(())
                        });
                    }
                    if ghost(ui, "clear queue").clicked() {
                        self.change_data(|d| d.print_queue.clear());
                    }
                });
            });
        }

        // status tiles
        let (focus_value, focus_sub, focus_color) = match self.countdown() {
            Some((phase, secs)) => (fmt_countdown(secs), format!("{phase} - running"), AMBER),
            None => ("idle".to_string(), "no focus timer".to_string(), DIM),
        };
        let open = self.data.todos.iter().filter(|t| !t.done).count();
        let next = self.sh.events.lock().unwrap().iter().find(|e| e.start > now).cloned();
        let counts = app::countdown_list(&self.data);
        let tw = ((ui.available_width() - 4.0 * 30.0 - 3.0 * 10.0) / 4.0).max(110.0);
        let mut go: Option<(Page, usize)> = None;
        ui.horizontal(|ui| {
            let printer_text = if self.printer_ok() { "online" } else { "offline" };
            if self.tile(ui, tw, "PRINTER", printer_text, &self.cfg.printer, if self.printer_ok() { GREEN } else { RED }) {
                go = Some((Page::Settings, 0));
            }
            if self.tile(ui, tw, "FOCUS", &focus_value, &focus_sub, focus_color) {
                go = Some((Page::Schedule, 4));
            }
            if self.tile(ui, tw, "TASKS", &format!("{open} open"), &format!("{} notes", self.data.notes.iter().filter(|n| !n.archived).count()), CYAN) {
                go = Some((Page::Tasks, 0));
            }
            let clicked = match &next {
                Some(e) => self.tile(ui, tw, "NEXT EVENT", &e.start.format("%a %H:%M").to_string(), &e.summary, AMBER),
                None => self.tile(ui, tw, "NEXT EVENT", "none", if self.sh.connected() { "calendar is clear" } else { "link your calendar" }, DIM),
            };
            if clicked {
                go = Some((Page::Schedule, 0));
            }
        });
        if let Some((p, t)) = go {
            self.goto(p, t);
        }

        if let Some((name, days)) = counts.iter().find(|c| c.1 >= 0) {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(mono("countdown", CYAN));
                    ui.label(RichText::new(name).strong());
                    ui.label(mono(if *days == 0 { "TODAY" } else { "in" }, DIM));
                    if *days > 0 {
                        ui.label(RichText::new(format!("{days} days")).size(18.0).strong().color(AMBER));
                    }
                });
            });
        }

        // quick actions
        section(ui, "$", "quick actions", |ui| {
            ui.horizontal_wrapped(|ui| {
                if action(ui, "print agenda", AMBER).clicked() {
                    self.run_cmd(Cmd::PrintAgenda);
                }
                if action(ui, "morning briefing", CYAN).clicked() {
                    self.run_cmd(Cmd::Briefing);
                }
                if action(ui, "end-of-day receipt", PINK).clicked() {
                    self.run_cmd(Cmd::EndOfDay);
                }
                if action(ui, "to-do list", GREEN).clicked() {
                    self.run_cmd(Cmd::Todos);
                }
                if action(ui, "new note", AMBER).clicked() {
                    self.run_cmd(Cmd::NewNote);
                }
                if action(ui, "fortune cookie", CYAN).clicked() {
                    self.run_cmd(Cmd::Fortune);
                }
                if action(ui, "test slip", DIM).clicked() {
                    self.run_cmd(Cmd::TestSlip);
                }
            });
            ui.label(mono("tip: ctrl+k opens search. type anything, hit enter.", DIM));
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

    fn setup_checklist(&mut self, ui: &mut Ui) {
        if self.cfg.setup_dismissed {
            return;
        }
        let items: [(&str, bool, Page, usize); 4] = [
            ("pick your printer", self.printer_ok(), Page::Settings, 0),
            ("print a test slip", self.cfg.did_test, Page::Settings, 0),
            ("link your calendar", self.cfg.calendar_linked(), Page::Settings, 2),
            ("set a morning briefing", self.cfg.briefing, Page::Schedule, 2),
        ];
        let done = items.iter().filter(|i| i.1).count();
        if done == items.len() {
            return;
        }
        section(ui, "setup", &format!("getting started  {done}/{}", items.len()), |ui| {
            for (label, ok, page, tab) in items {
                ui.horizontal(|ui| {
                    ui.label(mono(if ok { "[x]" } else { "[ ]" }, if ok { GREEN } else { AMBER }));
                    ui.label(if ok { RichText::new(label).color(DIM) } else { RichText::new(label) });
                    if !ok && ghost(ui, "do it").clicked() {
                        if label == "link your calendar" {
                            self.connect_open = true;
                        } else {
                            self.goto(page, tab);
                        }
                    }
                });
            }
            if small_btn(ui, "hide this").clicked() {
                self.cfg.setup_dismissed = true;
            }
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

    // -------------------------------------------------------------- Notes

    fn page_notes(&mut self, ui: &mut Ui) {
        page_title(ui, "~/notes", "// quick notes and checklists, keep-style");

        // composer
        section(ui, "+", "new note", |ui| {
            let title = ui.add(egui::TextEdit::singleline(&mut self.n_title).hint_text("title (optional)").desired_width(f32::INFINITY));
            if self.n_focus {
                title.request_focus();
                self.n_focus = false;
            }
            ui.add(
                egui::TextEdit::multiline(&mut self.n_body)
                    .hint_text(if self.n_check { "one item per line" } else { "take a note..." })
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                toggle(ui, &mut self.n_check, "checklist");
                ui.label(mono("labels", DIM));
                text_field(ui, &mut self.n_labels, "work, ideas", 160.0);
                swatches(ui, &mut self.n_color);
            });
            ui.horizontal(|ui| {
                if action(ui, "save note", AMBER).clicked() {
                    self.save_new_note(false);
                }
                if action(ui, "save + print", CYAN).clicked() {
                    self.save_new_note(true);
                }
                if ghost(ui, "clear").clicked() {
                    self.n_title.clear();
                    self.n_body.clear();
                }
            });
        });

        // search + filters
        ui.horizontal_wrapped(|ui| {
            text_field(ui, &mut self.n_search, "search notes", 220.0);
            let mut labels: Vec<String> = self
                .data
                .notes
                .iter()
                .filter(|n| n.archived == self.n_archived)
                .flat_map(|n| n.labels.clone())
                .collect();
            labels.sort();
            labels.dedup();
            for l in labels {
                let active = self.n_filter.as_deref() == Some(l.as_str());
                if ui.selectable_label(active, mono(&format!("#{l}"), if active { AMBER } else { CYAN })).clicked() {
                    self.n_filter = if active { None } else { Some(l) };
                }
            }
            toggle(ui, &mut self.n_archived, "archive");
        });

        let q = self.n_search.to_lowercase();
        let mut notes: Vec<Note> = self
            .data
            .notes
            .iter()
            .filter(|n| n.archived == self.n_archived)
            .filter(|n| self.n_filter.as_ref().map_or(true, |f| n.labels.contains(f)))
            .filter(|n| {
                q.is_empty()
                    || n.title.to_lowercase().contains(&q)
                    || n.body.to_lowercase().contains(&q)
                    || n.items.iter().any(|i| i.text.to_lowercase().contains(&q))
                    || n.labels.iter().any(|l| l.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        notes.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated.cmp(&a.updated)));

        if notes.is_empty() {
            ui.label(mono(if self.n_archived { "no archived notes" } else { "no notes yet. jot something down!" }, DIM));
            return;
        }
        let cols = ((ui.available_width() / 300.0) as usize).clamp(1, 3);
        ui.columns(cols, |columns| {
            for (i, n) in notes.iter().enumerate() {
                self.note_card(&mut columns[i % cols], n);
            }
        });
    }

    fn save_new_note(&mut self, print: bool) {
        if self.n_title.trim().is_empty() && self.n_body.trim().is_empty() {
            return;
        }
        let labels: Vec<String> = self
            .n_labels
            .split(',')
            .map(|l| l.trim().trim_start_matches('#').to_lowercase())
            .filter(|l| !l.is_empty())
            .collect();
        let items: Vec<NoteItem> = if self.n_check {
            self.n_body
                .lines()
                .map(|l| l.trim().trim_start_matches("[ ]").trim_start_matches("- ").trim())
                .filter(|l| !l.is_empty())
                .map(|l| NoteItem { text: l.to_string(), done: false })
                .collect()
        } else {
            Vec::new()
        };
        let note_template = Note {
            id: 0,
            title: self.n_title.trim().to_string(),
            body: if self.n_check { String::new() } else { self.n_body.trim_end().to_string() },
            checklist: self.n_check,
            items,
            color: self.n_color,
            pinned: false,
            archived: false,
            labels,
            updated: Local::now().to_rfc3339(),
        };
        let for_print = note_template.clone();
        self.change_data(|d| {
            let id = d.new_id();
            d.notes.push(Note { id, ..note_template });
        });
        if print {
            self.job("Printed the note", move |s| s.print(escpos::note_slip(&s.cfg(), &for_print)));
        }
        self.n_title.clear();
        self.n_body.clear();
        self.n_labels.clear();
        self.say("Note saved");
    }

    fn note_card(&mut self, ui: &mut Ui, n: &Note) {
        let color = NOTE_COLORS[(n.color as usize).min(NOTE_COLORS.len() - 1)].1;
        let width = (ui.available_width() - 34.0).max(120.0);
        let id = n.id;
        egui::Frame::new()
            .fill(color)
            .stroke(Stroke::new(1.0, lighten(color)))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(width);
                if self.editing == Some(id) {
                    ui.add(egui::TextEdit::singleline(&mut self.edit_title).hint_text("title").desired_width(f32::INFINITY));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.edit_body)
                            .hint_text(if n.checklist { "one item per line" } else { "text" })
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if action(ui, "save", GREEN).clicked() {
                            let (title, body) = (self.edit_title.trim().to_string(), self.edit_body.clone());
                            self.change_data(|d| {
                                if let Some(x) = d.notes.iter_mut().find(|x| x.id == id) {
                                    x.title = title;
                                    if x.checklist {
                                        let old = x.items.clone();
                                        x.items = body
                                            .lines()
                                            .map(|l| l.trim())
                                            .filter(|l| !l.is_empty())
                                            .map(|l| NoteItem {
                                                text: l.to_string(),
                                                done: old.iter().any(|o| o.text == l && o.done),
                                            })
                                            .collect();
                                    } else {
                                        x.body = body.trim_end().to_string();
                                    }
                                    x.updated = Local::now().to_rfc3339();
                                }
                            });
                            self.editing = None;
                        }
                        if ghost(ui, "cancel").clicked() {
                            self.editing = None;
                        }
                    });
                    return;
                }

                if n.pinned {
                    ui.label(mono("pinned", AMBER));
                }
                if !n.title.is_empty() {
                    ui.label(RichText::new(&n.title).strong().size(16.0));
                }
                if n.checklist {
                    for (i, it) in n.items.iter().enumerate() {
                        let mut done = it.done;
                        let label = if it.done { RichText::new(&it.text).strikethrough().color(DIM) } else { RichText::new(&it.text) };
                        if ui.checkbox(&mut done, label).changed() {
                            self.change_data(|d| {
                                if let Some(x) = d.notes.iter_mut().find(|x| x.id == id) {
                                    if let Some(item) = x.items.get_mut(i) {
                                        item.done = done;
                                    }
                                }
                            });
                        }
                    }
                } else if !n.body.is_empty() {
                    ui.label(&n.body);
                }
                if !n.labels.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for l in &n.labels {
                            ui.label(mono(&format!("#{l}"), CYAN));
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    if small_btn(ui, if n.pinned { "unpin" } else { "pin" }).clicked() {
                        self.change_data(|d| {
                            if let Some(x) = d.notes.iter_mut().find(|x| x.id == id) {
                                x.pinned = !x.pinned;
                            }
                        });
                    }
                    if small_btn(ui, "color").clicked() {
                        self.change_data(|d| {
                            if let Some(x) = d.notes.iter_mut().find(|x| x.id == id) {
                                x.color = (x.color + 1) % NOTE_COLORS.len() as u8;
                            }
                        });
                    }
                    if small_btn(ui, "edit").clicked() {
                        self.editing = Some(id);
                        self.edit_title = n.title.clone();
                        self.edit_body = if n.checklist {
                            n.items.iter().map(|i| i.text.clone()).collect::<Vec<_>>().join("\n")
                        } else {
                            n.body.clone()
                        };
                    }
                    if small_btn(ui, "print").clicked() {
                        let note = n.clone();
                        self.job("Printed the note", move |s| s.print(escpos::note_slip(&s.cfg(), &note)));
                    }
                    if small_btn(ui, if n.archived { "restore" } else { "archive" }).clicked() {
                        self.change_data(|d| {
                            if let Some(x) = d.notes.iter_mut().find(|x| x.id == id) {
                                x.archived = !x.archived;
                            }
                        });
                    }
                    if small_btn(ui, "delete").clicked() {
                        self.change_data(|d| d.notes.retain(|x| x.id != id));
                    }
                });
            });
    }

    // -------------------------------------------------------------- Print

    fn page_print(&mut self, ui: &mut Ui) {
        let mut tab = self.tab();
        page_title(ui, &format!("~/print/{}", PRINT_TABS[tab].replace(' ', "-")), "// text, pictures, tickets, labels and fun");
        tab_bar(ui, &mut tab, &PRINT_TABS);
        self.tabs[Page::Print.idx()] = tab;
        match tab {
            0 => self.print_text(ui),
            1 => self.print_picture(ui),
            2 => self.print_qr(ui),
            3 => self.print_tickets(ui),
            4 => self.print_labels(ui),
            5 => self.print_stickers(ui),
            _ => self.print_fun(ui),
        }
        self.print_history_card(ui);
    }

    fn print_history_card(&mut self, ui: &mut Ui) {
        if self.data.print_history.is_empty() && self.data.print_queue.is_empty() {
            return;
        }
        section(ui, "log", "recent & queued slips", |ui| {
            if !self.data.print_queue.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    pill(ui, &format!("queued: {}", self.data.print_queue.len()), AMBER);
                    if action(ui, "retry queue", GREEN).clicked() {
                        self.job("Printed queued slips", |s| {
                            let n = app::flush_print_queue(s)?;
                            if n == 0 {
                                return Err("Printer is still offline.".into());
                            }
                            Ok(())
                        });
                    }
                    if ghost(ui, "clear queue").clicked() {
                        self.change_data(|d| d.print_queue.clear());
                    }
                });
            }
            for job in self.data.print_history.clone() {
                ui.horizontal(|ui| {
                    ui.label(mono(&job.time, DIM));
                    ui.label(&job.label);
                    if ghost(ui, "reprint").clicked() {
                        if let Some(bytes) = data::hex_decode(&job.hex) {
                            let lbl = job.label.clone();
                            self.job("Reprinted slip", move |s| s.print_as(&lbl, bytes));
                        }
                    }
                });
            }
        });
    }

    fn print_text(&mut self, ui: &mut Ui) {
        section(ui, "01", "text", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.txt)
                    .hint_text(format!("type anything, or press {} in any app to print what you selected", self.cfg.hotkey_label().to_lowercase()))
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                toggle(ui, &mut self.t_bold, "bold");
                toggle(ui, &mut self.t_big, "big");
                toggle(ui, &mut self.t_center, "centered");
                toggle(ui, &mut self.t_stamp, "date and time");
                toggle(ui, &mut self.t_check, "checklist");
            });
            let opts = TextOpts { bold: self.t_bold, big: self.t_big, center: self.t_center, stamp: self.t_stamp, checklist: self.t_check };
            ui.horizontal(|ui| {
                if action(ui, "print text", AMBER).clicked() && !self.txt.trim().is_empty() {
                    let text = self.txt.clone();
                    let label = format!("Text: {}", text.lines().next().unwrap_or("").chars().take(28).collect::<String>());
                    let o = TextOpts { bold: opts.bold, big: opts.big, center: opts.center, stamp: opts.stamp, checklist: opts.checklist };
                    self.job("Printed", move |s| s.print_as(&label, escpos::text_slip(&s.cfg(), &text, &o)));
                }
                if ghost(ui, "save as note").clicked() && !self.txt.trim().is_empty() {
                    let body = self.txt.trim_end().to_string();
                    self.change_data(|d| {
                        let id = d.new_id();
                        d.notes.push(Note { id, body, updated: Local::now().to_rfc3339(), ..Default::default() });
                    });
                    self.say("Saved as a note");
                }
                if ghost(ui, "clear").clicked() {
                    self.txt.clear();
                }
            });
            if !self.txt.trim().is_empty() {
                ui.add_space(4.0);
                let w = if self.t_big { (self.cfg.paper_chars.max(16) / 2).max(8) } else { self.cfg.paper_chars.max(16) };
                ui.label(mono(&format!("receipt preview ({w} chars/line)"), DIM));
                let preview = escpos::preview_text(&self.cfg, &self.txt, &opts);
                let ruler: String = (0..w).map(|i| char::from(b'0' + ((i + 1) % 10) as u8)).collect();
                egui::Frame::new()
                    .fill(Color32::from_rgb(7, 10, 15))
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(mono(&ruler, DIM));
                        ui.label(mono(&preview, TEXT));
                    });
            }
        });
    }

    fn print_picture(&mut self, ui: &mut Ui) {
        section(ui, "02", "picture", |ui| {
            ui.label(mono("drop a png / jpg / gif / webp anywhere on this window, or paste a path", DIM));
            ui.horizontal(|ui| {
                text_field(ui, &mut self.img_path, r"C:\Pictures\cat.png", 420.0);
                if ghost(ui, "paste").clicked() {
                    if let Some(t) = sysinfo::clipboard_text() {
                        self.img_path = t.trim().trim_matches('"').to_string();
                    }
                }
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
                        let pixels: Vec<Color32> = bm.dots.iter().map(|d| if *d { Color32::BLACK } else { Color32::WHITE }).collect();
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
    }

    fn print_qr(&mut self, ui: &mut Ui) {
        section(ui, "03", "qr code", |ui| {
            ui.horizontal(|ui| {
                text_field(ui, &mut self.qr_text, "a link or any text", 360.0);
                if action(ui, "print qr", AMBER).clicked() && !self.qr_text.trim().is_empty() {
                    let t = self.qr_text.trim().to_string();
                    self.job("Printed the QR code", move |s| s.print(escpos::qr_slip(&s.cfg(), "SCAN ME", &t, &[t.clone()])?));
                }
            });
        });
        section(ui, "04", "wi-fi slip for guests", |ui| {
            ui.horizontal_wrapped(|ui| {
                text_field(ui, &mut self.wifi_ssid, "network name", 200.0);
                ui.add_enabled(!self.wifi_open, egui::TextEdit::singleline(&mut self.wifi_pass).hint_text("password").desired_width(200.0));
                toggle(ui, &mut self.wifi_open, "open network");
            });
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
    }

    fn print_tickets(&mut self, ui: &mut Ui) {
        section(ui, "05", "ticket maker", |ui| {
            ui.label(mono("print a fancy ticket for anything: a trip, a date night, a coupon for a hug", DIM));
            ui.horizontal(|ui| {
                for (i, name) in ["event ticket", "boarding pass"].iter().enumerate() {
                    if ui.selectable_label(self.tk_kind == i, mono(name, if self.tk_kind == i { AMBER } else { DIM })).clicked() {
                        self.tk_kind = i;
                    }
                }
            });
            let labels: [&str; 6] = if self.tk_kind == 0 {
                ["event", "date and time", "where", "seat", "price", ""]
            } else {
                ["from", "to", "date and time", "flight", "seat / gate", "passenger"]
            };
            for (i, l) in labels.iter().enumerate() {
                if l.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label(mono(&format!("{l:<14}"), DIM));
                    text_field(ui, &mut self.tk[i], "", 340.0);
                });
            }
            if action(ui, "print ticket", PINK).clicked() {
                let t = self.tk.clone();
                if self.tk_kind == 0 {
                    self.job("Printed the ticket", move |s| s.print(escpos::event_ticket(&s.cfg(), &t[0], &t[1], &t[2], &t[3], &t[4])));
                } else {
                    self.job("Printed the boarding pass", move |s| {
                        s.print(escpos::boarding_pass(&s.cfg(), &t[0], &t[1], &t[2], &t[3], &t[4], &t[5]))
                    });
                }
            }
        });
    }

    fn print_labels(&mut self, ui: &mut Ui) {
        section(ui, "06", "label maker", |ui| {
            ui.label(mono("big bold text for jars, boxes, cables and doors", DIM));
            text_field(ui, &mut self.lb_text, "FRAGILE", 360.0);
            ui.horizontal(|ui| {
                ui.label("size");
                ui.add(egui::Slider::new(&mut self.lb_scale, 1..=6));
                ui.label("copies");
                ui.add(egui::DragValue::new(&mut self.lb_copies).range(1..=10));
            });
            if action(ui, "print label", AMBER).clicked() && !self.lb_text.trim().is_empty() {
                let (t, sc, n) = (self.lb_text.trim().to_string(), self.lb_scale, self.lb_copies);
                self.job("Printed the label", move |s| s.print(escpos::big_label(&s.cfg(), &t, sc, n)));
            }
        });
    }

    fn print_stickers(&mut self, ui: &mut Ui) {
        section(ui, "07", "pip stickers", |ui| {
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

    fn print_fun(&mut self, ui: &mut Ui) {
        section(ui, "08", "pip decides", |ui| {
            ui.label(mono("can't choose? let a robot printer do it", DIM));
            ui.horizontal_wrapped(|ui| {
                if action(ui, "flip a coin", AMBER).clicked() {
                    self.flip_coin();
                }
                if action(ui, "roll d6", CYAN).clicked() {
                    self.fun_kind = 0;
                    self.fun_title = "d6".into();
                    self.fun_text = (1 + pip::rand(6)).to_string();
                }
                if action(ui, "roll d20", CYAN).clicked() {
                    self.fun_kind = 0;
                    self.fun_title = "d20".into();
                    self.fun_text = (1 + pip::rand(20)).to_string();
                }
            });
            ui.horizontal(|ui| {
                text_field(ui, &mut self.fun_question, "ask a yes/no question", 300.0);
                if action(ui, "magic 8-ball", PINK).clicked() {
                    self.fun_kind = 0;
                    self.fun_title = if self.fun_question.trim().is_empty() { "8-ball".into() } else { self.fun_question.trim().to_string() };
                    self.fun_text = pip::eight_ball().into();
                }
            });
            ui.add(egui::TextEdit::multiline(&mut self.fun_list).hint_text("or list options, one per line, and let pip pick").desired_rows(3).desired_width(420.0));
            if action(ui, "pick one", GREEN).clicked() {
                let opts: Vec<&str> = self.fun_list.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
                if !opts.is_empty() {
                    self.fun_kind = 0;
                    self.fun_title = "pip picks".into();
                    self.fun_text = opts[pip::rand(opts.len())].to_string();
                }
            }
        });
        section(ui, "09", "fortune cookie and jokes", |ui| {
            ui.horizontal(|ui| {
                if action(ui, "fortune", AMBER).clicked() {
                    self.fun_kind = 1;
                    self.fun_title = "FORTUNE".into();
                    self.fun_text = pip::fortune().into();
                }
                if action(ui, "joke", CYAN).clicked() {
                    self.fun_kind = 2;
                    self.fun_title = "JOKE".into();
                    self.fun_text = pip::joke().into();
                }
            });
        });
        if !self.fun_text.is_empty() {
            card(ui, |ui| {
                ui.label(mono(&self.fun_title, DIM));
                ui.label(RichText::new(&self.fun_text).size(22.0).strong().color(AMBER));
                if action(ui, "print it", PINK).clicked() {
                    let (kind, title, text) = (self.fun_kind, self.fun_title.clone(), self.fun_text.clone());
                    self.job("Printed", move |s| {
                        let cfg = s.cfg();
                        s.print(match kind {
                            1 => escpos::fortune_slip(&cfg, "FORTUNE", &text, true),
                            2 => escpos::fortune_slip(&cfg, "JOKE", &text, false),
                            _ => escpos::decide_slip(&cfg, &title, &text),
                        })
                    });
                }
            });
        }
    }

    // ----------------------------------------------------------- Schedule

    fn page_schedule(&mut self, ui: &mut Ui) {
        let mut tab = self.tab();
        page_title(ui, &format!("~/schedule/{}", SCHED_TABS[tab].replace(' ', "-")), "// calendar, briefings, reminders and focus");
        tab_bar(ui, &mut tab, &SCHED_TABS);
        self.tabs[Page::Schedule.idx()] = tab;
        match tab {
            0 => self.sched_calendar(ui),
            1 => self.sched_agenda(ui),
            2 => self.sched_briefing(ui),
            3 => self.sched_reminders(ui),
            4 => self.sched_focus(ui),
            5 => self.sched_eod(ui),
            _ => self.sched_planner(ui),
        }
    }

    fn sched_calendar(&mut self, ui: &mut Ui) {
        section(ui, "01", "calendar", |ui| {
            if !self.sh.connected() {
                ui.label("Pip can't see your calendar yet. Link it in about a minute, no Google Cloud needed.");
                if action(ui, "connect calendar", AMBER).clicked() {
                    self.connect_open = true;
                }
                return;
            }
            ui.horizontal(|ui| {
                pill(ui, if self.cfg.ical_url.is_empty() { "google sign-in" } else { "calendar link" }, GREEN);
                if ghost(ui, "refresh").clicked() {
                    self.job("Calendar updated", |s| s.refresh_events());
                }
                if ghost(ui, "change").clicked() {
                    self.connect_open = true;
                }
            });
            let events: Vec<_> = self
                .sh
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.start >= Local::now() - chrono::Duration::hours(1))
                .take(10)
                .cloned()
                .collect();
            if events.is_empty() {
                ui.label(mono("nothing coming up. suspicious.", DIM));
            }
            for e in &events {
                ui.horizontal(|ui| {
                    ui.label(mono(&e.start.format("%a %d %b %H:%M").to_string(), AMBER));
                    ui.label(&e.summary);
                });
            }
        });
    }

    fn sched_agenda(&mut self, ui: &mut Ui) {
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
            if action(ui, "print agenda now", AMBER).clicked() {
                self.run_cmd(Cmd::PrintAgenda);
            }
        });
    }

    fn sched_briefing(&mut self, ui: &mut Ui) {
        section(ui, "03", "morning briefing", |ui| {
            ui.label(mono("weather + agenda + to-dos + countdowns + a line from pip, all on one slip", DIM));
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
                self.run_cmd(Cmd::Briefing);
            }
        });
    }

    fn sched_reminders(&mut self, ui: &mut Ui) {
        section(ui, "04", "reminders and timers", |ui| {
            text_field(ui, &mut self.new_reminder, "remind me to...", 360.0);
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(!self.rem_at, mono("in", if !self.rem_at { AMBER } else { DIM })).clicked() {
                    self.rem_at = false;
                }
                ui.add_enabled(!self.rem_at, egui::DragValue::new(&mut self.rem_min).range(1..=1440).suffix(" min"));
                if ui.selectable_label(self.rem_at, mono("at", if self.rem_at { AMBER } else { DIM })).clicked() {
                    self.rem_at = true;
                }
                ui.add_enabled_ui(self.rem_at, |ui| time_edit(ui, &mut self.rem_h, &mut self.rem_m));
                ui.label("repeat");
                egui::ComboBox::from_id_salt("repeat").selected_text(REPEATS[self.rem_repeat].0).show_ui(ui, |ui| {
                    for (i, (name, _)) in REPEATS.iter().enumerate() {
                        ui.selectable_value(&mut self.rem_repeat, i, *name);
                    }
                });
                if action(ui, "add", AMBER).clicked() && !self.new_reminder.trim().is_empty() {
                    let now = Local::now();
                    let due = if self.rem_at {
                        let mut t = now.date_naive().and_hms_opt(self.rem_h, self.rem_m, 0).and_then(|n| Local.from_local_datetime(&n).earliest()).unwrap_or(now);
                        if t <= now {
                            t += chrono::Duration::days(1);
                        }
                        t
                    } else {
                        now + chrono::Duration::minutes(self.rem_min as i64)
                    };
                    let text = self.new_reminder.trim().to_string();
                    let repeat = REPEATS[self.rem_repeat].1.to_string();
                    self.add_reminder(text, due, repeat);
                    self.new_reminder.clear();
                }
            });
            ui.horizontal(|ui| {
                ui.label(mono("quick timers:", DIM));
                for mins in [5i64, 10, 15, 30, 60] {
                    if ghost(ui, &format!("{mins}m")).clicked() {
                        self.add_reminder("Timer finished".into(), Local::now() + chrono::Duration::minutes(mins), String::new());
                    }
                }
            });
            let upcoming: Vec<_> = self.data.reminders.iter().filter(|r| !r.fired).cloned().collect();
            for r in upcoming {
                ui.horizontal(|ui| {
                    let when = chrono::DateTime::parse_from_rfc3339(&r.due)
                        .map(|d| d.with_timezone(&Local).format("%a %H:%M").to_string())
                        .unwrap_or_default();
                    ui.label(mono(&when, AMBER));
                    ui.label(&r.text);
                    if !r.repeat.is_empty() {
                        pill(ui, &r.repeat, CYAN);
                    }
                    if ghost(ui, "x").clicked() {
                        let id = r.id;
                        self.change_data(|d| d.reminders.retain(|x| x.id != id));
                    }
                });
            }
        });
    }

    fn add_reminder(&mut self, text: String, due: chrono::DateTime<Local>, repeat: String) {
        let due = due.to_rfc3339();
        self.change_data(|d| {
            let id = d.new_id();
            d.reminders.push(data::Reminder { id, text, due, fired: false, repeat });
        });
        self.say("Reminder set");
    }

    fn sched_focus(&mut self, ui: &mut Ui) {
        section(ui, "05", "focus timer (pomodoro)", |ui| {
            let sessions = self.data.pomo_sessions.get(&Local::now().format("%Y-%m-%d").to_string()).copied().unwrap_or(0);
            match self.countdown() {
                Some((phase, secs)) => {
                    ui.label(RichText::new(fmt_countdown(secs)).monospace().size(40.0).color(if phase == "work" { AMBER } else { GREEN }));
                    ui.label(mono(&format!("{phase}  |  {}", self.data.pomodoro.task), DIM));
                    if action(ui, "stop", RED).clicked() {
                        self.change_data(|d| d.pomodoro.running = false);
                    }
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
                        self.toggle_focus();
                    }
                }
            }
            ui.label(mono(&format!("sessions today: {sessions}  (a stamp card prints after each one)"), DIM));
        });
    }

    fn sched_eod(&mut self, ui: &mut Ui) {
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
                self.run_cmd(Cmd::EndOfDay);
            }
        });
    }

    fn sched_planner(&mut self, ui: &mut Ui) {
        section(ui, "07", "week planner", |ui| {
            ui.label(mono("monday to sunday with your events and room to write, plus your open to-dos", DIM));
            ui.horizontal(|ui| {
                if action(ui, "this week", AMBER).clicked() {
                    self.job("Printed the week planner", |s| s.print_planner(0));
                }
                if action(ui, "next week", CYAN).clicked() {
                    self.job("Printed next week's planner", |s| s.print_planner(1));
                }
            });
        });
    }

    // -------------------------------------------------------------- Tasks

    fn page_tasks(&mut self, ui: &mut Ui) {
        let mut tab = self.tab();
        page_title(ui, &format!("~/tasks/{}", TASK_TABS[tab]), "// to-dos, habits and countdowns");
        tab_bar(ui, &mut tab, &TASK_TABS);
        self.tabs[Page::Tasks.idx()] = tab;
        match tab {
            0 => self.tasks_todo(ui),
            1 => self.tasks_habits(ui),
            _ => self.tasks_countdowns(ui),
        }
    }

    fn tasks_todo(&mut self, ui: &mut Ui) {
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
                    resp.request_focus();
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
                    self.run_cmd(Cmd::Todos);
                }
                if ghost(ui, "clear finished").clicked() {
                    self.change_data(|d| d.todos.retain(|t| !t.done));
                }
            });
        });
    }

    fn tasks_habits(&mut self, ui: &mut Ui) {
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
                            let today = Local::now().date_naive();
                            s.print(escpos::habit_card(&s.cfg(), &hh, today, data::streak(&hh, today)))
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

    fn tasks_countdowns(&mut self, ui: &mut Ui) {
        section(ui, "03", "countdowns", |ui| {
            ui.label(mono("days left until a trip, a birthday, a deadline. they show up in your morning briefing.", DIM));
            ui.horizontal_wrapped(|ui| {
                text_field(ui, &mut self.cd_name, "Trip to Cairo", 220.0);
                ui.add(egui::DragValue::new(&mut self.cd_date[0]).range(2024..=2100));
                ui.label("-");
                ui.add(egui::DragValue::new(&mut self.cd_date[1]).range(1..=12));
                ui.label("-");
                ui.add(egui::DragValue::new(&mut self.cd_date[2]).range(1..=31));
                if action(ui, "add", AMBER).clicked() && !self.cd_name.trim().is_empty() {
                    if let Some(date) = chrono::NaiveDate::from_ymd_opt(self.cd_date[0], self.cd_date[1] as u32, self.cd_date[2] as u32) {
                        let name = self.cd_name.trim().to_string();
                        let date = date.format("%Y-%m-%d").to_string();
                        self.change_data(|d| {
                            let id = d.new_id();
                            d.countdowns.push(data::Countdown { id, name, date });
                        });
                        self.cd_name.clear();
                    } else {
                        self.say("That date doesn't exist.");
                    }
                }
            });
            let list = self.data.countdowns.clone();
            let today = Local::now().date_naive();
            for c in list {
                let days = chrono::NaiveDate::parse_from_str(&c.date, "%Y-%m-%d").map(|d| (d - today).num_days()).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.name).strong());
                    ui.label(mono(&c.date, DIM));
                    let text = match days {
                        0 => "TODAY".to_string(),
                        d if d < 0 => format!("{} d ago", -d),
                        d => format!("{d} days"),
                    };
                    ui.label(RichText::new(text).strong().color(if days >= 0 { AMBER } else { DIM }));
                    if ghost(ui, "x").clicked() {
                        let id = c.id;
                        self.change_data(|d| d.countdowns.retain(|x| x.id != id));
                    }
                });
            }
            if action(ui, "print countdowns", PINK).clicked() {
                self.run_cmd(Cmd::Countdowns);
            }
        });
    }

    // ------------------------------------------------------------- Alerts

    fn page_alerts(&mut self, ui: &mut Ui) {
        let mut tab = self.tab();
        page_title(ui, &format!("~/alerts/{}", ALERT_TABS[tab].replace(' ', "-")), "// pip taps you on the shoulder, on paper");
        tab_bar(ui, &mut tab, &ALERT_TABS);
        self.tabs[Page::Alerts.idx()] = tab;
        match tab {
            0 => section(ui, "01", "downloads", |ui| {
                toggle(ui, &mut self.cfg.alert_downloads, "print a slip when a download finishes");
                ui.horizontal(|ui| {
                    ui.label("folder");
                    text_field(ui, &mut self.cfg.download_dir, "empty = your Downloads folder", 380.0);
                });
            }),
            1 => section(ui, "02", "battery and disk", |ui| {
                ui.horizontal(|ui| {
                    toggle(ui, &mut self.cfg.alert_battery, "battery drops to");
                    ui.add(egui::DragValue::new(&mut self.cfg.battery_pct).range(5..=90).suffix("%"));
                });
                ui.horizontal(|ui| {
                    toggle(ui, &mut self.cfg.alert_disk, "drive C: has less than");
                    ui.add(egui::DragValue::new(&mut self.cfg.disk_free_pct).range(1..=50).suffix("% free"));
                });
            }),
            2 => section(ui, "03", "programs that finish", |ui| {
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
            }),
            3 => section(ui, "04", "pages that change", |ui| {
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
            }),
            _ => section(ui, "05", "mail slips", |ui| {
                ui.label(mono("prints sender and subject for new mail with a label you choose. read-only.", DIM));
                ui.horizontal(|ui| {
                    toggle(ui, &mut self.cfg.mail, "print new mail labelled");
                    text_field(ui, &mut self.cfg.mail_label, "IMPORTANT", 140.0);
                });
                ui.label(mono("labels: IMPORTANT, STARRED, INBOX or a label id", DIM));
                if self.cfg.mail && !self.cfg.google_has_mail {
                    ui.colored_label(AMBER, "Gmail access isn't granted yet. Use the google sign-in tab and tick \"also allow reading Gmail\".");
                    if action(ui, "open google sign-in", AMBER).clicked() {
                        self.connect_open = true;
                        self.connect_tab = 1;
                    }
                }
            }),
        }
    }

    // ----------------------------------------------------------- Settings

    fn page_settings(&mut self, ui: &mut Ui) {
        let mut tab = self.tab();
        page_title(ui, &format!("~/settings/{}", SET_TABS[tab]), "// printer, pip and calendar");
        tab_bar(ui, &mut tab, &SET_TABS);
        self.tabs[Page::Settings.idx()] = tab;
        match tab {
            0 => self.set_printer(ui),
            1 => section(ui, "02", "system", |ui| {
                ui.horizontal(|ui| {
                    toggle(ui, &mut self.cfg.start_with_windows, "start Thermo Companion with Windows");
                    pill(
                        ui,
                        if self.cfg.start_with_windows { "enabled" } else { "off" },
                        if self.cfg.start_with_windows { GREEN } else { DIM },
                    );
                });
                ui.label(mono("optional: runs Pip quietly in your tray when you sign in to Windows", DIM));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let prev = self.context_menu;
                    toggle(ui, &mut self.context_menu, "add \"Thermo Print\" to file right-click menu");
                    pill(
                        ui,
                        if self.context_menu { "enabled" } else { "off" },
                        if self.context_menu { GREEN } else { DIM },
                    );
                    if self.context_menu != prev {
                        if let Err(e) = autostart::set_context_menu(self.context_menu) {
                            self.context_menu = prev;
                            self.say(format!("Couldn't update right-click menu: {e}"));
                        } else {
                            self.say(if self.context_menu {
                                "Added \"Thermo Print\" to Windows file right-click menu."
                            } else {
                                "Removed \"Thermo Print\" from right-click menu."
                            });
                        }
                    }
                });
                ui.label(mono("right-click any image or text file in Explorer (Show more options on Win 11) -> Thermo Print", DIM));
                ui.add_space(4.0);
                let hk_label = self.cfg.hotkey_label();
                toggle(ui, &mut self.cfg.hotkey, &format!("{hk_label} prints the selected text from any app"));
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(22.0);
                    let mut mod_toggle = |bit: u32, name: &str| {
                        let mut on = self.cfg.hotkey_mods & bit != 0;
                        if ui.checkbox(&mut on, name).changed() {
                            let next = if on { self.cfg.hotkey_mods | bit } else { self.cfg.hotkey_mods & !bit };
                            if next == 0 && !config::is_function_vk(self.cfg.hotkey_vk) {
                                self.hotkey_note = "Keep at least one modifier key (or pick F1-F24 / Insert).".into();
                            } else {
                                self.cfg.hotkey_mods = next;
                                self.hotkey_note.clear();
                            }
                        }
                    };
                    mod_toggle(config::HOTKEY_MOD_CTRL, "Ctrl");
                    mod_toggle(config::HOTKEY_MOD_ALT, "Alt");
                    mod_toggle(config::HOTKEY_MOD_SHIFT, "Shift");
                    mod_toggle(config::HOTKEY_MOD_WIN, "Win");
                    pill(ui, &config::vk_label(self.cfg.hotkey_vk), CYAN);
                    let rec_label = if self.hotkey_listen { "press any key... (Esc cancels)" } else { "set key" };
                    if ui.add(
                        egui::Button::new(RichText::new(rec_label).monospace().color(INK).strong())
                            .fill(if self.hotkey_listen { AMBER } else { CYAN })
                            .corner_radius(CornerRadius::same(8)),
                    ).clicked() {
                        self.hotkey_listen = !self.hotkey_listen;
                        self.hotkey_note.clear();
                    }
                    if ghost(ui, "reset Ctrl+Alt+P").clicked() {
                        self.hotkey_listen = false;
                        self.cfg.hotkey_mods = config::HOTKEY_MOD_CTRL | config::HOTKEY_MOD_ALT;
                        self.cfg.hotkey_vk = b'P' as u32;
                        self.hotkey_note.clear();
                    }
                });
                if !self.hotkey_note.is_empty() {
                    ui.label(mono(&self.hotkey_note, AMBER));
                }
                ui.add_space(4.0);
                toggle(ui, &mut self.cfg.setup_dismissed, "hide the getting-started list on Home");
            }),
            2 => self.set_calendar(ui),
            _ => {
                section(ui, "04", "about", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::from_texture(self.tex.get(Mood::Happy)).fit_to_exact_size(egui::vec2(72.0, 72.0)));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("Thermo Companion v{}", env!("CARGO_PKG_VERSION"))).strong());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(mono("pip says hi. made by", DIM));
                                ui.hyperlink_to(mono("miketsak.gr", CYAN), "https://miketsak.gr");
                                ui.label(mono("with rust. mit license.", DIM));
                            });
                            ui.label(mono(&format!("settings: {}", config::dir().display()), DIM));
                            ui.label(mono(&format!("shortcuts: ctrl+k search, ctrl+1..7 pages, {} print selection", self.cfg.hotkey_label().to_lowercase()), DIM));
                        });
                    });
                });
                self.set_updates(ui);
            }
        }
    }

    fn set_printer(&mut self, ui: &mut Ui) {
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
                if text_field(ui, &mut cp, "blank", 60.0).changed() {
                    self.cfg.codepage = cp.trim().parse::<u8>().ok();
                }
                ui.label(mono("advanced", DIM));
            });
            toggle(ui, &mut self.cfg.pip_stickers, "add a tiny Pip sticker to the bottom of every slip");
            if action(ui, "test print", AMBER).clicked() {
                let cfg = self.cfg.clone();
                self.job("Test slip printed", move |s| {
                    printer::print_raw(&cfg.printer, &escpos::test_slip(&cfg))?;
                    s.mark_tested();
                    Ok(())
                });
            }
        });
    }

    fn set_calendar(&mut self, ui: &mut Ui) {
        section(ui, "03", "calendar and google", |ui| {
            ui.horizontal_wrapped(|ui| {
                pill(
                    ui,
                    if !self.cfg.ical_url.is_empty() {
                        "calendar link active"
                    } else if !self.cfg.google_refresh_token.is_empty() {
                        "google sign-in active"
                    } else {
                        "no calendar yet"
                    },
                    if self.cfg.calendar_linked() { GREEN } else { DIM },
                );
                if self.cfg.google_has_mail {
                    pill(ui, "gmail ok", CYAN);
                }
            });
            ui.label("The easy way is a private calendar link. Google sign-in is only needed for mail slips.");
            ui.horizontal(|ui| {
                if action(ui, "connect calendar", AMBER).clicked() {
                    self.connect_open = true;
                    self.connect_tab = 0;
                }
                if ghost(ui, "google sign-in (mail)").clicked() {
                    self.connect_open = true;
                    self.connect_tab = 1;
                }
            });
        });
    }

    fn set_updates(&mut self, ui: &mut Ui) {
        section(ui, "05", "updates", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&format!("installed: v{}  | ", env!("CARGO_PKG_VERSION")), DIM));
                ui.hyperlink_to(mono("made by miketsak.gr", CYAN), "https://miketsak.gr");
                ui.label(mono(&format!(" |  github: {}", update::REPO), DIM));
            });
            toggle(ui, &mut self.cfg.auto_check_updates, "automatically check GitHub for updates when dashboard opens");

            let busy = self.upd_busy || self.upd_installing;
            let btn_label = if self.upd_busy {
                "checking github..."
            } else if self.upd_installing {
                "downloading update..."
            } else {
                "check for updates"
            };
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(RichText::new(btn_label).monospace().color(INK).strong())
                            .fill(AMBER)
                            .corner_radius(CornerRadius::same(8)),
                    )
                    .clicked()
                {
                    self.start_update_check();
                }
                if ghost(ui, "releases page").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(update::RELEASES_URL));
                }
            });

            let mut install_rel: Option<update::Release> = None;
            let mut open_url: Option<String> = None;
            match &self.upd_state {
                Some(Ok(update::CheckOutcome::UpToDate { latest, message })) => {
                    ui.horizontal_wrapped(|ui| {
                        pill(ui, "up to date", GREEN);
                        if let Some(tag) = latest {
                            ui.label(mono(&format!("latest: {tag}"), DIM));
                        }
                    });
                    ui.label(message);
                }
                Some(Ok(update::CheckOutcome::Available(rel))) => {
                    ui.horizontal_wrapped(|ui| {
                        pill(ui, &format!("new: {}", rel.tag), AMBER);
                        ui.label(RichText::new(&rel.title).strong());
                    });
                    if !rel.notes.is_empty() {
                        let preview: String = rel.notes.lines().take(6).collect::<Vec<_>>().join("\n");
                        egui::Frame::new()
                            .fill(Color32::from_rgb(7, 10, 15))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(mono(&preview, TEXT));
                            });
                    }
                    ui.horizontal(|ui| {
                        let has_asset = rel.setup_asset.is_some() || rel.exe_asset.is_some();
                        if has_asset
                            && ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(
                                        RichText::new(format!("update to {} & restart", rel.tag))
                                            .monospace()
                                            .color(INK)
                                            .strong(),
                                    )
                                    .fill(GREEN)
                                    .corner_radius(CornerRadius::same(8)),
                                )
                                .clicked()
                        {
                            install_rel = Some(rel.clone());
                        }
                        if ghost(ui, "view release notes").clicked() {
                            open_url = Some(rel.html_url.clone());
                        }
                    });
                }
                Some(Err(e)) => {
                    ui.horizontal(|ui| {
                        pill(ui, "error", RED);
                        ui.label(RichText::new(e).color(RED));
                    });
                }
                None => {}
            }
            if !self.upd_err.is_empty() {
                ui.label(RichText::new(&self.upd_err).color(RED));
            }
            if let Some(url) = open_url {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            if let Some(rel) = install_rel {
                self.start_update_install(rel);
            }
        });
    }
}

impl eframe::App for Dash {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut before = self.cfg.clone();
        if self.sh.reload_if_changed() {
            self.cfg = self.sh.cfg();
            before = self.cfg.clone();
        }
        self.tick(&ctx);

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
                        Page::Notes => self.page_notes(ui),
                        Page::Print => self.page_print(ui),
                        Page::Schedule => self.page_schedule(ui),
                        Page::Tasks => self.page_tasks(ui),
                        Page::Alerts => self.page_alerts(ui),
                        Page::Settings => self.page_settings(ui),
                    }
                    ui.add_space(24.0);
                });
            });

        // popups on top
        self.connect_window(&ctx);
        self.palette(&ctx);

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
            .with_inner_size([1100.0, 740.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title(WINDOW_TITLE)
            .with_drag_and_drop(true)
            .with_icon(egui::IconData { rgba: icon.into_raw(), width: w, height: h }),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(Dash::new(cc, shared, page))))) {
        config::log(&format!("dashboard failed to start: {e}"));
    }
}
