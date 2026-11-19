//! Weather (Open-Meteo, no key needed), web-page change watching, and Gmail.

use crate::config::Config;
use crate::gcal::{access_token, agent, http_err};
use std::io::Read;

pub struct Weather {
    pub place: String,
    pub temp: f64,
    pub hi: f64,
    pub lo: f64,
    pub rain: u32,
    pub desc: &'static str,
}

/// City name → (latitude, longitude, display name).
pub fn geocode(city: &str) -> Result<(f64, f64, String), String> {
    let v: serde_json::Value = agent()?
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query("name", city)
        .query("count", "1")
        .query("language", "en")
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let r = v["results"].get(0).ok_or_else(|| format!("couldn't find a city called \"{city}\""))?;
    Ok((
        r["latitude"].as_f64().ok_or("no latitude")?,
        r["longitude"].as_f64().ok_or("no longitude")?,
        r["name"].as_str().unwrap_or(city).to_string(),
    ))
}

fn describe(code: i64) -> &'static str {
    match code {
        0 => "Clear sky",
        1 | 2 => "Mostly clear",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "Weather",
    }
}

pub fn weather(lat: f64, lon: f64, place: &str) -> Result<Weather, String> {
    let v: serde_json::Value = agent()?
        .get("https://api.open-meteo.com/v1/forecast")
        .query("latitude", &lat.to_string())
        .query("longitude", &lon.to_string())
        .query("current", "temperature_2m,weather_code")
        .query("daily", "temperature_2m_max,temperature_2m_min,precipitation_probability_max")
        .query("timezone", "auto")
        .query("forecast_days", "1")
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(Weather {
        place: place.to_string(),
        temp: v["current"]["temperature_2m"].as_f64().unwrap_or(0.0),
        hi: v["daily"]["temperature_2m_max"][0].as_f64().unwrap_or(0.0),
        lo: v["daily"]["temperature_2m_min"][0].as_f64().unwrap_or(0.0),
        rain: v["daily"]["precipitation_probability_max"][0].as_u64().unwrap_or(0) as u32,
        desc: describe(v["current"]["weather_code"].as_i64().unwrap_or(-1)),
    })
}

/// A stable fingerprint of a web page (FNV-1a over the first 2 MB), to notice changes.
pub fn page_fingerprint(url: &str) -> Result<u64, String> {
    let resp = agent()?.get(url).call().map_err(http_err)?;
    let mut body = Vec::new();
    resp.into_reader()
        .take(2_000_000)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in body {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Ok(h)
}

// ------------------------------------------------------------------ Gmail

pub struct Mail {
    pub id: String,
    pub from: String,
    pub subject: String,
}

/// Recent messages (last 2 days) that carry `label` (IMPORTANT, STARRED, INBOX or a label id).
pub fn recent_mail(cfg: &Config, label: &str) -> Result<Vec<Mail>, String> {
    let token = access_token(cfg)?;
    let bearer = format!("Bearer {token}");
    let ag = agent()?;
    let list: serde_json::Value = ag
        .get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
        .set("Authorization", &bearer)
        .query("labelIds", label)
        .query("q", "newer_than:2d")
        .query("maxResults", "10")
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;

    let ids: Vec<String> = list["messages"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();

    let mut out = Vec::new();
    for id in ids {
        let m: serde_json::Value = ag
            .get(&format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}"))
            .set("Authorization", &bearer)
            .query("format", "metadata")
            .query("metadataHeaders", "From")
            .query("metadataHeaders", "Subject")
            .call()
            .map_err(http_err)?
            .into_json()
            .map_err(|e| e.to_string())?;
        let header = |name: &str| -> String {
            m["payload"]["headers"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or(&[])
                .iter()
                .find(|h| h["name"].as_str().map_or(false, |n| n.eq_ignore_ascii_case(name)))
                .and_then(|h| h["value"].as_str())
                .unwrap_or("")
                .to_string()
        };
        out.push(Mail {
            id,
            from: header("From"),
            subject: header("Subject"),
        });
    }
    Ok(out)
}
