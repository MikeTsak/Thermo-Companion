//! Settings stored as JSON in %APPDATA%\ThermoCompanion\config.json

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    // printer
    pub printer: String,
    /// Characters per line (see the ruler on the test slip).
    pub paper_chars: usize,
    /// Print Greek as Latin letters ("Geia sou"). Safe on every printer.
    pub greeklish: bool,
    /// ESC t n code page number; None leaves the printer's default.
    pub codepage: Option<u8>,
    /// A tiny Pip sticker at the bottom of every slip.
    pub pip_stickers: bool,
    pub start_with_windows: bool,
    pub hotkey: bool,

    // agenda + briefing
    pub auto_agenda: bool,
    pub agenda_hour: u32,
    pub agenda_minute: u32,
    pub agenda_days: u32,
    pub show_locations: bool,
    pub event_stubs: bool,
    pub stub_minutes: i64,
    pub briefing: bool,
    pub briefing_hour: u32,
    pub briefing_minute: u32,
    pub city: String,
    pub lat: f64,
    pub lon: f64,

    // focus
    pub pomo_work: u32,
    pub pomo_break: u32,

    // end-of-day receipt
    pub track_usage: bool,
    pub eod: bool,
    pub eod_hour: u32,
    pub eod_minute: u32,

    // PC alerts
    pub alert_downloads: bool,
    pub download_dir: String,
    pub alert_battery: bool,
    pub battery_pct: u32,
    pub alert_disk: bool,
    pub disk_free_pct: u32,
    pub watch_urls: Vec<String>,
    pub watch_procs: Vec<String>,

    // mail
    pub mail: bool,
    pub mail_label: String,

    // google
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_refresh_token: String,
    /// True once the saved token was granted the Gmail scope.
    pub google_has_mail: bool,

    pub last_auto_date: String,
    pub last_briefing_date: String,
    pub last_eod_date: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            printer: "EPSON TM-T20III Receipt".into(),
            paper_chars: 42,
            greeklish: true,
            codepage: None,
            pip_stickers: true,
            start_with_windows: true,
            hotkey: true,

            auto_agenda: true,
            agenda_hour: 8,
            agenda_minute: 0,
            agenda_days: 1,
            show_locations: true,
            event_stubs: false,
            stub_minutes: 30,
            briefing: false,
            briefing_hour: 7,
            briefing_minute: 45,
            city: String::new(),
            lat: 0.0,
            lon: 0.0,

            pomo_work: 25,
            pomo_break: 5,

            track_usage: false,
            eod: false,
            eod_hour: 18,
            eod_minute: 0,

            alert_downloads: false,
            download_dir: String::new(),
            alert_battery: false,
            battery_pct: 20,
            alert_disk: false,
            disk_free_pct: 10,
            watch_urls: Vec::new(),
            watch_procs: Vec::new(),

            mail: false,
            mail_label: "IMPORTANT".into(),

            google_client_id: String::new(),
            google_client_secret: String::new(),
            google_refresh_token: String::new(),
            google_has_mail: false,

            last_auto_date: String::new(),
            last_briefing_date: String::new(),
            last_eod_date: String::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("ThermoCompanion");
    let _ = fs::create_dir_all(&d);
    d
}

fn path() -> PathBuf {
    dir().join("config.json")
}

/// When the settings file last changed (used to notice edits from the dashboard).
pub fn mtime() -> Option<SystemTime> {
    fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Returns the config and whether this is the first run (no file yet).
pub fn load() -> (Config, bool) {
    match fs::read_to_string(path()) {
        Ok(text) => (serde_json::from_str(&text).unwrap_or_default(), false),
        Err(_) => (Config::default(), true),
    }
}

pub fn save(cfg: &Config) {
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let tmp = dir().join("config.json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, path());
        }
    }
}

pub fn log(msg: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir().join("log.txt"))
    {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{now}  {msg}");
    }
}

/// Last `n` lines of the log, newest last.
pub fn log_tail(n: usize) -> Vec<String> {
    let text = fs::read_to_string(dir().join("log.txt")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect()
}
