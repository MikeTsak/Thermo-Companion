//! Check GitHub Releases for newer versions and apply self-updates on Windows.

use crate::gcal::http_err;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

pub const REPO: &str = "MikeTsak/Thermo-Companion";
pub const RELEASES_URL: &str = "https://github.com/MikeTsak/Thermo-Companion/releases";
const API_LATEST: &str = "https://api.github.com/repos/MikeTsak/Thermo-Companion/releases/latest";

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub title: String,
    pub notes: String,
    pub html_url: String,
    pub setup_asset: Option<(String, u64)>,
    pub exe_asset: Option<(String, u64)>,
}

#[derive(Clone, Debug)]
pub enum CheckOutcome {
    UpToDate { latest: Option<String>, message: String },
    Available(Release),
}

fn http_agent(timeout_secs: u64) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(timeout_secs))
        .build())
}

fn user_agent() -> &'static str {
    concat!("thermo-companion/", env!("CARGO_PKG_VERSION"), " (+https://miketsak.gr)")
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
    /// Stable releases sort higher than pre-releases for the same major.minor.patch.
    stable: bool,
}

fn parse_semver(raw: &str) -> Option<Semver> {
    let s = raw.trim().trim_start_matches(['v', 'V']);
    let s = s.split('+').next().unwrap_or(s);
    let (core, stable) = match s.split_once('-') {
        Some((c, _)) => (c, false),
        None => (s, true),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some(Semver { major, minor, patch, stable })
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn parse_release(v: &serde_json::Value) -> Result<Release, String> {
    let tag = v["tag_name"]
        .as_str()
        .ok_or("GitHub response had no tag_name")?
        .to_string();
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let title = v["name"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&tag)
        .to_string();
    let notes = v["body"].as_str().unwrap_or("").trim().to_string();
    let html_url = v["html_url"].as_str().unwrap_or(RELEASES_URL).to_string();

    let mut setup_asset = None;
    let mut exe_asset = None;
    if let Some(assets) = v["assets"].as_array() {
        for a in assets {
            let Some(name) = a["name"].as_str() else { continue };
            let Some(url) = a["browser_download_url"].as_str() else { continue };
            let size = a["size"].as_u64().unwrap_or(0);
            let lower = name.to_ascii_lowercase();
            if lower.starts_with("thermocompanion-setup-") && lower.ends_with(".exe") {
                setup_asset = Some((url.to_string(), size));
            } else if lower == "thermo-companion.exe" {
                exe_asset = Some((url.to_string(), size));
            }
        }
    }

    Ok(Release {
        tag,
        version,
        title,
        notes,
        html_url,
        setup_asset,
        exe_asset,
    })
}

/// Query GitHub Releases for the latest release and compare with `CARGO_PKG_VERSION`.
pub fn check_latest() -> Result<CheckOutcome, String> {
    let ag = http_agent(20)?;
    let resp = ag
        .get(API_LATEST)
        .set("User-Agent", user_agent())
        .set("Accept", "application/vnd.github+json")
        .call();

    let v: serde_json::Value = match resp {
        Ok(r) => r.into_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => {
            return Ok(CheckOutcome::UpToDate {
                latest: None,
                message: "No releases published on GitHub yet.".into(),
            });
        }
        Err(ureq::Error::Status(403, _)) | Err(ureq::Error::Status(429, _)) => {
            return Err("GitHub rate limit reached. Try again in a bit.".into());
        }
        Err(e) => return Err(http_err(e)),
    };

    let rel = parse_release(&v)?;
    let current = env!("CARGO_PKG_VERSION");
    if is_newer(&rel.version, current) {
        Ok(CheckOutcome::Available(rel))
    } else {
        Ok(CheckOutcome::UpToDate {
            latest: Some(rel.tag.clone()),
            message: format!("You're on the latest version (v{current})."),
        })
    }
}

fn download_exe(url: &str, expected_size: u64, dest: &Path) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Refusing non-HTTPS release download URL.".into());
    }
    let ag = http_agent(120)?;
    let resp = ag
        .get(url)
        .set("User-Agent", user_agent())
        .call()
        .map_err(http_err)?;

    let mut bytes = Vec::new();
    // Cap at 100 MB so a malformed response cannot exhaust memory.
    resp.into_reader()
        .take(100 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;

    if bytes.len() < 64 * 1024 || !bytes.starts_with(b"MZ") {
        return Err("Downloaded file is not a valid Windows executable.".into());
    }
    if expected_size > 0 && bytes.len() as u64 != expected_size {
        return Err(format!(
            "Incomplete download ({} of {} bytes).",
            bytes.len(),
            expected_size
        ));
    }
    fs::write(dest, &bytes).map_err(|e| format!("Failed to save update: {e}"))
}

fn is_inno_installed(exe_path: &Path) -> bool {
    exe_path
        .parent()
        .map_or(false, |dir| dir.join("unins000.exe").exists())
}

fn spawn_detached_cmd(script_path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd.exe")
            .args(["/C", &script_path.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to launch updater: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = script_path;
        Err("Self-update is only supported on Windows.".into())
    }
}

fn build_setup_script(setup_path: &Path, target_exe: &Path) -> String {
    format!(
        "@echo off\r\n\
         set \"SETUP={}\"\r\n\
         set \"APP={}\"\r\n\
         ping -n 2 127.0.0.1 >nul\r\n\
         taskkill /F /IM thermo-companion.exe >nul 2>&1\r\n\
         ping -n 2 127.0.0.1 >nul\r\n\
         \"%SETUP%\" /SILENT /NORESTART\r\n\
         del /Q \"%SETUP%\" >nul 2>&1\r\n\
         if exist \"%APP%\" (\r\n\
           start \"\" \"%APP%\"\r\n\
           ping -n 2 127.0.0.1 >nul\r\n\
           start \"\" \"%APP%\" --dashboard settings\r\n\
         )\r\n\
         del /Q \"%~f0\" >nul 2>&1\r\n",
        setup_path.display(),
        target_exe.display()
    )
}

fn build_portable_script(new_exe: &Path, target_exe: &Path) -> String {
    format!(
        "@echo off\r\n\
         set \"SRC={}\"\r\n\
         set \"DST={}\"\r\n\
         ping -n 2 127.0.0.1 >nul\r\n\
         taskkill /F /IM thermo-companion.exe >nul 2>&1\r\n\
         for /L %%i in (1,1,15) do (\r\n\
           move /Y \"%SRC%\" \"%DST%\" >nul 2>&1 && goto done\r\n\
           ping -n 2 127.0.0.1 >nul\r\n\
         )\r\n\
         :done\r\n\
         if exist \"%DST%\" (\r\n\
           start \"\" \"%DST%\"\r\n\
           ping -n 2 127.0.0.1 >nul\r\n\
           start \"\" \"%DST%\" --dashboard settings\r\n\
         )\r\n\
         del /Q \"%~f0\" >nul 2>&1\r\n",
        new_exe.display(),
        target_exe.display()
    )
}

/// Download the selected release asset and spawn a detached updater script.
/// When this returns `Ok(())`, the caller should exit immediately so the executable is unlocked.
pub fn download_and_apply(rel: &Release) -> Result<(), String> {
    let target_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let temp_dir: PathBuf = std::env::temp_dir();
    let script_path = temp_dir.join("thermo-companion-updater.cmd");

    let script = if is_inno_installed(&target_exe) {
        if let Some((url, size)) = &rel.setup_asset {
            let setup_path = temp_dir.join(format!("ThermoCompanion-Setup-{}.exe", rel.version));
            download_exe(url, *size, &setup_path)?;
            build_setup_script(&setup_path, &target_exe)
        } else if let Some((url, size)) = &rel.exe_asset {
            let new_exe = temp_dir.join("thermo-companion-update.exe");
            download_exe(url, *size, &new_exe)?;
            build_portable_script(&new_exe, &target_exe)
        } else {
            return Err("No Windows release asset found. Download it from GitHub instead.".into());
        }
    } else if let Some((url, size)) = &rel.exe_asset {
        let new_exe = temp_dir.join("thermo-companion-update.exe");
        download_exe(url, *size, &new_exe)?;
        build_portable_script(&new_exe, &target_exe)
    } else if let Some((url, size)) = &rel.setup_asset {
        let setup_path = temp_dir.join(format!("ThermoCompanion-Setup-{}.exe", rel.version));
        download_exe(url, *size, &setup_path)?;
        build_setup_script(&setup_path, &target_exe)
    } else {
        return Err("No Windows release asset found. Download it from GitHub instead.".into());
    };

    fs::write(&script_path, script).map_err(|e| format!("Failed to write updater script: {e}"))?;
    spawn_detached_cmd(&script_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison() {
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("0.3.0", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.2.0-rc.1"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("v0.2.0-beta.1", "0.2.0"));
    }

    #[test]
    fn parses_release_assets() {
        let json: serde_json::Value = serde_json::json!({
            "tag_name": "v0.2.1",
            "name": "v0.2.1",
            "body": "Bug fixes",
            "html_url": "https://github.com/MikeTsak/Thermo-Companion/releases/tag/v0.2.1",
            "assets": [
                {
                    "name": "ThermoCompanion-Setup-0.2.1.exe",
                    "browser_download_url": "https://github.com/MikeTsak/Thermo-Companion/releases/download/v0.2.1/ThermoCompanion-Setup-0.2.1.exe",
                    "size": 4100000
                },
                {
                    "name": "thermo-companion.exe",
                    "browser_download_url": "https://github.com/MikeTsak/Thermo-Companion/releases/download/v0.2.1/thermo-companion.exe",
                    "size": 6100000
                }
            ]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, "0.2.1");
        assert!(r.setup_asset.is_some());
        assert!(r.exe_asset.is_some());
    }
}
