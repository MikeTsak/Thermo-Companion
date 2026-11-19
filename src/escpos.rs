//! ESC/POS slip builders. Every function returns the raw bytes for one slip.

use crate::config::Config;
use crate::data::{DayUsage, Habit, Note, Todo};
use crate::gcal::Event;
use crate::net::{Mail, Weather};
use crate::pip::{self, Mood};
use crate::raster::{self, Bitmap};
use chrono::{Datelike, Local, NaiveDate};

const ESC: u8 = 0x1b;
const GS: u8 = 0x1d;
/// Width of the "  09:00   " column in the agenda.
const PREFIX: usize = 10;

pub struct TextOpts {
    pub bold: bool,
    pub big: bool,
    pub center: bool,
    pub stamp: bool,
    pub checklist: bool,
}

pub struct Slip<'a> {
    buf: Vec<u8>,
    cfg: &'a Config,
}

impl<'a> Slip<'a> {
    pub fn new(cfg: &'a Config) -> Self {
        let mut buf = vec![ESC, b'@'];
        if let Some(n) = cfg.codepage {
            buf.extend_from_slice(&[ESC, b't', n]);
        }
        Slip { buf, cfg }
    }

    pub fn width(&self) -> usize {
        self.cfg.paper_chars.max(16)
    }

    /// Printable width in dots (font A is 12 dots per character).
    pub fn dots(&self) -> u32 {
        self.width() as u32 * 12
    }

    pub fn center(&mut self) {
        self.buf.extend_from_slice(&[ESC, b'a', 1]);
    }

    pub fn left(&mut self) {
        self.buf.extend_from_slice(&[ESC, b'a', 0]);
    }

    pub fn bold(&mut self, on: bool) {
        self.buf.extend_from_slice(&[ESC, b'E', on as u8]);
    }

    pub fn big(&mut self, on: bool) {
        self.buf.extend_from_slice(&[GS, b'!', if on { 0x11 } else { 0x00 }]);
    }

    pub fn feed(&mut self, lines: u8) {
        self.buf.extend_from_slice(&[ESC, b'd', lines]);
    }

    /// Text → what the printer will actually be sent (Greek handling, punctuation).
    pub fn prep(&self, s: &str) -> String {
        if self.cfg.greeklish {
            translit(s)
        } else {
            s.chars().map(fold).collect()
        }
    }

    /// Print one line that has already been through `prep`.
    pub fn put(&mut self, s: &str) {
        let bytes: Vec<u8> = s.chars().map(enc_char).collect();
        self.buf.extend(bytes);
        self.buf.push(b'\n');
    }

    pub fn line(&mut self, s: &str) {
        let p = self.prep(s);
        self.put(&p);
    }

    /// Wrapped paragraph.
    pub fn para(&mut self, s: &str) {
        let p = self.prep(s);
        for l in wrap(&p, self.width()) {
            self.put(&l);
        }
    }

    pub fn rule(&mut self) {
        let w = self.width();
        self.put(&"-".repeat(w));
    }

    /// "label ........ right" on one line.
    pub fn item(&mut self, label: &str, right: &str) {
        let w = self.width();
        let right = self.prep(right);
        let mut label = self.prep(label);
        let max_label = w.saturating_sub(right.chars().count() + 2);
        if label.chars().count() > max_label {
            label = label.chars().take(max_label).collect();
        }
        let dots = w.saturating_sub(label.chars().count() + right.chars().count()).max(1);
        self.put(&format!("{label}{}{right}", ".".repeat(dots)));
    }

    pub fn title(&mut self, text: &str) {
        self.center();
        self.bold(true);
        self.big(true);
        self.line(text);
        self.big(false);
        self.bold(false);
        self.left();
    }

    pub fn raster(&mut self, bm: &Bitmap, align: u8) {
        self.buf.extend_from_slice(&[ESC, b'a', align]);
        self.buf.extend(raster::escpos(bm));
        self.buf.extend_from_slice(&[ESC, b'a', 0]);
    }

    /// Pip's little corner sticker (if enabled in settings).
    pub fn sticker(&mut self, mood: Mood) {
        if !self.cfg.pip_stickers {
            return;
        }
        if let Ok(bm) = raster::from_bytes(mood.sticker(), 110, 0.0, false) {
            self.raster(&bm, 2);
        }
    }

    /// Text size multiplier 1..=6 (both width and height).
    pub fn scale(&mut self, n: u8) {
        let n = n.clamp(1, 6) - 1;
        self.buf.extend_from_slice(&[GS, b'!', (n << 4) | n]);
    }

    /// A Code 39 barcode with the text printed under it.
    pub fn barcode(&mut self, text: &str) {
        let data: Vec<u8> = text
            .to_uppercase()
            .bytes()
            .filter(|b| b.is_ascii_digit() || b.is_ascii_uppercase() || b" -.$/+%".contains(b))
            .collect();
        if data.is_empty() {
            return;
        }
        self.center();
        self.buf.extend_from_slice(&[GS, b'h', 64, GS, b'w', 2, GS, b'H', 2]);
        self.buf.extend_from_slice(&[GS, b'k', 69, data.len() as u8]);
        self.buf.extend_from_slice(&data);
        self.buf.push(b'\n');
        self.left();
    }

    /// Feed the paper past the cutter and cut.
    pub fn finish_with(mut self, mood: Mood) -> Vec<u8> {
        self.sticker(mood);
        self.center();
        self.put("made by miketsak.gr");
        self.left();
        self.finish_plain()
    }

    /// Cut without a sticker.
    pub fn finish_plain(mut self) -> Vec<u8> {
        self.feed(4);
        self.buf.extend_from_slice(&[GS, b'V', 66, 3]);
        self.buf
    }

    pub fn finish(self) -> Vec<u8> {
        self.finish_with(Mood::Happy)
    }
}

pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in s.split_whitespace() {
        let mut chars: Vec<char> = word.chars().collect();
        while chars.len() > width {
            if cur_len > 0 {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            out.push(chars.drain(..width).collect());
        }
        let wl = chars.len();
        if wl == 0 {
            continue;
        }
        if cur_len > 0 && cur_len + 1 + wl > width {
            out.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.extend(chars.iter());
        cur_len += wl;
    }
    if cur_len > 0 {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn money(x: f64) -> String {
    format!("${x:.2}")
}

fn hm(minutes: u32) -> String {
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

fn days_left(days: i64) -> String {
    match days {
        0 => "TODAY!".to_string(),
        1 => "tomorrow".to_string(),
        d if d < 0 => format!("{} d ago", -d),
        d => format!("{d} days"),
    }
}

fn field(s: &mut Slip, label: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    s.bold(true);
    s.put(label);
    s.bold(false);
    s.para(value);
}

fn serial() -> String {
    format!("PIP-{}-{:04}", Local::now().format("%y%m%d"), crate::pip::rand(10000))
}

// ---------------------------------------------------------------- slips

/// Events grouped by day, used by the agenda and the briefing.
fn events_block(s: &mut Slip, events: &[Event]) {
    let cfg = s.cfg;
    if events.is_empty() {
        s.line("Nothing planned. Enjoy the day!");
        return;
    }
    let text_width = s.width().saturating_sub(PREFIX).max(8);
    let mut last_day = None;
    for e in events {
        let day = e.start.date_naive();
        if last_day != Some(day) {
            last_day = Some(day);
            s.bold(true);
            s.line(&e.start.format("%A %d %B").to_string());
            s.bold(false);
        }
        let when = if e.all_day {
            "all day".to_string()
        } else {
            e.start.format("%H:%M").to_string()
        };
        let title = s.prep(&e.summary);
        for (i, l) in wrap(&title, text_width).iter().enumerate() {
            let head = if i == 0 {
                format!("  {:<7} ", when)
            } else {
                " ".repeat(PREFIX)
            };
            s.put(&format!("{head}{l}"));
        }
        if cfg.show_locations {
            if let Some(loc) = &e.location {
                let p = s.prep(loc);
                for (i, l) in wrap(&p, text_width.saturating_sub(2).max(6)).iter().enumerate() {
                    let head = if i == 0 {
                        format!("{}@ ", " ".repeat(PREFIX))
                    } else {
                        " ".repeat(PREFIX + 2)
                    };
                    s.put(&format!("{head}{l}"));
                }
            }
        }
    }
}

pub fn agenda(cfg: &Config, events: &[Event]) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("AGENDA");
    s.center();
    s.line(&Local::now().format("%A %d %B %Y").to_string());
    s.left();
    s.rule();
    events_block(&mut s, events);
    s.rule();
    s.finish()
}

pub fn briefing(
    cfg: &Config,
    events: &[Event],
    weather: Option<&Weather>,
    todos: &[Todo],
    countdowns: &[(String, i64)],
    quote: &str,
) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("GOOD MORNING");
    s.center();
    s.line(&Local::now().format("%A %d %B %Y").to_string());
    s.left();
    s.rule();
    if let Some(w) = weather {
        s.bold(true);
        s.line(&format!("WEATHER  {}", w.place));
        s.bold(false);
        s.line(&format!("{}, {:.0}C now", w.desc, w.temp));
        s.line(&format!("High {:.0}C  Low {:.0}C  Rain {}%", w.hi, w.lo, w.rain));
        s.rule();
    }
    s.bold(true);
    s.line("TODAY");
    s.bold(false);
    events_block(&mut s, events);
    let open: Vec<&Todo> = todos.iter().filter(|t| !t.done).collect();
    if !open.is_empty() {
        s.rule();
        s.bold(true);
        s.line("TO-DO");
        s.bold(false);
        for t in open.iter().take(8) {
            let p = s.prep(&t.text);
            for (i, l) in wrap(&p, s.width().saturating_sub(4)).iter().enumerate() {
                s.put(&format!("{}{}", if i == 0 { "[ ] " } else { "    " }, l));
            }
        }
        if open.len() > 8 {
            s.line(&format!("...and {} more", open.len() - 8));
        }
    }
    if !countdowns.is_empty() {
        s.rule();
        s.bold(true);
        s.line("COUNTDOWNS");
        s.bold(false);
        for (name, days) in countdowns.iter().take(3) {
            s.item(name, &days_left(*days));
        }
    }
    s.rule();
    s.center();
    s.para(quote);
    s.left();
    s.finish_with(Mood::Happy)
}

pub fn todo_list(cfg: &Config, todos: &[Todo]) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("TO-DO");
    s.center();
    s.line(&Local::now().format("%a %d %b %Y").to_string());
    s.left();
    s.rule();
    if todos.is_empty() {
        s.line("Nothing to do. Suspicious.");
    }
    for t in todos {
        let p = s.prep(&t.text);
        let mark = if t.done { "[x] " } else { "[ ] " };
        for (i, l) in wrap(&p, s.width().saturating_sub(4)).iter().enumerate() {
            s.put(&format!("{}{}", if i == 0 { mark } else { "    " }, l));
        }
    }
    s.rule();
    let all_done = !todos.is_empty() && todos.iter().all(|t| t.done);
    s.finish_with(if all_done { Mood::Party } else { Mood::Happy })
}

/// A weekly stamp card (Monday to Sunday) for one habit.
pub fn habit_card(cfg: &Config, h: &Habit, today: NaiveDate, streak: u32) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("HABIT CARD");
    s.center();
    s.bold(true);
    s.para(&h.name);
    s.bold(false);
    s.left();
    s.rule();
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let mut names = String::new();
    let mut boxes = String::new();
    for i in 0..7 {
        let d = monday + chrono::Duration::days(i);
        let key = d.format("%Y-%m-%d").to_string();
        names.push_str(&format!("{:<4}", &d.format("%a").to_string()[..2]));
        boxes.push_str(if h.days.contains(&key) {
            "[X] "
        } else if d > today {
            "[.] "
        } else {
            "[ ] "
        });
    }
    s.line(names.trim_end());
    s.line(boxes.trim_end());
    s.rule();
    s.line(&format!("Streak: {streak} day{}", if streak == 1 { "" } else { "s" }));
    s.finish_with(if streak >= 7 { Mood::Party } else { Mood::Happy })
}

pub fn reminder(cfg: &Config, text: &str) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("REMINDER");
    s.rule();
    s.bold(true);
    s.para(text);
    s.bold(false);
    s.line(&Local::now().format("%a %d %b  %H:%M").to_string());
    s.rule();
    s.finish_with(Mood::Wink)
}

/// Stamp card for a finished focus session.
pub fn pomodoro_done(cfg: &Config, task: &str, minutes: u32, today_count: u32) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("FOCUS DONE");
    s.rule();
    if !task.trim().is_empty() {
        s.bold(true);
        s.para(task);
        s.bold(false);
    }
    s.line(&format!("{minutes} minutes of deep work"));
    s.line(&Local::now().format("%a %d %b  %H:%M").to_string());
    let mut stamps = String::new();
    for i in 0..8u32 {
        stamps.push_str(if i < today_count { "(X)" } else { "( )" });
    }
    s.rule();
    s.center();
    s.line(&stamps);
    s.line(&format!("Session {today_count} today"));
    s.left();
    s.finish_with(Mood::Party)
}

pub struct DayStats {
    pub meetings: usize,
    pub todos_done: usize,
    pub focus_sessions: u32,
    pub habits_kept: usize,
    pub usage: Option<DayUsage>,
}

/// The "receipt of your day".
pub fn end_of_day(cfg: &Config, st: &DayStats) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.center();
    s.bold(true);
    s.big(true);
    s.line("PIP'S DESK CAFE");
    s.big(false);
    s.bold(false);
    s.line("Receipt of your day");
    s.line(&Local::now().format("%a %d %b %Y  %H:%M").to_string());
    s.left();
    s.rule();

    let mut total = 0.0;
    let mut add = |s: &mut Slip, label: String, price: f64| {
        total += price;
        s.item(&label, &money(price));
    };
    add(&mut s, format!("Meetings attended x{}", st.meetings), st.meetings as f64 * 12.5);
    add(&mut s, format!("Focus sessions x{}", st.focus_sessions), st.focus_sessions as f64 * 8.0);
    add(&mut s, format!("Tasks finished x{}", st.todos_done), st.todos_done as f64 * 3.5);
    add(&mut s, format!("Habits kept x{}", st.habits_kept), st.habits_kept as f64 * 2.0);
    if let Some(u) = &st.usage {
        add(&mut s, format!("Time at PC {}", hm(u.active_minutes)), u.active_minutes as f64 * 0.05);
        let mut apps: Vec<(&String, &u32)> = u.apps.iter().collect();
        apps.sort_by(|a, b| b.1.cmp(a.1));
        for (name, mins) in apps.into_iter().take(5) {
            add(&mut s, format!("  {name} {mins}m"), *mins as f64 * 0.01);
        }
    }
    s.rule();
    s.item("SUBTOTAL", &money(total));
    let tax = total * 0.2;
    s.item("COFFEE TAX 20%", &money(tax));
    s.bold(true);
    s.item("TOTAL", &money(total + tax));
    s.bold(false);
    s.rule();
    s.center();
    s.bold(true);
    s.line("THANK YOU, COME AGAIN!");
    s.bold(false);
    s.para(pip::slip_line(Local::now().timestamp() as u64 / 3600));
    s.left();
    s.finish_with(Mood::Sleepy)
}

pub fn mail_slip(cfg: &Config, mails: &[Mail]) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("NEW MAIL");
    s.rule();
    for m in mails {
        let from = match m.from.split('<').next() {
            Some(name) if !name.trim().is_empty() => name.trim().trim_matches('"').to_string(),
            _ => m.from.clone(),
        };
        s.bold(true);
        s.para(&from);
        s.bold(false);
        s.para(if m.subject.is_empty() { "(no subject)" } else { &m.subject });
        s.rule();
    }
    s.finish()
}

pub fn alert(cfg: &Config, title: &str, body: &str, mood: Mood) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title(title);
    s.rule();
    s.bold(true);
    s.para(body);
    s.bold(false);
    s.line(&Local::now().format("%a %d %b  %H:%M").to_string());
    s.rule();
    s.finish_with(mood)
}

/// A heading, a QR code and some caption lines.
pub fn qr_slip(cfg: &Config, heading: &str, qr_text: &str, caption: &[String]) -> Result<Vec<u8>, String> {
    let mut s = Slip::new(cfg);
    let probe = raster::qr(qr_text, 1)?;
    let scale = ((s.dots() as usize * 4 / 5) / probe.w).clamp(2, 10);
    let bm = raster::qr(qr_text, scale)?;
    s.center();
    s.bold(true);
    s.para(heading);
    s.bold(false);
    s.left();
    s.raster(&bm, 1);
    s.center();
    for c in caption {
        s.para(c);
    }
    s.left();
    Ok(s.finish())
}

pub fn image_slip(cfg: &Config, bm: &Bitmap, caption: &str) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.raster(bm, 1);
    if !caption.trim().is_empty() {
        s.center();
        s.para(caption);
        s.left();
    }
    s.finish()
}

/// A big Pip sticker you can peel off the roll.
pub fn sticker_slip(cfg: &Config, mood: Mood) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    if let Ok(bm) = raster::from_bytes(mood.sticker(), (s.dots() * 3 / 5).min(300), 0.0, false) {
        s.raster(&bm, 1);
    }
    let mut buf = s.buf;
    buf.extend_from_slice(&[ESC, b'd', 4, GS, b'V', 66, 3]);
    buf
}

/// A Keep-style note as a slip.
pub fn note_slip(cfg: &Config, n: &Note) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.rule();
    if !n.title.trim().is_empty() {
        s.bold(true);
        s.para(&n.title);
        s.bold(false);
        s.rule();
    }
    if n.checklist {
        for it in &n.items {
            let mark = if it.done { "[x] " } else { "[ ] " };
            let p = s.prep(&it.text);
            for (i, l) in wrap(&p, s.width().saturating_sub(4)).iter().enumerate() {
                s.put(&format!("{}{}", if i == 0 { mark } else { "    " }, l));
            }
        }
    } else {
        for line in n.body.lines() {
            if line.trim().is_empty() {
                s.put("");
            } else {
                s.para(line);
            }
        }
    }
    if !n.labels.is_empty() {
        s.line(&n.labels.iter().map(|l| format!("#{l}")).collect::<Vec<_>>().join(" "));
    }
    s.rule();
    s.finish_with(Mood::Wink)
}

pub fn event_ticket(cfg: &Config, name: &str, when: &str, venue: &str, seat: &str, price: &str) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.rule();
    s.center();
    s.bold(true);
    s.line("*  ADMIT ONE  *");
    s.big(true);
    let half = s.width() / 2;
    let p = s.prep(name);
    for l in wrap(&p, half) {
        s.put(&l);
    }
    s.big(false);
    s.bold(false);
    s.left();
    s.rule();
    field(&mut s, "DATE", when);
    field(&mut s, "WHERE", venue);
    field(&mut s, "SEAT", seat);
    field(&mut s, "PRICE", price);
    s.rule();
    s.barcode(&serial());
    s.finish_with(Mood::Party)
}

pub fn boarding_pass(cfg: &Config, from: &str, to: &str, when: &str, flight: &str, seat: &str, who: &str) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("BOARDING PASS");
    s.rule();
    s.center();
    s.line("FROM");
    s.bold(true);
    s.big(true);
    let half = s.width() / 2;
    for l in wrap(&s.prep(from), half) {
        s.put(&l);
    }
    s.big(false);
    s.bold(false);
    s.line("TO");
    s.bold(true);
    s.big(true);
    for l in wrap(&s.prep(to), half) {
        s.put(&l);
    }
    s.big(false);
    s.bold(false);
    s.left();
    s.rule();
    field(&mut s, "DATE", when);
    field(&mut s, "FLIGHT", flight);
    field(&mut s, "SEAT / GATE", seat);
    field(&mut s, "PASSENGER", who);
    s.rule();
    s.barcode(&serial());
    s.finish_with(Mood::Wink)
}

/// Big text for labels, one slip per copy.
pub fn big_label(cfg: &Config, text: &str, scale: u8, copies: u8) -> Vec<u8> {
    let scale = scale.clamp(1, 6);
    let mut out = Vec::new();
    for _ in 0..copies.clamp(1, 10) {
        let mut s = Slip::new(cfg);
        let width = (s.width() / scale as usize).max(1);
        s.rule();
        s.center();
        s.bold(true);
        s.scale(scale);
        let p = s.prep(text);
        for l in wrap(&p, width) {
            s.put(&l);
        }
        s.scale(1);
        s.bold(false);
        s.left();
        s.rule();
        out.extend(s.finish_plain());
    }
    out
}

/// Monday to Sunday with the week's events and room to write.
pub fn week_planner(cfg: &Config, monday: NaiveDate, events: &[Event], todos: &[Todo]) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("WEEK PLANNER");
    s.center();
    s.line(&format!("{} - {}", monday.format("%d %b"), (monday + chrono::Duration::days(6)).format("%d %b %Y")));
    s.left();
    s.rule();
    let text_width = s.width().saturating_sub(8).max(8);
    for i in 0..7 {
        let day = monday + chrono::Duration::days(i);
        s.bold(true);
        s.line(&day.format("%a %d %b").to_string());
        s.bold(false);
        for e in events.iter().filter(|e| e.start.date_naive() == day) {
            let when = if e.all_day { "all day".to_string() } else { e.start.format("%H:%M").to_string() };
            let title = s.prep(&e.summary);
            for (k, l) in wrap(&title, text_width).iter().enumerate() {
                let head = if k == 0 { format!("  {:<5} ", when) } else { " ".repeat(8) };
                s.put(&format!("{head}{l}"));
            }
        }
        s.put(&format!("  {}", ".".repeat(s.width().saturating_sub(4))));
        s.put(&format!("  {}", ".".repeat(s.width().saturating_sub(4))));
    }
    let open: Vec<&Todo> = todos.iter().filter(|t| !t.done).collect();
    if !open.is_empty() {
        s.rule();
        s.bold(true);
        s.line("TO-DO");
        s.bold(false);
        for t in open.iter().take(10) {
            let p = s.prep(&t.text);
            for (k, l) in wrap(&p, s.width().saturating_sub(4)).iter().enumerate() {
                s.put(&format!("{}{}", if k == 0 { "[ ] " } else { "    " }, l));
            }
        }
    }
    s.rule();
    s.finish()
}

/// Days left until things you care about.
pub fn countdown_slip(cfg: &Config, items: &[(String, i64)]) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("COUNTDOWN");
    s.rule();
    if items.is_empty() {
        s.line("Nothing to count down to yet.");
    }
    for (name, days) in items {
        s.item(name, &days_left(*days));
    }
    s.rule();
    s.finish_with(Mood::Party)
}

/// Fortune cookie / joke.
pub fn fortune_slip(cfg: &Config, heading: &str, text: &str, lucky_numbers: bool) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title(heading);
    s.rule();
    s.center();
    s.bold(true);
    s.para(text);
    s.bold(false);
    if lucky_numbers {
        let nums: Vec<String> = (0..6).map(|_| format!("{:02}", 1 + crate::pip::rand(49))).collect();
        s.put("");
        s.line(&format!("Lucky numbers: {}", nums.join(" ")));
    }
    s.left();
    s.rule();
    s.finish_with(Mood::Wink)
}

/// "Pip decides": coin, dice, 8-ball or a pick from a list.
pub fn decide_slip(cfg: &Config, question: &str, answer: &str) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.title("PIP DECIDES");
    if !question.trim().is_empty() {
        s.center();
        s.para(question);
        s.left();
    }
    s.rule();
    s.center();
    s.bold(true);
    s.big(true);
    let half = s.width() / 2;
    for l in wrap(&s.prep(answer), half) {
        s.put(&l);
    }
    s.big(false);
    s.bold(false);
    s.left();
    s.rule();
    s.line(&Local::now().format("%a %d %b  %H:%M").to_string());
    s.finish_with(Mood::Wink)
}

pub fn stub(cfg: &Config, e: &Event) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.rule();
    s.center();
    s.bold(true);
    s.line("ADMIT ONE");
    s.bold(false);
    s.left();
    s.bold(true);
    s.para(&e.summary);
    s.bold(false);
    let when = if e.all_day {
        e.start.format("%a %d %b %Y  (all day)").to_string()
    } else {
        e.start.format("%a %d %b %Y  %H:%M").to_string()
    };
    s.line(&when);
    if let Some(loc) = &e.location {
        s.para(loc);
    }
    s.rule();
    s.finish()
}

fn format_text_lines(cfg: &Config, text: &str, o: &TextOpts) -> Vec<String> {
    let base_w = cfg.paper_chars.max(16);
    let width = if o.big { (base_w / 2).max(8) } else { base_w };
    let prep = |s: &str| -> String {
        if cfg.greeklish {
            translit(s)
        } else {
            s.chars().map(fold).collect()
        }
    };
    let mut out = Vec::new();
    let text = text.replace("\r\n", "\n");
    for raw in text.split('\n') {
        let raw = raw.trim_end();
        if raw.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        if o.checklist {
            let trimmed = raw.trim();
            let (prefix, body) = if let Some(rest) = trimmed.strip_prefix("- [x]").or_else(|| trimmed.strip_prefix("- [X]")) {
                ("[x] ", rest.trim())
            } else if let Some(rest) = trimmed.strip_prefix("[x]").or_else(|| trimmed.strip_prefix("[X]")) {
                ("[x] ", rest.trim())
            } else {
                let item = trimmed
                    .trim_start_matches("- [ ]")
                    .trim_start_matches("[ ]")
                    .trim_start_matches("- ")
                    .trim();
                ("[ ] ", item)
            };
            let p = prep(body);
            for (i, l) in wrap(&p, width.saturating_sub(4).max(1)).iter().enumerate() {
                out.push(format!("{}{}", if i == 0 { prefix } else { "    " }, l));
            }
        } else {
            let p = prep(raw);
            out.extend(wrap(&p, width));
        }
    }
    out
}

pub fn preview_text(cfg: &Config, text: &str, o: &TextOpts) -> String {
    let mut lines = format_text_lines(cfg, text, o);
    if o.stamp {
        lines.push(Local::now().format("%d %b %Y  %H:%M").to_string());
    }
    lines.join("\n")
}

pub fn titled_text_slip(cfg: &Config, heading: Option<&str>, text: &str, o: &TextOpts) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    if let Some(h) = heading.filter(|h| !h.trim().is_empty()) {
        s.center();
        s.bold(true);
        s.line(h);
        s.bold(false);
        s.left();
        s.rule();
    }
    if o.center {
        s.center();
    }
    if o.bold {
        s.bold(true);
    }
    if o.big {
        s.big(true);
    }
    for line in format_text_lines(cfg, text, o) {
        s.put(&line);
    }
    s.bold(false);
    s.big(false);
    if o.stamp {
        s.line(&Local::now().format("%d %b %Y  %H:%M").to_string());
    }
    s.left();
    s.finish()
}

pub fn text_slip(cfg: &Config, text: &str, o: &TextOpts) -> Vec<u8> {
    titled_text_slip(cfg, None, text, o)
}

pub fn test_slip(cfg: &Config) -> Vec<u8> {
    let mut s = Slip::new(cfg);
    s.center();
    s.bold(true);
    s.big(true);
    s.line("THERMO");
    s.line("COMPANION");
    s.big(false);
    s.bold(false);
    s.left();
    s.rule();
    s.line("Printer connection OK");
    s.line("Characters per line (ruler):");
    let ruler: String = (0..s.width())
        .map(|i| char::from(b'0' + ((i + 1) % 10) as u8))
        .collect();
    s.put(&ruler);
    s.line("Latin: The quick brown fox");
    s.line("Greek: \u{393}\u{3b5}\u{3b9}\u{3ac} \u{3c3}\u{3bf}\u{3c5} \u{3ba}\u{3cc}\u{3c3}\u{3bc}\u{3b5}");
    s.line(&Local::now().format("%d %b %Y  %H:%M").to_string());
    s.rule();
    s.finish_with(Mood::Party)
}

// ------------------------------------------------------------- encoding

/// Typographic punctuation that printers don't have → plain ASCII.
fn fold(c: char) -> char {
    match c {
        '\u{2013}' | '\u{2014}' => '-',
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{2026}' => '.',
        '\u{a0}' => ' ',
        _ => c,
    }
}

fn enc_char(c: char) -> u8 {
    if c == '\t' {
        b' '
    } else if (' '..='~').contains(&c) {
        c as u8
    } else {
        cp737(c).unwrap_or(b'?')
    }
}

/// Greek letters in code page 737 (used when "print Greek as Latin" is off).
fn cp737(c: char) -> Option<u8> {
    const UPPER: &str = "ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩ";
    const LOWER: &str = "αβγδεζηθικλμνξοπρσςτυφχψ";
    if let Some(i) = UPPER.chars().position(|x| x == c) {
        return Some(0x80 + i as u8);
    }
    if let Some(i) = LOWER.chars().position(|x| x == c) {
        return Some(0x98 + i as u8);
    }
    Some(match c {
        'ω' => 0xE0,
        'ά' => 0xE1,
        'έ' => 0xE2,
        'ή' => 0xE3,
        'ϊ' => 0xE4,
        'ί' => 0xE5,
        'ό' => 0xE6,
        'ύ' => 0xE7,
        'ϋ' => 0xE8,
        'ώ' => 0xE9,
        'Ά' => 0xEA,
        'Έ' => 0xEB,
        'Ή' => 0xEC,
        'Ί' => 0xED,
        'Ό' => 0xEE,
        'Ύ' => 0xEF,
        'Ώ' => 0xF0,
        _ => return None,
    })
}

fn latin(lower: char) -> Option<&'static str> {
    Some(match lower {
        'α' | 'ά' => "a",
        'β' => "v",
        'γ' => "g",
        'δ' => "d",
        'ε' | 'έ' => "e",
        'ζ' => "z",
        'η' | 'ή' => "i",
        'θ' => "th",
        'ι' | 'ί' | 'ϊ' | 'ΐ' => "i",
        'κ' => "k",
        'λ' => "l",
        'μ' => "m",
        'ν' => "n",
        'ξ' => "x",
        'ο' | 'ό' => "o",
        'π' => "p",
        'ρ' => "r",
        'σ' | 'ς' => "s",
        'τ' => "t",
        'υ' | 'ύ' | 'ϋ' | 'ΰ' => "y",
        'φ' => "f",
        'χ' => "ch",
        'ψ' => "ps",
        'ω' | 'ώ' => "o",
        _ => return None,
    })
}

/// Greek → Latin letters ("Θέατρο" → "Theatro").
fn translit(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let lower = c.to_lowercase().next().unwrap_or(c);
        // "ου" reads as "ou", not "oy".
        if matches!(lower, 'ο' | 'ό')
            && i + 1 < chars.len()
            && matches!(chars[i + 1].to_lowercase().next(), Some('υ') | Some('ύ'))
        {
            out.push_str(if c.is_uppercase() { "Ou" } else { "ou" });
            i += 2;
            continue;
        }
        match latin(lower) {
            Some(l) if c.is_uppercase() => {
                let mut it = l.chars();
                if let Some(first) = it.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(it.as_str());
                }
            }
            Some(l) => out.push_str(l),
            None => out.push(fold(c)),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> TextOpts {
        TextOpts { bold: false, big: false, center: false, stamp: false, checklist: false }
    }

    #[test]
    fn greek_to_latin() {
        assert_eq!(translit("Θέατρο"), "Theatro");
        assert_eq!(translit("Βανέσα"), "Vanesa");
        assert_eq!(translit("Σάκης Ρουβάς"), "Sakis Rouvas");
    }

    #[test]
    fn slip_starts_with_init_and_ends_with_cut() {
        let cfg = Config::default();
        let data = text_slip(&cfg, "hello", &opts());
        assert!(data.starts_with(&[ESC, b'@']));
        assert!(data.ends_with(&[GS, b'V', 66, 3]));
    }

    #[test]
    fn wrap_respects_width() {
        for l in wrap("the quick brown fox jumps over the lazy dog", 10) {
            assert!(l.chars().count() <= 10);
        }
    }

    #[test]
    fn every_slip_builds() {
        let cfg = Config::default();
        let todos = vec![Todo { id: 1, text: "write code".into(), done: false }];
        let habit = Habit { id: 1, name: "Read".into(), days: vec![] };
        let today = Local::now().date_naive();
        assert!(briefing(&cfg, &[], None, &todos, &[("Trip".into(), 7)], "hi").len() > 20);
        let note = Note { id: 1, title: "Groceries".into(), checklist: true, items: vec![crate::data::NoteItem { text: "milk".into(), done: false }], ..Default::default() };
        assert!(note_slip(&cfg, &note).len() > 20);
        assert!(event_ticket(&cfg, "Concert", "Fri 20:00", "Odeon", "A12", "10 EUR").len() > 40);
        assert!(boarding_pass(&cfg, "ATHENS", "CAIRO", "26 Nov", "A3 123", "12A", "Pip").len() > 40);
        assert!(big_label(&cfg, "FRAGILE", 3, 2).len() > 40);
        assert!(week_planner(&cfg, today, &[], &todos).len() > 40);
        assert!(countdown_slip(&cfg, &[("Trip".into(), 3)]).len() > 20);
        assert!(fortune_slip(&cfg, "FORTUNE", "You will print.", true).len() > 20);
        assert!(decide_slip(&cfg, "Pizza?", "YES").len() > 20);
        assert!(todo_list(&cfg, &todos).len() > 20);
        assert!(habit_card(&cfg, &habit, today, 0).len() > 20);
        assert!(reminder(&cfg, "stand up").len() > 20);
        assert!(pomodoro_done(&cfg, "task", 25, 2).len() > 20);
        assert!(alert(&cfg, "ALERT", "battery low", Mood::Sad).len() > 20);
        assert!(qr_slip(&cfg, "Wi-Fi", "WIFI:T:WPA;S:home;P:secret;;", &[]).unwrap().len() > 100);
        let st = DayStats { meetings: 2, todos_done: 3, focus_sessions: 4, habits_kept: 1, usage: None };
        assert!(end_of_day(&cfg, &st).len() > 20);
        assert!(sticker_slip(&cfg, Mood::Happy).len() > 20);
    }
}
