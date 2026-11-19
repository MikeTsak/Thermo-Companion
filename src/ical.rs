//! Calendar from a private iCal link (Google Calendar's "Secret address in iCal format",
//! or any .ics feed). No Google Cloud project and no sign-in needed.
//! Handles repeating events (daily / weekly / monthly / yearly), exceptions and time zones.

use crate::gcal::{agent, http_err, Event};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use std::collections::{HashMap, HashSet};
use std::io::Read;

#[derive(Clone, Debug)]
enum When {
    Date(NaiveDate),
    Utc(NaiveDateTime),
    Zoned(Tz, NaiveDateTime),
    Floating(NaiveDateTime),
}

impl When {
    fn naive(&self) -> NaiveDateTime {
        match self {
            When::Date(d) => d.and_hms_opt(0, 0, 0).unwrap(),
            When::Utc(n) | When::Zoned(_, n) | When::Floating(n) => *n,
        }
    }

    fn with_naive(&self, n: NaiveDateTime) -> When {
        match self {
            When::Date(_) => When::Date(n.date()),
            When::Utc(_) => When::Utc(n),
            When::Zoned(tz, _) => When::Zoned(*tz, n),
            When::Floating(_) => When::Floating(n),
        }
    }

    fn all_day(&self) -> bool {
        matches!(self, When::Date(_))
    }

    fn to_local(&self) -> Option<DateTime<Local>> {
        match self {
            When::Date(d) => Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest(),
            When::Utc(n) => Some(Utc.from_utc_datetime(n).with_timezone(&Local)),
            When::Zoned(tz, n) => tz.from_local_datetime(n).earliest().map(|d| d.with_timezone(&Local)),
            When::Floating(n) => Local.from_local_datetime(n).earliest(),
        }
    }
}

#[derive(Default)]
struct Raw {
    uid: String,
    summary: String,
    location: String,
    start: Option<When>,
    rrule: Option<String>,
    exdates: Vec<When>,
    recurrence_id: Option<When>,
    cancelled: bool,
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_when(params: &str, value: &str) -> Option<When> {
    let value = value.trim();
    let tzid = params
        .split(';')
        .find_map(|p| p.strip_prefix("TZID="))
        .map(|t| t.trim_matches('"'));
    if value.len() == 8 && !value.contains('T') {
        return NaiveDate::parse_from_str(value, "%Y%m%d").ok().map(When::Date);
    }
    if let Some(utc) = value.strip_suffix('Z') {
        return NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok().map(When::Utc);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    match tzid.and_then(|t| t.parse::<Tz>().ok()) {
        Some(tz) => Some(When::Zoned(tz, naive)),
        None => Some(When::Floating(naive)),
    }
}

fn parse_raw(text: &str) -> Vec<Raw> {
    // unfold continuation lines
    let mut lines: Vec<String> = Vec::new();
    for l in text.lines() {
        let l = l.trim_end_matches('\r');
        if (l.starts_with(' ') || l.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&l[1..]);
        } else {
            lines.push(l.to_string());
        }
    }

    let mut events = Vec::new();
    let mut cur: Option<Raw> = None;
    for l in lines {
        if l == "BEGIN:VEVENT" {
            cur = Some(Raw::default());
            continue;
        }
        if l == "END:VEVENT" {
            if let Some(r) = cur.take() {
                events.push(r);
            }
            continue;
        }
        let Some(raw) = cur.as_mut() else { continue };
        let Some((head, value)) = l.split_once(':') else { continue };
        let (name, params) = head.split_once(';').unwrap_or((head, ""));
        match name.to_ascii_uppercase().as_str() {
            "UID" => raw.uid = value.to_string(),
            "SUMMARY" => raw.summary = unescape(value),
            "LOCATION" => raw.location = unescape(value),
            "DTSTART" => raw.start = parse_when(params, value),
            "RRULE" => raw.rrule = Some(value.to_string()),
            "EXDATE" => raw.exdates.extend(value.split(',').filter_map(|v| parse_when(params, v))),
            "RECURRENCE-ID" => raw.recurrence_id = parse_when(params, value),
            "STATUS" => raw.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    events
}

// ------------------------------------------------------------ repeat rules

enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

struct Rule {
    freq: Freq,
    interval: i64,
    count: Option<usize>,
    until: Option<NaiveDateTime>,
    /// (ordinal like 1 or -1, weekday)
    byday: Vec<(Option<i32>, Weekday)>,
    bymonthday: Option<u32>,
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_rule(s: &str) -> Option<Rule> {
    let mut rule = Rule { freq: Freq::Daily, interval: 1, count: None, until: None, byday: vec![], bymonthday: None };
    let mut have_freq = false;
    for part in s.split(';') {
        let (k, v) = part.split_once('=')?;
        match k {
            "FREQ" => {
                rule.freq = match v {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                };
                have_freq = true;
            }
            "INTERVAL" => rule.interval = v.parse::<i64>().ok()?.max(1),
            "COUNT" => rule.count = v.parse().ok(),
            "UNTIL" => {
                let v = v.trim_end_matches('Z');
                rule.until = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
                    .ok()
                    .or_else(|| NaiveDate::parse_from_str(v, "%Y%m%d").ok().and_then(|d| d.and_hms_opt(23, 59, 59)));
            }
            "BYDAY" => {
                for d in v.split(',') {
                    let split = d.len().saturating_sub(2);
                    let (num, wd) = d.split_at(split);
                    let wd = parse_weekday(wd)?;
                    rule.byday.push((num.parse::<i32>().ok(), wd));
                }
            }
            "BYMONTHDAY" => rule.bymonthday = v.split(',').next().and_then(|x| x.parse().ok()),
            _ => {}
        }
    }
    have_freq.then_some(rule)
}

/// The n-th (or -1 = last) weekday of a month.
fn nth_weekday(year: i32, month: u32, wd: Weekday, n: i32) -> Option<NaiveDate> {
    if n > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64).rem_euclid(7);
        let d = first + Duration::days(shift + 7 * (n as i64 - 1));
        (d.month() == month).then_some(d)
    } else {
        let next = if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(year, month + 1, 1)? };
        let last = next - Duration::days(1);
        let shift = (last.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64).rem_euclid(7);
        let d = last - Duration::days(shift + 7 * (-n as i64 - 1));
        (d.month() == month).then_some(d)
    }
}

/// All start times of a repeating event that fall in [from, to] (naive, in the event's own zone).
fn expand(start: NaiveDateTime, rule: &Rule, from: NaiveDateTime, to: NaiveDateTime) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();
    let mut n = 0usize;
    let time = start.time();

    // returns false when generation should stop
    let mut push = |dt: NaiveDateTime| -> bool {
        if dt < start {
            return true;
        }
        if dt > to || rule.until.map_or(false, |u| dt > u) {
            return false;
        }
        n += 1;
        if dt >= from {
            out.push(dt);
        }
        rule.count.map_or(true, |c| n < c)
    };

    match rule.freq {
        Freq::Daily => {
            for k in 0..20000i64 {
                if !push(start + Duration::days(k * rule.interval)) {
                    break;
                }
            }
        }
        Freq::Weekly => {
            let mut days: Vec<Weekday> = rule.byday.iter().map(|b| b.1).collect();
            if days.is_empty() {
                days.push(start.weekday());
            }
            days.sort_by_key(|d| d.num_days_from_monday());
            let week0 = start.date() - Duration::days(start.weekday().num_days_from_monday() as i64);
            'weeks: for k in 0..5000i64 {
                let base = week0 + Duration::days(7 * rule.interval * k);
                for d in &days {
                    let date = base + Duration::days(d.num_days_from_monday() as i64);
                    if !push(date.and_time(time)) {
                        break 'weeks;
                    }
                }
            }
        }
        Freq::Monthly => {
            for m in 0..6000i64 {
                let total = start.year() as i64 * 12 + start.month0() as i64 + m * rule.interval;
                let (year, month) = (total.div_euclid(12) as i32, total.rem_euclid(12) as u32 + 1);
                let date = if let Some((ord, wd)) = rule.byday.first().copied() {
                    nth_weekday(year, month, wd, ord.unwrap_or(1))
                } else {
                    NaiveDate::from_ymd_opt(year, month, rule.bymonthday.unwrap_or(start.day()))
                };
                match date {
                    Some(d) => {
                        if !push(d.and_time(time)) {
                            break;
                        }
                    }
                    None => {
                        // e.g. the 31st in a 30-day month: skipped, but stop once far past the window
                        if NaiveDate::from_ymd_opt(year, month, 1).map_or(true, |d| d.and_time(time) > to) {
                            break;
                        }
                    }
                }
            }
        }
        Freq::Yearly => {
            for k in 0..400i64 {
                let year = start.year() + (k * rule.interval) as i32;
                if let Some(d) = NaiveDate::from_ymd_opt(year, start.month(), start.day()) {
                    if !push(d.and_time(time)) {
                        break;
                    }
                } else if NaiveDate::from_ymd_opt(year, 1, 1).map_or(true, |d| d.and_time(time) > to) {
                    break;
                }
            }
        }
    }
    out
}

fn make_event(raw: &Raw, when: &When) -> Option<Event> {
    let start = when.to_local()?;
    Some(Event {
        id: format!("{}@{}", raw.uid, start.timestamp()),
        summary: if raw.summary.is_empty() { "(no title)".to_string() } else { raw.summary.clone() },
        start,
        all_day: when.all_day(),
        location: (!raw.location.is_empty()).then(|| raw.location.clone()),
    })
}

/// Events that start between `from` and `to`, oldest first.
pub fn parse(text: &str, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Event> {
    let raws = parse_raw(text);

    // instances that were changed or moved individually (RECURRENCE-ID)
    let mut overridden: HashMap<String, HashSet<i64>> = HashMap::new();
    for r in &raws {
        if let Some(id) = r.recurrence_id.as_ref().and_then(|w| w.to_local()) {
            overridden.entry(r.uid.clone()).or_default().insert(id.timestamp());
        }
    }

    // generous naive bounds; exact filtering happens after the time zone conversion
    let nfrom = from.naive_local() - Duration::days(2);
    let nto = to.naive_local() + Duration::days(2);

    let mut out = Vec::new();
    let in_window = |e: &Event| e.start >= from && e.start <= to;
    for raw in &raws {
        if raw.cancelled {
            continue;
        }
        let Some(start) = raw.start.clone() else { continue };
        let skip: HashSet<i64> = raw
            .exdates
            .iter()
            .filter_map(|w| w.to_local())
            .map(|d| d.timestamp())
            .chain(overridden.get(&raw.uid).into_iter().flatten().copied())
            .collect();

        let rule = if raw.recurrence_id.is_none() { raw.rrule.as_deref().and_then(parse_rule) } else { None };
        match rule {
            Some(rule) => {
                for n in expand(start.naive(), &rule, nfrom, nto) {
                    if let Some(e) = make_event(raw, &start.with_naive(n)) {
                        if in_window(&e) && !skip.contains(&e.start.timestamp()) {
                            out.push(e);
                        }
                    }
                }
            }
            None => {
                if let Some(e) = make_event(raw, &start) {
                    let own_instance_overridden = raw.recurrence_id.is_none() && skip.contains(&e.start.timestamp());
                    if in_window(&e) && !own_instance_overridden {
                        out.push(e);
                    }
                }
            }
        }
    }
    out.sort_by_key(|e| e.start);
    out
}

/// Download the feed and return events from the start of today up to `days` days ahead.
pub fn fetch(url: &str, days: i64) -> Result<Vec<Event>, String> {
    let url = url.trim().replacen("webcal://", "https://", 1);
    if !url.starts_with("http") {
        return Err("that doesn't look like a link (it should start with https://)".into());
    }
    let resp = agent()?.get(&url).call().map_err(http_err)?;
    let mut text = String::new();
    resp.into_reader()
        .take(12_000_000)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("that link didn't return a calendar. Use the secret address in iCal format (ends in .ics).".into());
    }
    let today0 = Local::now().date_naive().and_hms_opt(0, 0, 0).ok_or("bad date")?;
    let from = Local.from_local_datetime(&today0).earliest().ok_or("bad local time")?;
    Ok(parse(&text, from, from + Duration::days(days)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(from: &str, days: i64) -> (DateTime<Local>, DateTime<Local>) {
        let d = NaiveDate::parse_from_str(from, "%Y-%m-%d").unwrap().and_hms_opt(0, 0, 0).unwrap();
        let f = Local.from_local_datetime(&d).earliest().unwrap();
        (f, f + Duration::days(days))
    }

    #[test]
    fn weekly_with_exception_and_single() {
        let ics = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Standup\r\nDTSTART;TZID=Europe/Athens:20260105T100000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE;TZID=Europe/Athens:20260114T100000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Dinner\\, with friends\r\nDTSTART:20260115T180000Z\r\nLOCATION:Home\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let (f, t) = window("2026-01-12", 7);
        let ev = parse(ics, f, t);
        let names: Vec<&str> = ev.iter().map(|e| e.summary.as_str()).collect();
        // Mon 12 and Wed 14 standups, but Wed 14 is an EXDATE; plus the dinner on the 15th
        assert_eq!(names.iter().filter(|n| **n == "Standup").count(), 1, "{names:?}");
        assert!(names.contains(&"Dinner, with friends"));
        assert_eq!(ev.iter().find(|e| e.summary == "Dinner, with friends").unwrap().location.as_deref(), Some("Home"));
    }

    #[test]
    fn exdate_and_override_are_respected() {
        let ics = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Gym\r\nDTSTART:20260105T070000Z\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEXDATE:20260107T070000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let (f, t) = window("2026-01-04", 10);
        let ev = parse(ics, f, t);
        assert_eq!(ev.len(), 4, "5 occurrences minus one exception");
    }

    #[test]
    fn monthly_last_friday() {
        let ics = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:Review\r\nDTSTART:20260130T120000Z\r\nRRULE:FREQ=MONTHLY;BYDAY=-1FR\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let (f, t) = window("2026-02-20", 14);
        let ev = parse(ics, f, t);
        assert_eq!(ev.len(), 1, "{:?}", ev.iter().map(|e| e.start).collect::<Vec<_>>());
        assert_eq!(ev[0].start.date_naive(), NaiveDate::from_ymd_opt(2026, 2, 27).unwrap());
    }
}
