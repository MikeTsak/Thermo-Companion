//! Read-only Google Calendar access: OAuth "desktop app" loopback sign-in + events list.

use crate::config::Config;
use chrono::{DateTime, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const MAIL_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const EVENTS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";

#[derive(Clone, Debug)]
pub struct Event {
    pub id: String,
    pub summary: String,
    pub start: DateTime<Local>,
    pub all_day: bool,
    pub location: Option<String>,
}

pub(crate) fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(20))
        .build())
}

pub(crate) fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            format!("HTTP {code}: {}", resp.into_string().unwrap_or_default())
        }
        other => other.to_string(),
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{:02X}", b)
            }
        })
        .collect()
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(request_line: &str, key: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            Some(dec(v))
        } else {
            None
        }
    })
}

fn reply(stream: &mut std::net::TcpStream, status: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes());
}

/// Opens the browser for Google sign-in and returns a refresh token.
/// Blocks for up to 3 minutes; call it from a worker thread.
pub fn connect(client_id: &str, client_secret: &str, with_mail: bool) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let scope = if with_mail {
        format!("{SCOPE} {MAIL_SCOPE}")
    } else {
        SCOPE.to_string()
    };
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        enc(client_id),
        enc(&redirect),
        enc(&scope)
    );
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn();

    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let first = req.lines().next().unwrap_or("");
                if let Some(code) = query_param(first, "code") {
                    reply(
                        &mut stream,
                        "200 OK",
                        "<html><body style=\"font-family:Segoe UI,sans-serif;text-align:center;margin-top:20vh\"><h2>Thermo Companion is connected.</h2><p>Made by <a href=\"https://miketsak.gr\">miketsak.gr</a> — you can close this tab.</p></body></html>",
                    );
                    break code;
                }
                if let Some(err) = query_param(first, "error") {
                    reply(
                        &mut stream,
                        "200 OK",
                        "<html><body style=\"font-family:Segoe UI,sans-serif;text-align:center;margin-top:20vh\"><h2>Sign-in was cancelled.</h2><p>Made by <a href=\"https://miketsak.gr\">miketsak.gr</a> — you can close this tab.</p></body></html>",
                    );
                    return Err(err);
                }
                reply(&mut stream, "404 Not Found", "");
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("timed out waiting for the browser sign-in".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    let resp: serde_json::Value = agent()?
        .post(TOKEN_URL)
        .send_form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;

    resp["refresh_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            "Google did not return a refresh token. Remove Thermo Companion under \
             myaccount.google.com > Security > Third-party access, then connect again."
                .to_string()
        })
}

pub(crate) fn access_token(cfg: &Config) -> Result<String, String> {
    let resp: serde_json::Value = agent()?
        .post(TOKEN_URL)
        .send_form(&[
            ("client_id", cfg.google_client_id.as_str()),
            ("client_secret", cfg.google_client_secret.as_str()),
            ("refresh_token", cfg.google_refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    resp["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no access token in Google's reply".to_string())
}

fn parse_event(item: &serde_json::Value) -> Option<Event> {
    if item["status"].as_str() == Some("cancelled") {
        return None;
    }
    let start = &item["start"];
    let (start, all_day) = if let Some(dt) = start["dateTime"].as_str() {
        (DateTime::parse_from_rfc3339(dt).ok()?.with_timezone(&Local), false)
    } else {
        let date = NaiveDate::parse_from_str(start["date"].as_str()?, "%Y-%m-%d").ok()?;
        let midnight = Local.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).earliest()?;
        (midnight, true)
    };
    Some(Event {
        id: item["id"].as_str().unwrap_or_default().to_string(),
        summary: item["summary"].as_str().unwrap_or("(no title)").to_string(),
        start,
        all_day,
        location: item["location"].as_str().map(str::to_string),
    })
}

/// Events from the start of today up to `days` days ahead, oldest first.
pub fn fetch(cfg: &Config, days: i64) -> Result<Vec<Event>, String> {
    let token = access_token(cfg)?;
    let today0 = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .ok_or("bad date")?;
    let from = Local
        .from_local_datetime(&today0)
        .earliest()
        .ok_or("bad local time")?
        .with_timezone(&Utc);
    let to = from + chrono::Duration::days(days);

    let v: serde_json::Value = agent()?
        .get(EVENTS_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .query("timeMin", &from.to_rfc3339_opts(SecondsFormat::Secs, true))
        .query("timeMax", &to.to_rfc3339_opts(SecondsFormat::Secs, true))
        .query("singleEvents", "true")
        .query("orderBy", "startTime")
        .query("maxResults", "100")
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;

    let items = v["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let mut events: Vec<Event> = items.iter().filter_map(parse_event).collect();
    events.sort_by_key(|e| e.start);
    Ok(events)
}
