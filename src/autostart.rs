//! "Start with Windows" through the per-user Run registry key (no admin needed).

use winreg::enums::*;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ThermoCompanion";

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
