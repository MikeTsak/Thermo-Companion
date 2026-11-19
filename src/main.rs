#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod config;
mod dashboard;
mod data;
mod escpos;
mod gcal;
mod net;
mod pip;
mod printer;
mod raster;
mod sysinfo;

use app::{Notify, Shared};
use native_windows_gui as nwg;
use nwg::Event as E;
use std::cell::Cell;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::Arc;
use winapi::um::winuser::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, WM_HOTKEY};

pub const TITLE: &str = "Thermo Companion";
const HOTKEY_ID: i32 = 0x7C01;

/// Only one copy of a given kind (tray / dashboard) at a time.
fn already_running(name: &str) -> bool {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::synchapi::CreateMutexW;
    let wname: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // The handle is intentionally never closed: it lives as long as the process.
        CreateMutexW(null_mut(), 0, wname.as_ptr());
        GetLastError() == 183 // ERROR_ALREADY_EXISTS
    }
}

fn open_dashboard(page: Option<&str>) {
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--dashboard");
        if let Some(p) = page {
            cmd.arg(p);
        }
        let _ = cmd.spawn();
    }
}

// -------------------------------------------------------------------- tray

struct Tray {
    shared: Arc<Shared>,
    notify: nwg::Notice,
    host: nwg::MessageWindow,
    tray: nwg::TrayNotification,
    menu: nwg::Menu,
    mi_open: nwg::MenuItem,
    mi_agenda: nwg::MenuItem,
    mi_briefing: nwg::MenuItem,
    mi_eod: nwg::MenuItem,
    mi_clip: nwg::MenuItem,
    mi_focus: nwg::MenuItem,
    mi_test: nwg::MenuItem,
    t_agenda: nwg::MenuItem,
    t_briefing: nwg::MenuItem,
    t_stubs: nwg::MenuItem,
    t_hotkey: nwg::MenuItem,
    t_startup: nwg::MenuItem,
    mi_quit: nwg::MenuItem,
    _seps: Vec<nwg::MenuSeparator>,
    _icon: nwg::Icon,
    hotkey_on: Cell<bool>,
}

fn build(shared: Arc<Shared>) -> Result<Tray, nwg::NwgError> {
    let mut icon = nwg::Icon::default();
    nwg::Icon::builder().source_bin(Some(pip::ICON)).build(&mut icon)?;

    let mut host = nwg::MessageWindow::default();
    nwg::MessageWindow::builder().build(&mut host)?;

    let mut notify = nwg::Notice::default();
    nwg::Notice::builder().parent(&host).build(&mut notify)?;

    let mut tray = nwg::TrayNotification::default();
    nwg::TrayNotification::builder()
        .parent(&host)
        .icon(Some(&icon))
        .tip(Some(TITLE))
        .build(&mut tray)?;

    let mut menu = nwg::Menu::default();
    nwg::Menu::builder().popup(true).parent(&host).build(&mut menu)?;

    let item = |text: &str| -> Result<nwg::MenuItem, nwg::NwgError> {
        let mut m = nwg::MenuItem::default();
        nwg::MenuItem::builder().text(text).parent(&menu).build(&mut m)?;
        Ok(m)
    };
    let sep = || -> Result<nwg::MenuSeparator, nwg::NwgError> {
        let mut s = nwg::MenuSeparator::default();
        nwg::MenuSeparator::builder().parent(&menu).build(&mut s)?;
        Ok(s)
    };

    let mi_open = item("Open Pip's dashboard")?;
    let sep1 = sep()?;
    let mi_agenda = item("Print today's agenda")?;
    let mi_briefing = item("Print morning briefing")?;
    let mi_eod = item("Print end-of-day receipt")?;
    let mi_clip = item("Print clipboard")?;
    let mi_focus = item("Start / stop focus timer")?;
    let mi_test = item("Print test slip")?;
    let sep2 = sep()?;
    let t_agenda = item("Agenda every morning")?;
    let t_briefing = item("Morning briefing")?;
    let t_stubs = item("Stub before events")?;
    let t_hotkey = item("Hotkey Ctrl+Alt+P prints selection")?;
    let t_startup = item("Start with Windows")?;
    let sep3 = sep()?;
    let mi_quit = item("Quit")?;

    Ok(Tray {
        shared,
        notify,
        host,
        tray,
        menu,
        mi_open,
        mi_agenda,
        mi_briefing,
        mi_eod,
        mi_clip,
        mi_focus,
        mi_test,
        t_agenda,
        t_briefing,
        t_stubs,
        t_hotkey,
        t_startup,
        mi_quit,
        _seps: vec![sep1, sep2, sep3],
        _icon: icon,
        hotkey_on: Cell::new(false),
    })
}

impl Tray {
    fn notify_fn(&self) -> Notify {
        let sender = self.notify.sender();
        Arc::new(move || sender.notice())
    }

    fn say(&self, msg: &str) {
        self.tray.show(msg, Some(TITLE), Some(nwg::TrayNotificationFlags::INFO_ICON), None);
    }

    fn sync_menu(&self) {
        let c = self.shared.cfg();
        self.t_agenda.set_checked(c.auto_agenda);
        self.t_briefing.set_checked(c.briefing);
        self.t_stubs.set_checked(c.event_stubs);
        self.t_hotkey.set_checked(c.hotkey);
        self.t_startup.set_checked(c.start_with_windows);
    }

    /// Register or release Ctrl+Alt+P to match the setting.
    fn apply_hotkey(&self) {
        let want = self.shared.cfg().hotkey;
        if want == self.hotkey_on.get() {
            return;
        }
        if let Some(hwnd) = self.host.handle.hwnd() {
            unsafe {
                if want {
                    let ok = RegisterHotKey(hwnd as _, HOTKEY_ID, (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT) as u32, b'P' as u32);
                    self.hotkey_on.set(ok != 0);
                    if ok == 0 {
                        config::log("Ctrl+Alt+P is already taken by another program");
                    }
                } else {
                    UnregisterHotKey(hwnd as _, HOTKEY_ID);
                    self.hotkey_on.set(false);
                }
            }
        }
    }

    fn flush(&self) {
        for m in self.shared.take_msgs() {
            self.say(&m);
        }
        self.apply_hotkey();
        self.sync_menu();
    }

    fn toggle(&self, f: impl FnOnce(&mut config::Config)) {
        let mut c = self.shared.cfg();
        f(&mut c);
        if let Err(e) = autostart::set(c.start_with_windows) {
            self.say(&format!("Couldn't change start-with-Windows: {e}"));
        }
        self.shared.set_cfg(c);
        self.apply_hotkey();
    }

    fn toggle_focus(&self) {
        let cfg = self.shared.cfg();
        let now = chrono::Local::now();
        data::update(|d| {
            if d.pomodoro.running {
                d.pomodoro.running = false;
            } else {
                d.pomodoro.running = true;
                d.pomodoro.phase = "work".into();
                d.pomodoro.ends_at = (now + chrono::Duration::minutes(cfg.pomo_work as i64)).to_rfc3339();
            }
        });
        let running = data::load().pomodoro.running;
        self.say(if running {
            "Focus timer started. Go!"
        } else {
            "Focus timer stopped."
        });
    }
}

fn on_event(t: &Rc<Tray>, evt: E, handle: nwg::ControlHandle) {
    match evt {
        E::OnContextMenu if handle == t.tray.handle => {
            t.sync_menu();
            let (x, y) = nwg::GlobalCursor::position();
            t.menu.popup(x, y);
        }
        E::OnMousePress(nwg::MousePressEvent::MousePressLeftUp) if handle == t.tray.handle => {
            open_dashboard(None);
        }
        E::OnNotice if handle == t.notify.handle => t.flush(),
        E::OnMenuItemSelected => {
            let sh = &t.shared;
            if handle == t.mi_quit.handle {
                nwg::stop_thread_dispatch();
            } else if handle == t.mi_open.handle {
                open_dashboard(None);
            } else if handle == t.mi_agenda.handle {
                app::spawn_job(sh, t.notify_fn(), "Printed today's agenda", |s| s.print_agenda(true));
            } else if handle == t.mi_briefing.handle {
                app::spawn_job(sh, t.notify_fn(), "Printed your morning briefing", |s| s.print_briefing());
            } else if handle == t.mi_eod.handle {
                app::spawn_job(sh, t.notify_fn(), "Printed your end-of-day receipt", |s| s.print_end_of_day());
            } else if handle == t.mi_clip.handle {
                app::spawn_job(sh, t.notify_fn(), "Printed the clipboard", |s| {
                    let text = sysinfo::clipboard_text()
                        .filter(|x| !x.trim().is_empty())
                        .ok_or("the clipboard has no text")?;
                    let o = escpos::TextOpts { bold: false, big: false, center: false, stamp: true, checklist: false };
                    s.print(escpos::text_slip(&s.cfg(), &text, &o))
                });
            } else if handle == t.mi_focus.handle {
                t.toggle_focus();
            } else if handle == t.mi_test.handle {
                app::spawn_job(sh, t.notify_fn(), "Test slip printed", |s| s.print(escpos::test_slip(&s.cfg())));
            } else if handle == t.t_agenda.handle {
                t.toggle(|c| c.auto_agenda = !c.auto_agenda);
            } else if handle == t.t_briefing.handle {
                t.toggle(|c| c.briefing = !c.briefing);
            } else if handle == t.t_stubs.handle {
                t.toggle(|c| c.event_stubs = !c.event_stubs);
            } else if handle == t.t_hotkey.handle {
                t.toggle(|c| c.hotkey = !c.hotkey);
            } else if handle == t.t_startup.handle {
                t.toggle(|c| c.start_with_windows = !c.start_with_windows);
            }
        }
        _ => {}
    }
}

fn run_tray() {
    if already_running("ThermoCompanionSingleton") {
        return;
    }
    nwg::init().expect("failed to initialise the Windows UI");
    let _ = nwg::Font::set_global_family("Segoe UI");

    let (mut cfg, first_run) = config::load();
    if first_run {
        if let Some(p) = printer::guess_receipt_printer(&printer::list_printers()) {
            cfg.printer = p;
        }
        // The installer passes --no-autostart when the user unticked "Start with Windows".
        if std::env::args().any(|a| a == "--no-autostart") {
            cfg.start_with_windows = false;
        }
        config::save(&cfg);
        if cfg.start_with_windows {
            if let Err(e) = autostart::set(true) {
                config::log(&format!("autostart: {e}"));
            }
        }
    }

    let shared = Shared::new(cfg);
    let tray = Rc::new(build(shared.clone()).expect("failed to build the tray icon"));

    let t = tray.clone();
    let _handler = nwg::full_bind_event_handler(&tray.host.handle, move |e, _d, h| on_event(&t, e, h));

    // Ctrl+Alt+P: print what is selected.
    let t = tray.clone();
    let _raw = nwg::bind_raw_event_handler(&tray.host.handle, 0x10000, move |_hwnd, msg, w, _l| {
        if msg == WM_HOTKEY && w as i32 == HOTKEY_ID {
            app::spawn_job(&t.shared, t.notify_fn(), "Printed your selection", |s| app::hotkey_print(s).map(|_| ()));
        }
        None
    })
    .expect("hotkey handler");

    tray.apply_hotkey();
    tray.sync_menu();

    {
        let sh = shared.clone();
        let notify = tray.notify_fn();
        std::thread::spawn(move || app::scheduler(sh, notify));
    }

    if first_run {
        tray.say("Hi, I'm Pip! I live in your tray. Opening settings so you can pick your printer.");
        open_dashboard(Some("settings"));
    }

    nwg::dispatch_thread_events();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // `thermo-companion.exe --test-print`: print one test slip and exit (result goes to log.txt).
    if args.iter().any(|a| a == "--test-print") {
        let (cfg, _) = config::load();
        let result = printer::print_raw(&cfg.printer, &escpos::test_slip(&cfg));
        config::log(&match &result {
            Ok(()) => format!("test print sent to \"{}\"", cfg.printer),
            Err(e) => format!("test print failed: {e}"),
        });
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    if args.iter().any(|a| a == "--dashboard") {
        if already_running("ThermoCompanionDashboard") {
            sysinfo::focus_window(dashboard::WINDOW_TITLE);
            return;
        }
        let page = args.iter().skip_while(|a| *a != "--dashboard").nth(1).cloned();
        dashboard::run(page);
        return;
    }

    run_tray();
}
