//! Your stuff (to-dos, habits, reminders, focus timer, usage stats) in
//! %APPDATA%\ThermoCompanion\data.json. The tray app and the dashboard are two
//! processes that share this file, so every change goes through `update`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Todo {
    pub id: u64,
    pub text: String,
    pub done: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Habit {
    pub id: u64,
    pub name: String,
    /// Days (YYYY-MM-DD) on which the habit was ticked.
    pub days: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Reminder {
    pub id: u64,
    pub text: String,
    /// RFC 3339, local time zone.
    pub due: String,
    pub fired: bool,
    /// "" (once), "daily", "weekdays" or "weekly".
    pub repeat: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct NoteItem {
    pub text: String,
    pub done: bool,
}

/// A Keep-style note: text or a checklist, with a colour, pin, labels and archive.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub checklist: bool,
    pub items: Vec<NoteItem>,
    /// Index into the dashboard's colour list.
    pub color: u8,
    pub pinned: bool,
    pub archived: bool,
    pub labels: Vec<String>,
    /// RFC 3339.
    pub updated: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Countdown {
    pub id: u64,
    pub name: String,
    /// YYYY-MM-DD
    pub date: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Pomodoro {
    pub running: bool,
    /// "work" or "break"
    pub phase: String,
    /// RFC 3339.
    pub ends_at: String,
    pub task: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DayUsage {
    pub active_minutes: u32,
    /// Program name -> minutes in the foreground.
    pub apps: BTreeMap<String, u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PrintJob {
    pub id: u64,
    pub label: String,
    pub time: String,
    pub hex: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Data {
    pub next_id: u64,
    pub todos: Vec<Todo>,
    pub habits: Vec<Habit>,
    pub reminders: Vec<Reminder>,
    pub notes: Vec<Note>,
    pub countdowns: Vec<Countdown>,
    pub pomodoro: Pomodoro,
    /// Date -> finished focus sessions.
    pub pomo_sessions: BTreeMap<String, u32>,
    pub usage: BTreeMap<String, DayUsage>,
    pub mail_seen: Vec<String>,
    /// False until the first mail check has recorded what already exists.
    pub mail_ready: bool,
    pub print_history: Vec<PrintJob>,
    pub print_queue: Vec<PrintJob>,
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    for i in (0..b.len()).step_by(2) {
        out.push((val(b[i])? << 4) | val(b[i + 1])?);
    }
    Some(out)
}

impl Data {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

fn path() -> PathBuf {
    crate::config::dir().join("data.json")
}

pub fn load() -> Data {
    fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(d: &Data) {
    if let Ok(text) = serde_json::to_string_pretty(d) {
        let target = path();
        let tmp = crate::config::dir().join("data.json.tmp");
        if fs::write(&tmp, &text).is_ok() && fs::rename(&tmp, &target).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if fs::rename(&tmp, &target).is_err() {
                let _ = fs::write(&target, &text);
                let _ = fs::remove_file(&tmp);
            }
        }
    }
}

/// Load the latest file, apply `f`, save. Use this for every change.
pub fn update<R>(f: impl FnOnce(&mut Data) -> R) -> R {
    let mut d = load();
    let r = f(&mut d);
    save(&d);
    r
}

/// Consecutive days up to today (or yesterday) on which the habit was ticked.
pub fn streak(h: &Habit, today: chrono::NaiveDate) -> u32 {
    let has = |d: chrono::NaiveDate| h.days.iter().any(|x| *x == d.format("%Y-%m-%d").to_string());
    let mut day = today;
    if !has(day) {
        day = today - chrono::Duration::days(1);
    }
    let mut n = 0;
    while has(day) {
        n += 1;
        day -= chrono::Duration::days(1);
    }
    n
}
