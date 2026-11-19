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
    /// Hotkey modifiers bitmask (1=Ctrl, 2=Alt, 4=Shift, 8=Win).
    pub hotkey_mods: u32,
    /// Windows virtual-key code for the print-selection hotkey (default 'P' = 0x50).
    pub hotkey_vk: u32,

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

    // calendar: a private iCal link (easy) or Google sign-in (calendar + mail)
    pub ical_url: String,
    pub did_test: bool,
    pub setup_dismissed: bool,
    pub auto_check_updates: bool,

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
            start_with_windows: false,
            hotkey: true,
            hotkey_mods: HOTKEY_MOD_CTRL | HOTKEY_MOD_ALT,
            hotkey_vk: b'P' as u32,

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

            ical_url: String::new(),
            did_test: false,
            setup_dismissed: false,
            auto_check_updates: false,

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

pub const HOTKEY_MOD_CTRL: u32 = 1;
pub const HOTKEY_MOD_ALT: u32 = 2;
pub const HOTKEY_MOD_SHIFT: u32 = 4;
pub const HOTKEY_MOD_WIN: u32 = 8;

pub fn is_function_vk(vk: u32) -> bool {
    (0x70..=0x87).contains(&vk) || matches!(vk, 0x2D | 0x2C | 0x13)
}

pub fn vk_label(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x20 => "Space".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x08 => "Backspace".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "PgUp".into(),
        0x22 => "PgDn".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xDC => "\\".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0x2C => "PrtSc".into(),
        0x13 => "Pause".into(),
        other => format!("Key(0x{other:02X})"),
    }
}

impl Config {
    /// True when events can be read (iCal link or Google sign-in).
    pub fn calendar_linked(&self) -> bool {
        !self.ical_url.trim().is_empty() || !self.google_refresh_token.is_empty()
    }

    pub fn hotkey_label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.hotkey_mods & HOTKEY_MOD_CTRL != 0 {
            parts.push("Ctrl".into());
        }
        if self.hotkey_mods & HOTKEY_MOD_ALT != 0 {
            parts.push("Alt".into());
        }
        if self.hotkey_mods & HOTKEY_MOD_SHIFT != 0 {
            parts.push("Shift".into());
        }
        if self.hotkey_mods & HOTKEY_MOD_WIN != 0 {
            parts.push("Win".into());
        }
        let vk = if self.hotkey_vk == 0 { b'P' as u32 } else { self.hotkey_vk };
        parts.push(vk_label(vk));
        parts.join("+")
    }

    pub fn hotkey_win_mods(&self) -> u32 {
        use winapi::um::winuser::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN};
        let mut m = MOD_NOREPEAT as u32;
        if self.hotkey_mods & HOTKEY_MOD_CTRL != 0 {
            m |= MOD_CONTROL as u32;
        }
        if self.hotkey_mods & HOTKEY_MOD_ALT != 0 {
            m |= MOD_ALT as u32;
        }
        if self.hotkey_mods & HOTKEY_MOD_SHIFT != 0 {
            m |= MOD_SHIFT as u32;
        }
        if self.hotkey_mods & HOTKEY_MOD_WIN != 0 {
            m |= MOD_WIN as u32;
        }
        m
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
        let target = path();
        let tmp = dir().join("config.json.tmp");
        if let Err(e) = fs::write(&tmp, &text) {
            log(&format!("config save tmp failed: {e}"));
            return;
        }
        if fs::rename(&tmp, &target).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if fs::rename(&tmp, &target).is_err() {
                if let Err(e) = fs::write(&target, &text) {
                    log(&format!("config save failed: {e}"));
                }
                let _ = fs::remove_file(&tmp);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_label_and_defaults() {
        let mut c = Config::default();
        assert_eq!(c.hotkey_label(), "Ctrl+Alt+P");
        c.hotkey_mods = HOTKEY_MOD_CTRL | HOTKEY_MOD_SHIFT;
        c.hotkey_vk = 0x78; // F9
        assert_eq!(c.hotkey_label(), "Ctrl+Shift+F9");
        assert!(is_function_vk(0x78));
        assert!(!is_function_vk(b'P' as u32));
    }
}
