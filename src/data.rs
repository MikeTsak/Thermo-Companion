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
pub struct Data {
    pub next_id: u64,
    pub todos: Vec<Todo>,
    pub habits: Vec<Habit>,
    pub reminders: Vec<Reminder>,
    pub pomodoro: Pomodoro,
    /// Date -> finished focus sessions.
    pub pomo_sessions: BTreeMap<String, u32>,
    pub usage: BTreeMap<String, DayUsage>,
    pub mail_seen: Vec<String>,
    /// False until the first mail check has recorded what already exists.
    pub mail_ready: bool,
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
        let tmp = crate::config::dir().join("data.json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, path());
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
