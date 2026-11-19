//! Raw printing through the Windows spooler (winspool).
//! The bytes are ESC/POS; the spooler passes them to the printer untouched.

use std::ptr::null_mut;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::DWORD;
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::winnt::HANDLE;
use winapi::um::winspool::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

/// Names of the printers installed on this PC.
pub fn list_printers() -> Vec<String> {
    unsafe {
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let mut needed: DWORD = 0;
        let mut returned: DWORD = 0;
        EnumPrintersW(flags, null_mut(), 4, null_mut(), 0, &mut needed, &mut returned);
        if needed == 0 {
            return Vec::new();
        }
        // u64 elements keep the buffer 8-byte aligned for the pointers inside.
        let mut buf: Vec<u64> = vec![0; (needed as usize + 7) / 8];
        let ok = EnumPrintersW(
            flags,
            null_mut(),
            4,
            buf.as_mut_ptr() as *mut u8,
            needed,
            &mut needed,
            &mut returned,
        );
        if ok == 0 {
            return Vec::new();
        }
        let infos =
            std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, returned as usize);
        infos.iter().map(|i| from_wide_ptr(i.pPrinterName)).collect()
    }
}

/// Guess which installed printer is the receipt printer.
pub fn guess_receipt_printer(names: &[String]) -> Option<String> {
    names
        .iter()
        .find(|n| {
            let l = n.to_lowercase();
            l.contains("receipt") || l.contains("tm-t") || l.contains("pos") || l.contains("thermal")
        })
        .cloned()
}

/// Send raw bytes to the named printer.
pub fn print_raw(name: &str, data: &[u8]) -> Result<(), String> {
    unsafe {
        let mut wname = wide(name);
        let mut handle: HANDLE = null_mut();
        if OpenPrinterW(wname.as_mut_ptr(), &mut handle, null_mut()) == 0 {
            return Err(format!(
                "Can't open printer \"{name}\" (Windows error {}). Is it plugged in and switched on?",
                GetLastError()
            ));
        }

        let mut doc_name = wide("Thermo Companion");
        let mut datatype = wide("RAW");
        let mut doc = DOC_INFO_1W {
            pDocName: doc_name.as_mut_ptr(),
            pOutputFile: null_mut(),
            pDatatype: datatype.as_mut_ptr(),
        };

        if StartDocPrinterW(handle, 1, &mut doc as *mut DOC_INFO_1W as *mut u8) == 0 {
            let err = GetLastError();
            ClosePrinter(handle);
            return Err(format!("Printer refused the job (Windows error {err})."));
        }
        StartPagePrinter(handle);

        let mut written: DWORD = 0;
        let ok = WritePrinter(
            handle,
            data.as_ptr() as *mut c_void,
            data.len() as DWORD,
            &mut written,
        );
        let err = GetLastError();

        EndPagePrinter(handle);
        EndDocPrinter(handle);
        ClosePrinter(handle);

        if ok == 0 || written as usize != data.len() {
            return Err(format!("Sending data to the printer failed (Windows error {err})."));
        }
        Ok(())
    }
}
