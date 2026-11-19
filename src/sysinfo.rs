//! Small Windows sensors and helpers: battery, disk, idle time, foreground program,
//! running programs, clipboard, window focus.

use std::collections::HashSet;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use winapi::shared::minwindef::DWORD;
use winapi::shared::ntdef::ULARGE_INTEGER;
use winapi::um::fileapi::GetDiskFreeSpaceExW;
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::processthreadsapi::OpenProcess;
use winapi::um::sysinfoapi::GetTickCount;
use winapi::um::tlhelp32::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use winapi::um::winbase::{GetSystemPowerStatus, GlobalLock, GlobalUnlock, QueryFullProcessImageNameW, SYSTEM_POWER_STATUS};
use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
use winapi::um::winuser::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn utf16_until_nul(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// (percent, plugged in). None on desktops without a battery.
pub fn battery() -> Option<(u8, bool)> {
    unsafe {
        let mut s: SYSTEM_POWER_STATUS = zeroed();
        if GetSystemPowerStatus(&mut s) == 0 || s.BatteryLifePercent > 100 {
            return None;
        }
        Some((s.BatteryLifePercent, s.ACLineStatus == 1))
    }
}

/// (percent free, GB free) of a drive such as "C:\".
pub fn disk_free(root: &str) -> Option<(f64, f64)> {
    unsafe {
        let w = wide(root);
        let mut free: ULARGE_INTEGER = zeroed();
        let mut total: ULARGE_INTEGER = zeroed();
        let mut total_free: ULARGE_INTEGER = zeroed();
        if GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) == 0 {
            return None;
        }
        let (f, t) = (*free.QuadPart() as f64, *total.QuadPart() as f64);
        if t <= 0.0 {
            return None;
        }
        Some((f / t * 100.0, f / 1e9))
    }
}

pub fn idle_seconds() -> u64 {
    unsafe {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut info) == 0 {
            return 0;
        }
        (GetTickCount().wrapping_sub(info.dwTime) / 1000) as u64
    }
}

/// Name (without .exe) of the program that has the focused window.
pub fn foreground_program() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: DWORD = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 520];
        let mut len = buf.len() as DWORD;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let path = utf16_until_nul(&buf);
        std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
    }
}

/// Lower-case names (without .exe) of all running programs.
pub fn running_programs() -> HashSet<String> {
    let mut set = HashSet::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return set;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let name = utf16_until_nul(&e.szExeFile).to_lowercase();
                set.insert(name.trim_end_matches(".exe").to_string());
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    set
}

pub fn clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}

fn key_down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

/// Wait (up to `ms`) until Ctrl, Alt and P are released, so a synthetic Ctrl+C isn't mangled.
pub fn wait_hotkey_released(ms: u64) {
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < ms as u128
        && (key_down(VK_CONTROL) || key_down(VK_MENU) || key_down(b'P' as i32))
    {
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// Press Ctrl+C in whatever window has focus.
pub fn send_copy() {
    unsafe {
        keybd_event(VK_CONTROL as u8, 0, 0, 0);
        keybd_event(b'C', 0, 0, 0);
        keybd_event(b'C', 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

/// Bring the window with this exact title to the front. Returns whether it was found.
pub fn focus_window(title: &str) -> bool {
    unsafe {
        let w = wide(title);
        let hwnd = FindWindowW(null_mut(), w.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        true
    }
}
