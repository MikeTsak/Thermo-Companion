//! Shared state, print jobs and the background scheduler.
//! The tray process runs the scheduler; the dashboard process shares the same `Shared` type.

use crate::config::{self, Config};
use crate::data;
use crate::escpos::{self, DayStats, TextOpts};
use crate::gcal::{self, Event};
use crate::net;
use crate::pip::{self, Mood};
use crate::printer;
use crate::sysinfo;
use chrono::{DateTime, Datelike, Local, Timelike};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Called when there is something new to show (tray balloon / repaint).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

pub struct Shared {
    cfg: Mutex<Config>,
    cfg_mtime: Mutex<Option<SystemTime>>,
    pub events: Mutex<Vec<Event>>,
    pub last_fetch: Mutex<Option<Instant>>,
    messages: Mutex<Vec<String>>,
    stubs_done: Mutex<HashSet<String>>,
    /// Print jobs currently running (Pip looks busy while this is > 0).
    pub jobs: AtomicUsize,
}

fn parse_time(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Local))
}

impl Shared {
    pub fn new(cfg: Config) -> Arc<Shared> {
        Arc::new(Shared {
            cfg: Mutex::new(cfg),
            cfg_mtime: Mutex::new(config::mtime()),
            events: Mutex::new(Vec::new()),
            last_fetch: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            stubs_done: Mutex::new(HashSet::new()),
            jobs: AtomicUsize::new(0),
        })
    }

    pub fn cfg(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    pub fn set_cfg(&self, cfg: Config) {
        config::save(&cfg);
        *self.cfg_mtime.lock().unwrap() = config::mtime();
        *self.cfg.lock().unwrap() = cfg;
    }

    /// Pick up settings the other process (dashboard or tray) saved. True if anything changed.
    pub fn reload_if_changed(&self) -> bool {
        let now = config::mtime();
        let mut seen = self.cfg_mtime.lock().unwrap();
        if now.is_some() && *seen != now {
            *seen = now;
            *self.cfg.lock().unwrap() = config::load().0;
            return true;
        }
        false
    }

    pub fn push_msg(&self, msg: impl Into<String>) {
        let msg = msg.into();
        config::log(&msg);
        self.messages.lock().unwrap().push(msg);
    }

    pub fn take_msgs(&self) -> Vec<String> {
        std::mem::take(&mut *self.messages.lock().unwrap())
    }

    /// A calendar is linked (iCal link or Google sign-in).
    pub fn connected(&self) -> bool {
        self.cfg().calendar_linked()
    }

    pub fn print(&self, data: Vec<u8>) -> Result<(), String> {
        self.print_as("Slip", data)
    }

    pub fn print_as(&self, label: &str, data: Vec<u8>) -> Result<(), String> {
        let printer_name = self.cfg().printer;
        match printer::print_raw(&printer_name, &data) {
            Ok(()) => {
                record_history(label, &data);
                Ok(())
            }
            Err(e) => {
                if !printer::list_printers().iter().any(|p| p == &printer_name)
                    || e.contains("Can't open printer")
                {
                    enqueue_print(label, &data);
                    Err(format!("Printer offline — queued \"{label}\" to print when back online."))
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn mark_tested(&self) {
        let mut c = self.cfg();
        if !c.did_test {
            c.did_test = true;
            self.set_cfg(c);
        }
    }

    pub fn refresh_events(&self) -> Result<(), String> {
        let cfg = self.cfg();
        let events = if !cfg.ical_url.trim().is_empty() {
            crate::ical::fetch(&cfg.ical_url, 14)?
        } else if !cfg.google_refresh_token.is_empty() {
            gcal::fetch(&cfg, 14)?
        } else {
            return Err("No calendar is linked yet (Settings > Calendar).".into());
        };
        *self.events.lock().unwrap() = events;
        *self.last_fetch.lock().unwrap() = Some(Instant::now());
        Ok(())
    }

    fn ensure_events(&self, refresh: bool) -> Result<(), String> {
        if refresh || self.last_fetch.lock().unwrap().is_none() {
            self.refresh_events()?;
        }
        Ok(())
    }

    /// Cached events that fall in the next `days` days (starting today).
    pub fn events_for_days(&self, days: u32) -> Vec<Event> {
        let today = Local::now().date_naive();
        let end = today + chrono::Duration::days(days.max(1) as i64);
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| {
                let d = e.start.date_naive();
                d >= today && d < end
            })
            .cloned()
            .collect()
    }

    pub fn print_agenda(&self, refresh: bool) -> Result<(), String> {
        self.ensure_events(refresh)?;
        let cfg = self.cfg();
        self.print(escpos::agenda(&cfg, &self.events_for_days(cfg.agenda_days)))
    }

    fn weather(&self, cfg: &Config) -> Option<net::Weather> {
        let (lat, lon, place) = if cfg.lat != 0.0 || cfg.lon != 0.0 {
            (cfg.lat, cfg.lon, cfg.city.clone())
        } else if !cfg.city.is_empty() {
            match net::geocode(&cfg.city) {
                Ok((lat, lon, name)) => {
                    let mut c = self.cfg();
                    c.lat = lat;
                    c.lon = lon;
                    self.set_cfg(c);
                    (lat, lon, name)
                }
                Err(e) => {
                    config::log(&format!("weather: {e}"));
                    return None;
                }
            }
        } else {
            return None;
        };
        net::weather(lat, lon, &place)
            .map_err(|e| config::log(&format!("weather: {e}")))
            .ok()
    }

    pub fn print_briefing(&self) -> Result<(), String> {
        let cfg = self.cfg();
        let events = if self.connected() {
            self.ensure_events(true)?;
            self.events_for_days(1)
        } else {
            Vec::new()
        };
        let weather = self.weather(&cfg);
        let d = data::load();
        let quote = pip::slip_line(Local::now().timestamp() as u64 / 86400);
        let counts: Vec<(String, i64)> = countdown_list(&d).into_iter().filter(|c| c.1 >= 0).take(3).collect();
        self.print(escpos::briefing(&cfg, &events, weather.as_ref(), &d.todos, &counts, quote))
    }

    /// The week (Monday first) that is `offset` weeks from this one.
    pub fn print_planner(&self, offset: i64) -> Result<(), String> {
        if self.connected() {
            let _ = self.ensure_events(true);
        }
        let today = Local::now().date_naive();
        let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64)
            + chrono::Duration::days(7 * offset);
        let end = monday + chrono::Duration::days(7);
        let events: Vec<Event> = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.start.date_naive() >= monday && e.start.date_naive() < end)
            .cloned()
            .collect();
        self.print(escpos::week_planner(&self.cfg(), monday, &events, &data::load().todos))
    }

    pub fn print_countdowns(&self) -> Result<(), String> {
        self.print(escpos::countdown_slip(&self.cfg(), &countdown_list(&data::load())))
    }

    pub fn print_end_of_day(&self) -> Result<(), String> {
        let cfg = self.cfg();
        if self.connected() {
            let _ = self.ensure_events(true);
        }
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let d = data::load();
        let meetings = self
            .events_for_days(1)
            .iter()
            .filter(|e| !e.all_day && e.start < now)
            .count();
        let stats = DayStats {
            meetings,
            todos_done: d.todos.iter().filter(|t| t.done).count(),
            focus_sessions: d.pomo_sessions.get(&today).copied().unwrap_or(0),
            habits_kept: d.habits.iter().filter(|h| h.days.contains(&today)).count(),
            usage: if cfg.track_usage { d.usage.get(&today).cloned() } else { None },
        };
        self.print(escpos::end_of_day(&cfg, &stats))
    }

    pub fn print_alert(&self, title: &str, body: &str, mood: Mood) -> Result<(), String> {
        self.print(escpos::alert(&self.cfg(), title, body, mood))
    }
}

/// Countdowns as (name, days left), nearest first.
pub fn countdown_list(d: &data::Data) -> Vec<(String, i64)> {
    let today = Local::now().date_naive();
    let mut v: Vec<(String, i64)> = d
        .countdowns
        .iter()
        .filter_map(|c| {
            let date = chrono::NaiveDate::parse_from_str(&c.date, "%Y-%m-%d").ok()?;
            Some((c.name.clone(), (date - today).num_days()))
        })
        .collect();
    v.sort_by_key(|x| (x.1 < 0, x.1.abs()));
    v
}

/// Run `job` on a worker thread, then show `ok` (or the error) and ping `notify`.
pub fn spawn_job<F>(shared: &Arc<Shared>, notify: Notify, ok: &'static str, job: F)
where
    F: FnOnce(&Shared) -> Result<(), String> + Send + 'static,
{
    let sh = shared.clone();
    sh.jobs.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let result = job(&sh);
        sh.jobs.fetch_sub(1, Ordering::SeqCst);
        sh.push_msg(match result {
            Ok(()) => ok.to_string(),
            Err(e) => format!("Failed: {e}"),
        });
        notify();
    });
}

fn parse_url_shortcut(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| {
            let (k, v) = line.split_once('=')?;
            if k.trim().eq_ignore_ascii_case("URL") && !v.trim().is_empty() {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
}

fn format_delimited_table(text: &str, sep: char) -> String {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let cap = 60usize;
    for line in lines.iter().take(cap) {
        let cols: Vec<&str> = line.split(sep).map(|c| c.trim().trim_matches('"')).collect();
        out.push(cols.join(" | "));
    }
    if lines.len() > cap {
        out.push(format!("... ({} more rows)", lines.len() - cap));
    }
    out.join("\n")
}

/// Build an ESC/POS slip for a right-clicked file or folder (image, .url QR, .ics agenda, .csv, or text).
pub fn build_file_slip(cfg: &Config, path: &std::path::Path) -> Result<Vec<u8>, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let opts = TextOpts { bold: false, big: false, center: false, stamp: true, checklist: false };

    if path.is_dir() {
        let mut entries: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| format!("can't open folder {}: {e}", path.display()))?
            .filter_map(|e| e.ok())
            .map(|e| {
                let mut s = e.file_name().to_string_lossy().to_string();
                if e.file_type().map_or(false, |t| t.is_dir()) {
                    s.push('/');
                }
                s
            })
            .collect();
        entries.sort_by_key(|s| s.to_ascii_lowercase());
        if entries.is_empty() {
            return Err("That folder is empty.".into());
        }
        let total = entries.len();
        let cap = 80usize;
        let mut body = entries.into_iter().take(cap).collect::<Vec<_>>().join("\n");
        if total > cap {
            body.push_str(&format!("\n... ({} more items)", total - cap));
        }
        return Ok(escpos::titled_text_slip(cfg, Some(&format!("{name}/ ({total})")), &body, &opts));
    }

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ["png", "jpg", "jpeg", "bmp", "gif", "webp"].contains(&ext.as_str()) {
        let dots = (cfg.paper_chars.max(16) as u32) * 12;
        let bm = crate::raster::from_file(&path.to_string_lossy(), dots, 0.0, true)?;
        return Ok(escpos::image_slip(cfg, &bm, &name));
    }

    let meta = std::fs::metadata(path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err("Only files or folders can be printed with Thermo Print.".into());
    }

    // Read at most 128 KB so large logs/files truncate gracefully instead of overflowing paper.
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    f.take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let byte_truncated = bytes.len() > 128 * 1024;
    if byte_truncated {
        bytes.truncate(128 * 1024);
    }

    if bytes.contains(&0) {
        return Err("That looks like a binary file. Choose a text file, folder, link (.url), calendar (.ics), or image.".into());
    }
    let raw_text = String::from_utf8_lossy(&bytes);
    if raw_text.trim().is_empty() {
        return Err("That file is empty.".into());
    }

    if ext == "url" {
        if let Some(url) = parse_url_shortcut(&raw_text) {
            let heading = name.strip_suffix(".url").or(Some(&name)).unwrap_or("LINK");
            return escpos::qr_slip(cfg, heading, &url, &[url.clone()]);
        }
    }

    if ext == "ics" {
        let now = Local::now();
        let events = crate::ical::parse(&raw_text, now - chrono::Duration::days(1), now + chrono::Duration::days(30));
        return Ok(escpos::agenda(cfg, &events));
    }

    if ext == "csv" || ext == "tsv" {
        let sep = if ext == "tsv" { '\t' } else { ',' };
        let body = format_delimited_table(&raw_text, sep);
        return Ok(escpos::titled_text_slip(cfg, Some(&name), &body, &opts));
    }

    let lines: Vec<&str> = raw_text.lines().collect();
    let line_cap = 120usize;
    let mut body = lines.iter().take(line_cap).copied().collect::<Vec<_>>().join("\n");
    if lines.len() > line_cap {
        body.push_str(&format!("\n... ({} more lines)", lines.len() - line_cap));
    } else if byte_truncated {
        body.push_str("\n... (truncated at 128 KB)");
    }

    let has_checks = (ext == "md" || ext == "txt")
        && lines.iter().take(30).any(|l| {
            let t = l.trim_start();
            t.starts_with("- [ ]") || t.starts_with("- [x]") || t.starts_with("- [X]")
        });
    let file_opts = TextOpts { checklist: has_checks, ..opts };
    Ok(escpos::titled_text_slip(cfg, Some(&name), &body, &file_opts))
}

pub fn record_history(label: &str, data: &[u8]) {
    if data.is_empty() || data.len() > 64 * 1024 {
        return;
    }
    let time = Local::now().format("%H:%M").to_string();
    let hex = data::hex_encode(data);
    data::update(|d| {
        let id = d.new_id();
        d.print_history.insert(0, data::PrintJob { id, label: label.to_string(), time, hex });
        d.print_history.truncate(8);
    });
}

pub fn enqueue_print(label: &str, data: &[u8]) {
    if data.is_empty() || data.len() > 256 * 1024 {
        return;
    }
    let time = Local::now().format("%H:%M").to_string();
    let hex = data::hex_encode(data);
    data::update(|d| {
        let id = d.new_id();
        d.print_queue.push(data::PrintJob { id, label: label.to_string(), time, hex });
        if d.print_queue.len() > 20 {
            let excess = d.print_queue.len() - 20;
            d.print_queue.drain(..excess);
        }
    });
}

pub fn flush_print_queue(sh: &Shared) -> Result<usize, String> {
    let cfg = sh.cfg();
    let queue = data::load().print_queue;
    if queue.is_empty() {
        return Ok(0);
    }
    if !printer::list_printers().iter().any(|p| p == &cfg.printer) {
        return Ok(0);
    }
    let mut printed = 0usize;
    for job in queue {
        let Some(bytes) = data::hex_decode(&job.hex) else {
            data::update(|d| d.print_queue.retain(|x| x.id != job.id));
            continue;
        };
        printer::print_raw(&cfg.printer, &bytes)?;
        record_history(&job.label, &bytes);
        data::update(|d| d.print_queue.retain(|x| x.id != job.id));
        printed += 1;
    }
    Ok(printed)
}

/// Returns `Ok(true)` if printed immediately, `Ok(false)` if queued while printer is offline.
pub fn print_file(cfg: &Config, path: &std::path::Path) -> Result<bool, String> {
    let slip = build_file_slip(cfg, path)?;
    let label = format!(
        "File: {}",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    );
    match printer::print_raw(&cfg.printer, &slip) {
        Ok(()) => {
            record_history(&label, &slip);
            Ok(true)
        }
        Err(e) => {
            if !printer::list_printers().iter().any(|p| p == &cfg.printer)
                || e.contains("Can't open printer")
            {
                enqueue_print(&label, &slip);
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// Global hotkey: copy what is selected (or fall back to the clipboard) and print it.
pub fn hotkey_print(sh: &Shared) -> Result<&'static str, String> {
    let cfg = sh.cfg();
    sysinfo::wait_hotkey_released(cfg.hotkey_vk, 1500);
    let seq_before = sysinfo::clipboard_seq();
    let before = sysinfo::clipboard_text_retry(5);
    sysinfo::send_copy();

    // Poll until the focused app updates the clipboard (Chromium/Keep can take 200-400ms via IPC).
    let mut copied = None;
    let deadline = Instant::now() + Duration::from_millis(650);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
        if sysinfo::clipboard_seq() != seq_before {
            if let Some(t) = sysinfo::clipboard_text_retry(8).filter(|t| !t.trim().is_empty()) {
                copied = Some(t);
                break;
            }
        }
    }

    let text = copied
        .or(before)
        .filter(|t| !t.trim().is_empty())
        .ok_or("nothing selected and the clipboard is empty")?;
    let opts = TextOpts { bold: false, big: false, center: false, stamp: true, checklist: false };
    sh.print_as("Selection", escpos::text_slip(&cfg, &text, &opts))?;
    Ok("Printed your selection")
}

// ---------------------------------------------------------------- scheduler

struct Sched {
    last_usage: Instant,
    last_sys: Instant,
    last_url: Option<Instant>,
    last_mail: Option<Instant>,
    errors: HashMap<&'static str, String>,
    dl_known: Option<HashSet<PathBuf>>,
    dl_pending: HashMap<PathBuf, (u64, Instant)>,
    batt_alerted: bool,
    disk_alerted: bool,
    urls: HashMap<String, u64>,
    procs: HashSet<String>,
}

fn say(sh: &Shared, notify: &Notify, msg: impl Into<String>) {
    sh.push_msg(msg);
    notify();
}

/// Report a failure once per distinct message, not every few seconds.
fn report(st: &mut Sched, sh: &Shared, notify: &Notify, key: &'static str, result: Result<(), String>) {
    match result {
        Ok(()) => {
            st.errors.remove(key);
        }
        Err(e) => {
            if st.errors.get(key) != Some(&e) {
                say(sh, notify, format!("{key} failed: {e}"));
                st.errors.insert(key, e);
            }
        }
    }
}

/// Background loop: calendar, scheduled slips, reminders, focus timer, alerts, mail.
pub fn scheduler(sh: Arc<Shared>, notify: Notify) {
    let mut st = Sched {
        last_usage: Instant::now(),
        last_sys: Instant::now() - Duration::from_secs(60),
        last_url: None,
        last_mail: None,
        errors: HashMap::new(),
        dl_known: None,
        dl_pending: HashMap::new(),
        batt_alerted: false,
        disk_alerted: false,
        urls: HashMap::new(),
        procs: HashSet::new(),
    };
    loop {
        if sh.reload_if_changed() {
            notify(); // lets the tray refresh its menu / hotkey
        }
        tick(&sh, &notify, &mut st);
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn tick(sh: &Arc<Shared>, notify: &Notify, st: &mut Sched) {
    let cfg = sh.cfg();
    let now = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let after = |h: u32, m: u32| (now.hour(), now.minute()) >= (h, m);

    // ---- calendar
    if cfg.calendar_linked() {
        let stale = sh
            .last_fetch
            .lock()
            .unwrap()
            .map_or(true, |t| t.elapsed() > Duration::from_secs(600));
        if stale {
            let r = sh.refresh_events();
            if r.is_err() {
                // retry in 10 minutes
                *sh.last_fetch.lock().unwrap() = Some(Instant::now());
            }
            report(st, sh, notify, "Calendar sync", r);
        }
    }
    let have_calendar = sh.last_fetch.lock().unwrap().is_some();

    // ---- daily agenda / briefing / end-of-day receipt
    if cfg.auto_agenda && cfg.last_auto_date != today && after(cfg.agenda_hour, cfg.agenda_minute) && have_calendar {
        let mut c = sh.cfg();
        c.last_auto_date = today.clone();
        sh.set_cfg(c);
        let r = sh.print_agenda(false);
        say(sh, notify, match &r {
            Ok(()) => "Printed today's agenda".to_string(),
            Err(e) => format!("Daily agenda failed: {e}"),
        });
    }
    if cfg.briefing && cfg.last_briefing_date != today && after(cfg.briefing_hour, cfg.briefing_minute) {
        let mut c = sh.cfg();
        c.last_briefing_date = today.clone();
        sh.set_cfg(c);
        let r = sh.print_briefing();
        say(sh, notify, match &r {
            Ok(()) => "Printed your morning briefing".to_string(),
            Err(e) => format!("Morning briefing failed: {e}"),
        });
    }
    if cfg.eod && cfg.last_eod_date != today && after(cfg.eod_hour, cfg.eod_minute) {
        let mut c = sh.cfg();
        c.last_eod_date = today.clone();
        sh.set_cfg(c);
        let r = sh.print_end_of_day();
        say(sh, notify, match &r {
            Ok(()) => "Printed your end-of-day receipt".to_string(),
            Err(e) => format!("End-of-day receipt failed: {e}"),
        });
    }

    // ---- event stubs
    if cfg.event_stubs {
        let events = sh.events.lock().unwrap().clone();
        for ev in events.iter().filter(|e| !e.all_day) {
            let minutes = (ev.start - now).num_minutes();
            if minutes < 0 || minutes > cfg.stub_minutes {
                continue;
            }
            if sh.stubs_done.lock().unwrap().insert(ev.id.clone()) {
                let r = sh.print(escpos::stub(&cfg, ev));
                if let Err(e) = r {
                    say(sh, notify, format!("Event stub failed: {e}"));
                }
            }
        }
    }

    // ---- reminders
    let d = data::load();
    let due: Vec<u64> = d
        .reminders
        .iter()
        .filter(|r| !r.fired && parse_time(&r.due).map_or(false, |t| t <= now))
        .map(|r| r.id)
        .collect();
    for id in due {
        if let Some(r) = d.reminders.iter().find(|r| r.id == id) {
            let res = sh.print(escpos::reminder(&cfg, &r.text));
            say(sh, notify, match &res {
                Ok(()) => format!("Reminder: {}", r.text),
                Err(e) => format!("Reminder failed: {e}"),
            });
        }
        data::update(|d| {
            if let Some(r) = d.reminders.iter_mut().find(|r| r.id == id) {
                match r.repeat.as_str() {
                    "daily" | "weekdays" | "weekly" => {
                        // schedule the next one (skipping weekends for "weekdays")
                        if let Some(mut due) = parse_time(&r.due) {
                            loop {
                                due += chrono::Duration::days(if r.repeat == "weekly" { 7 } else { 1 });
                                let weekend = matches!(due.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
                                if due > now && !(r.repeat == "weekdays" && weekend) {
                                    break;
                                }
                            }
                            r.due = due.to_rfc3339();
                        } else {
                            r.fired = true;
                        }
                    }
                    _ => r.fired = true,
                }
            }
            // forget reminders that fired more than a day ago
            let cutoff = now - chrono::Duration::days(1);
            d.reminders.retain(|r| !r.fired || parse_time(&r.due).map_or(true, |t| t > cutoff));
        });
    }

    // ---- focus timer
    let p = d.pomodoro.clone();
    if p.running && parse_time(&p.ends_at).map_or(false, |t| t <= now) {
        let finished_work = data::update(|d| {
            let next = |mins: u32| (now + chrono::Duration::minutes(mins as i64)).to_rfc3339();
            if d.pomodoro.phase == "break" {
                d.pomodoro.phase = "work".into();
                d.pomodoro.ends_at = next(cfg.pomo_work);
                None
            } else {
                let count = {
                    let c = d.pomo_sessions.entry(today.clone()).or_insert(0);
                    *c += 1;
                    *c
                };
                d.pomodoro.phase = "break".into();
                d.pomodoro.ends_at = next(cfg.pomo_break);
                Some((d.pomodoro.task.clone(), count))
            }
        });
        match finished_work {
            Some((task, count)) => {
                let res = sh.print(escpos::pomodoro_done(&cfg, &task, cfg.pomo_work, count));
                say(sh, notify, match &res {
                    Ok(()) => format!("Focus session {count} done! Take a {} minute break.", cfg.pomo_break),
                    Err(e) => format!("Focus slip failed: {e}"),
                });
            }
            None => say(sh, notify, "Break over. Back to it!"),
        }
    }

    // ---- what you do at the PC (opt-in)
    if st.last_usage.elapsed() >= Duration::from_secs(60) {
        st.last_usage = Instant::now();
        if cfg.track_usage && sysinfo::idle_seconds() < 120 {
            if let Some(app) = sysinfo::foreground_program() {
                data::update(|d| {
                    let day = d.usage.entry(today.clone()).or_default();
                    day.active_minutes += 1;
                    *day.apps.entry(app).or_insert(0) += 1;
                    // keep two months of history
                    while d.usage.len() > 60 {
                        let first = d.usage.keys().next().cloned();
                        if let Some(k) = first {
                            d.usage.remove(&k);
                        }
                    }
                });
            }
        }
    }

    // ---- PC alerts & offline print queue
    if st.last_sys.elapsed() >= Duration::from_secs(10) {
        st.last_sys = Instant::now();
        check_alerts(sh, notify, st, &cfg);
        if let Ok(n) = flush_print_queue(sh) {
            if n > 0 {
                say(sh, notify, format!("Printed {n} queued slip(s)"));
            }
        }
    }
    if !cfg.watch_urls.is_empty() && st.last_url.map_or(true, |t| t.elapsed() > Duration::from_secs(600)) {
        st.last_url = Some(Instant::now());
        for url in &cfg.watch_urls {
            match net::page_fingerprint(url) {
                Ok(fp) => {
                    if let Some(old) = st.urls.insert(url.clone(), fp) {
                        if old != fp {
                            let r = sh.print_alert("PAGE CHANGED", url, Mood::Wink);
                            say(sh, notify, match r {
                                Ok(()) => format!("Page changed: {url}"),
                                Err(e) => format!("Page alert failed: {e}"),
                            });
                        }
                    }
                }
                Err(e) => config::log(&format!("watching {url}: {e}")),
            }
        }
    }

    // ---- mail
    if cfg.mail
        && cfg.google_has_mail
        && !cfg.google_refresh_token.is_empty()
        && st.last_mail.map_or(true, |t| t.elapsed() > Duration::from_secs(300))
    {
        st.last_mail = Some(Instant::now());
        let r = check_mail(sh, notify, &cfg);
        report(st, sh, notify, "Mail check", r);
    }
}

fn check_alerts(sh: &Arc<Shared>, notify: &Notify, st: &mut Sched, cfg: &Config) {
    // battery
    if cfg.alert_battery {
        if let Some((pct, plugged)) = sysinfo::battery() {
            if !plugged && pct as u32 <= cfg.battery_pct && !st.batt_alerted {
                st.batt_alerted = true;
                let r = sh.print_alert("LOW BATTERY", &format!("Battery is at {pct}%. Plug me in!"), Mood::Sad);
                if let Err(e) = r {
                    say(sh, notify, format!("Battery alert failed: {e}"));
                }
            } else if plugged || pct as u32 > cfg.battery_pct + 5 {
                st.batt_alerted = false;
            }
        }
    }
    // disk
    if cfg.alert_disk {
        if let Some((pct, gb)) = sysinfo::disk_free("C:\\") {
            if pct <= cfg.disk_free_pct as f64 && !st.disk_alerted {
                st.disk_alerted = true;
                let r = sh.print_alert("DISK ALMOST FULL", &format!("Drive C: has {gb:.1} GB free ({pct:.0}%)."), Mood::Sad);
                if let Err(e) = r {
                    say(sh, notify, format!("Disk alert failed: {e}"));
                }
            } else if pct > cfg.disk_free_pct as f64 + 3.0 {
                st.disk_alerted = false;
            }
        }
    }
    // downloads
    if cfg.alert_downloads {
        check_downloads(sh, notify, st, cfg);
    } else {
        st.dl_known = None;
        st.dl_pending.clear();
    }
    // programs that finish
    if !cfg.watch_procs.is_empty() {
        let running = sysinfo::running_programs();
        let watched: HashSet<String> = cfg
            .watch_procs
            .iter()
            .map(|p| p.trim().to_lowercase().trim_end_matches(".exe").to_string())
            .filter(|p| !p.is_empty())
            .collect();
        for w in &watched {
            if st.procs.contains(w) && !running.contains(w) {
                let r = sh.print_alert("PROGRAM FINISHED", &format!("{w} has stopped running."), Mood::Party);
                say(sh, notify, match r {
                    Ok(()) => format!("{w} finished"),
                    Err(e) => format!("Program alert failed: {e}"),
                });
            }
        }
        st.procs = watched.into_iter().filter(|w| running.contains(w)).collect();
    } else {
        st.procs.clear();
    }
}

fn check_downloads(sh: &Arc<Shared>, notify: &Notify, st: &mut Sched, cfg: &Config) {
    let dir = if cfg.download_dir.trim().is_empty() {
        PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Downloads")
    } else {
        PathBuf::from(cfg.download_dir.trim())
    };
    let entries: Vec<(PathBuf, u64)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let md = e.metadata().ok()?;
                    md.is_file().then(|| (e.path(), md.len()))
                })
                .collect()
        })
        .unwrap_or_default();

    // First look: remember what is already there, only announce new files.
    if st.dl_known.is_none() {
        st.dl_known = Some(entries.iter().map(|e| e.0.clone()).collect());
        return;
    }
    let known = st.dl_known.as_mut().unwrap();
    let pending = &mut st.dl_pending;
    for (path, len) in entries {
        if known.contains(&path) {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let lower = name.to_lowercase();
        if [".crdownload", ".part", ".tmp", ".download", ".partial"].iter().any(|s| lower.ends_with(s)) {
            continue;
        }
        let entry = pending.entry(path.clone()).or_insert((len, Instant::now()));
        if entry.0 != len {
            *entry = (len, Instant::now());
        } else if entry.1.elapsed() >= Duration::from_secs(6) {
            known.insert(path.clone());
            pending.remove(&path);
            let size = if len > 1_000_000 {
                format!("{:.1} MB", len as f64 / 1e6)
            } else {
                format!("{} KB", len / 1000)
            };
            let r = sh.print_alert("DOWNLOAD DONE", &format!("{name} ({size})"), Mood::Party);
            say(sh, notify, match r {
                Ok(()) => format!("Download finished: {name}"),
                Err(e) => format!("Download alert failed: {e}"),
            });
        }
    }
}

fn check_mail(sh: &Arc<Shared>, notify: &Notify, cfg: &Config) -> Result<(), String> {
    let label = if cfg.mail_label.trim().is_empty() { "IMPORTANT" } else { cfg.mail_label.trim() };
    let mails = net::recent_mail(cfg, label)?;
    let known = data::load();
    let first_time = !known.mail_ready;
    let fresh: Vec<_> = mails.into_iter().filter(|m| !known.mail_seen.contains(&m.id)).collect();
    if first_time || !fresh.is_empty() {
        data::update(|d| {
            d.mail_ready = true;
            d.mail_seen.extend(fresh.iter().map(|m| m.id.clone()));
            let excess = d.mail_seen.len().saturating_sub(500);
            d.mail_seen.drain(..excess);
        });
    }
    if first_time {
        // Don't print the backlog the moment mail slips are switched on.
        say(sh, notify, "Mail slips are on. I'll print new mail from now on.");
        return Ok(());
    }
    if fresh.is_empty() {
        return Ok(());
    }
    let batch: Vec<_> = fresh.into_iter().take(5).collect();
    sh.print(escpos::mail_slip(cfg, &batch))?;
    say(sh, notify, format!("Printed {} new mail", batch.len()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_text_url_csv_md_dir_and_rejects_binary_file_slips() {
        let cfg = Config::default();
        let dir = std::env::temp_dir().join("thermo_test_files");
        let _ = std::fs::create_dir_all(&dir);
        let txt_path = dir.join("note.txt");
        let md_path = dir.join("tasks.md");
        let url_path = dir.join("site.url");
        let csv_path = dir.join("table.csv");
        let bin_path = dir.join("raw.bin");

        std::fs::write(&txt_path, "hello from right-click!").unwrap();
        assert!(build_file_slip(&cfg, &txt_path).unwrap().len() > 20);

        std::fs::write(&md_path, "- [ ] buy milk\n- [x] print").unwrap();
        let md_slip = build_file_slip(&cfg, &md_path).unwrap();
        assert!(md_slip.windows(3).any(|w| w == b"[x]"));

        std::fs::write(&url_path, "[InternetShortcut]\nURL=https://miketsak.gr\n").unwrap();
        assert!(build_file_slip(&cfg, &url_path).unwrap().len() > 40);

        std::fs::write(&csv_path, "name,qty\ncoffee,2\n").unwrap();
        let csv_slip = build_file_slip(&cfg, &csv_path).unwrap();
        assert!(csv_slip.windows(12).any(|w| w == b"coffee | 2\n") || csv_slip.windows(10).any(|w| w == b"coffee | 2"));

        assert!(build_file_slip(&cfg, &dir).unwrap().len() > 20);

        std::fs::write(&bin_path, b"\x00\x01\x02\x03").unwrap();
        assert!(build_file_slip(&cfg, &bin_path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        let raw = b"\x1b@\x00\xffhi";
        let hex = data::hex_encode(raw);
        assert_eq!(data::hex_decode(&hex).as_deref(), Some(raw.as_slice()));
    }
}
