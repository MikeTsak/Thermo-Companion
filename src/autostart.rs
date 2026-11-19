//! "Start with Windows" through the per-user Run registry key (no admin needed).

use winreg::enums::*;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ThermoCompanion";
const CONTEXT_KEY: &str = r"Software\Classes\*\shell\ThermoPrint";
const DIR_CONTEXT_KEY: &str = r"Software\Classes\Directory\shell\ThermoPrint";

pub fn is_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
        .map_or(false, |v| !v.trim().is_empty())
}

pub fn set(enabled: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        // Deleting a value that isn't there is fine.
        let _ = key.delete_value(VALUE_NAME);
        Ok(())
    }
}

pub fn context_menu_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(CONTEXT_KEY, KEY_READ).is_ok()
}

fn sendto_lnk_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| {
        std::path::PathBuf::from(a)
            .join("Microsoft")
            .join("Windows")
            .join("SendTo")
            .join("Thermo Print.lnk")
    })
}

fn write_shell_verb(hkcu: &RegKey, subkey: &str, exe: &std::path::Path, icon: &str) -> Result<(), String> {
    let (shell_key, _) = hkcu.create_subkey(subkey).map_err(|e| e.to_string())?;
    shell_key.set_value("", &"Thermo Print").map_err(|e| e.to_string())?;
    shell_key.set_value("Icon", &icon).map_err(|e| e.to_string())?;
    let (cmd_key, _) = shell_key.create_subkey("command").map_err(|e| e.to_string())?;
    cmd_key
        .set_value("", &format!("\"{}\" --print-file \"%1\"", exe.display()))
        .map_err(|e| e.to_string())
}

fn set_sendto_shortcut(enabled: bool, exe: &std::path::Path, icon_path: &str) {
    let Some(lnk) = sendto_lnk_path() else { return };
    if !enabled {
        let _ = std::fs::remove_file(&lnk);
        return;
    }
    if let Some(parent) = lnk.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let esc = |s: &str| s.replace('\'', "''");
    let ps = format!(
        "$ws=New-Object -ComObject WScript.Shell;\
         $s=$ws.CreateShortcut('{}');\
         $s.TargetPath='{}';\
         $s.Arguments='--print-file';\
         $s.IconLocation='{}';\
         $s.Save()",
        esc(&lnk.to_string_lossy()),
        esc(&exe.to_string_lossy()),
        esc(icon_path),
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

/// Add or remove "Thermo Print" in Windows Explorer's file/folder right-click menu and SendTo (per-user, no admin needed).
pub fn set_context_menu(enabled: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let raw_icon = exe
            .parent()
            .map(|d| d.join("icon.ico"))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{},0", exe.display()));
        let quoted_icon = if raw_icon.ends_with(",0") {
            let p = raw_icon.trim_end_matches(",0");
            format!("\"{p}\",0")
        } else {
            format!("\"{raw_icon}\"")
        };

        write_shell_verb(&hkcu, CONTEXT_KEY, &exe, &quoted_icon)?;
        write_shell_verb(&hkcu, DIR_CONTEXT_KEY, &exe, &quoted_icon)?;
        set_sendto_shortcut(true, &exe, &raw_icon);
        Ok(())
    } else {
        let _ = hkcu.delete_subkey_all(CONTEXT_KEY);
        let _ = hkcu.delete_subkey_all(DIR_CONTEXT_KEY);
        set_sendto_shortcut(false, std::path::Path::new(""), "");
        Ok(())
    }
}
