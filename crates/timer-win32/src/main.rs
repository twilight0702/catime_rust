#![windows_subsystem = "windows"]
#![allow(unused_must_use)]

mod countdown_dialog;
mod render;
mod tray;
mod watcher;
mod window;

use std::io::Write;
use std::sync::mpsc;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use std::ptr::null_mut;
use timer_app::AppController;
use timer_storage::{ConfigRepository, TomlConfigRepository};
use windows::core::w;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use window::{wndproc, AppState};

const ERROR_LOG_FILE: &str = "catime_error.log";

fn append_error_file(level: &str, msg: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let path = match std::env::current_exe() {
        Ok(mut p) => {
            p.pop();
            p.push(ERROR_LOG_FILE);
            p
        }
        Err(_) => return,
    };

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[{}][{}] {}", ts, level, msg);
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown-location".to_string());
        let msg = format!("panic at {} -> {}", location, payload);
        append_error_file("PANIC", &msg);
        log::error!("{}", msg);
    }));
}

fn main() {
    append_error_file("INFO", "catime startup");
    install_panic_hook();

    // 声明 DPI 感知，避免高 DPI 屏幕上 Windows 拉伸窗口导致模糊
    let _ = unsafe { SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) };
    // 根据屏幕 DPI 缩放窗口尺寸，基础值 320×220 对应 96 DPI
    let dpi = unsafe { GetDpiForSystem() } as i32;
    let win_w = 320 * dpi / 96;
    let win_h = 220 * dpi / 96;

    env_logger::init();

    let config_path = match TomlConfigRepository::default_path() {
        Ok(p) => p,
        Err(e) => {
            log::error!("config path: {}", e);
            return;
        }
    };
    log::info!("config path: {}", config_path.display());

    let config_repo = TomlConfigRepository::new(config_path.clone());
    let config = match config_repo.load() {
        Ok(c) => c,
        Err(e) => {
            log::error!("load config: {}", e);
            return;
        }
    };

    let controller = AppController::new(config, Box::new(config_repo));
    let render = render::RenderContext::new();

    let (tx, rx) = mpsc::channel::<timer_core::AppCommand>();

    let mut state = Box::new(AppState {
        controller,
        rx,
        render,
        last_tick: Instant::now(),
    });

    watcher::spawn_watcher(config_path, tx.clone());

    let instance = unsafe { GetModuleHandleW(None).unwrap() };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_HAND).unwrap() },
        // 背景改由 WM_PAINT + 双缓冲统一绘制，避免系统先擦背景导致闪烁
        hbrBackground: HBRUSH(null_mut()),
        lpszClassName: w!("CATIME_WINDOW"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            w!("CATIME_WINDOW"),
            w!("Catime"),
            WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            win_w,
            win_h,
            None,
            None,
            instance,
            Some(state.as_mut() as *mut AppState as _),
        )
    };
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            log::error!("create window: {}", e);
            return;
        }
    };

    let _ = tray::create_tray(hwnd);

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
    }

    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
