//! Tiny localhost-only HTTP bridge (`127.0.0.1:47315`) for the Chrome extension.
//! Receives right-click "Thermo Print" actions (selected text, image URL, or link/page QR).

use crate::app::{self, Notify, Shared};
use crate::config;
use crate::escpos::{self, TextOpts};
use crate::gcal::{agent, http_err};
use crate::raster;
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

pub const PORT: u16 = 47315;

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct PrintReq {
    pub kind: String,
    pub text: String,
    pub title: String,
    pub url: String,
}

fn reply(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn read_http(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), &'static str> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut header_end = None;

    while header_end.is_none() {
        let n = stream.read(&mut chunk).map_err(|_| "read failed")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = Some(pos + 4);
        }
        if buf.len() > 32 * 1024 {
            return Err("headers too large");
        }
    }

    let head_end = header_end.ok_or("incomplete HTTP request")?;
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| "invalid UTF-8 headers")?;
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let content_len = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);

    if content_len > 512 * 1024 {
        return Err("payload too large");
    }

    let mut body = buf[head_end..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut chunk).map_err(|_| "body read failed")?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_len);
    Ok((method, path, body))
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err("invalid base64 image data".into()),
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1u32 << bits) - 1;
        }
    }
    Ok(out)
}

fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',').ok_or("invalid data URL")?;
        if !meta.contains(";base64") {
            return Err("only base64 data URLs are supported".into());
        }
        return decode_base64(data);
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("image URL must be http(s) or data URL".into());
    }
    let resp = agent()?.get(url).call().map_err(http_err)?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(10 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn short_label(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    let mut out: String = first.chars().take(max).collect();
    if first.chars().count() > max {
        out.push_str("...");
    }
    out
}

fn dispatch(sh: &Arc<Shared>, notify: &Notify, req: PrintReq) -> Result<(), &'static str> {
    match req.kind.as_str() {
        "text" => {
            if req.text.trim().is_empty() {
                return Err("no text selected");
            }
            let text = req.text;
            let title = req.title;
            let lbl = format!("Chrome: {}", short_label(&text, 24));
            app::spawn_job(sh, notify.clone(), "Printed from Chrome", move |s| {
                let cfg = s.cfg();
                let opts = TextOpts { bold: false, big: false, center: false, stamp: true, checklist: false };
                let heading = (!title.trim().is_empty()).then_some(title.trim());
                s.print_as(&lbl, escpos::titled_text_slip(&cfg, heading, &text, &opts))
            });
            Ok(())
        }
        "image" => {
            if req.url.trim().is_empty() {
                return Err("missing image url");
            }
            let url = req.url;
            let caption = req.title;
            app::spawn_job(sh, notify.clone(), "Printed image from Chrome", move |s| {
                let cfg = s.cfg();
                let dots = (cfg.paper_chars.max(16) as u32) * 12;
                let bytes = fetch_image(&url)?;
                let bm = raster::from_bytes(&bytes, dots, 0.0, true)?;
                s.print_as("Chrome image", escpos::image_slip(&cfg, &bm, &caption))
            });
            Ok(())
        }
        "qr" => {
            if req.url.trim().is_empty() {
                return Err("missing url");
            }
            let url = req.url;
            let title = req.title;
            let lbl = format!("QR: {}", short_label(&url, 24));
            app::spawn_job(sh, notify.clone(), "Printed QR from Chrome", move |s| {
                let cfg = s.cfg();
                let heading = if title.trim().is_empty() {
                    "SCAN ME".to_string()
                } else {
                    short_label(&title, cfg.paper_chars.max(16))
                };
                let slip = escpos::qr_slip(&cfg, &heading, &url, &[url.clone()])?;
                s.print_as(&lbl, slip)
            });
            Ok(())
        }
        _ => Err("unknown print kind"),
    }
}

fn handle(sh: &Arc<Shared>, notify: &Notify, mut stream: TcpStream) {
    let (method, path, body) = match read_http(&mut stream) {
        Ok(r) => r,
        Err(msg) => {
            reply(&mut stream, "400 Bad Request", &format!(r#"{{"ok":false,"error":"{msg}"}}"#));
            return;
        }
    };

    if method == "OPTIONS" {
        reply(&mut stream, "204 No Content", "");
        return;
    }
    if method == "GET" && path == "/health" {
        reply(
            &mut stream,
            "200 OK",
            concat!(r#"{"ok":true,"app":"Thermo Companion","version":""#, env!("CARGO_PKG_VERSION"), r#""}"#),
        );
        return;
    }
    if method == "POST" && path == "/print" {
        let req: PrintReq = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(_) => {
                reply(&mut stream, "400 Bad Request", r#"{"ok":false,"error":"invalid JSON"}"#);
                return;
            }
        };
        match dispatch(sh, notify, req) {
            Ok(()) => reply(&mut stream, "200 OK", r#"{"ok":true}"#),
            Err(e) => reply(&mut stream, "400 Bad Request", &format!(r#"{{"ok":false,"error":"{e}"}}"#)),
        }
        return;
    }
    reply(&mut stream, "404 Not Found", r#"{"ok":false,"error":"not found"}"#);
}

/// Listen on `127.0.0.1:47315` for requests from the browser extension.
pub fn serve(sh: Arc<Shared>, notify: Notify) {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(e) => {
            config::log(&format!("browser bridge failed on 127.0.0.1:{PORT}: {e}"));
            return;
        }
    };
    for stream in listener.incoming().flatten() {
        let sh = sh.clone();
        let notify = notify.clone();
        std::thread::spawn(move || handle(&sh, &notify, stream));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_and_parses_request() {
        assert_eq!(decode_base64("SGVsbG8h").unwrap(), b"Hello!");
        let req: PrintReq = serde_json::from_str(r#"{"kind":"text","text":"MIELE Guard S1"}"#).unwrap();
        assert_eq!(req.kind, "text");
        assert_eq!(req.text, "MIELE Guard S1");
    }
}
