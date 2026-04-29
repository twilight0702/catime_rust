//! Catime Win32 原生 GUI 入口。
//! 使用 raw Win32 API (`windows` crate) 创建窗口、系统托盘和消息循环。
//! 不使用任何 UI 框架，直接调用 GDI 渲染。

// 仅在 release 构建中使用 windows 子系统，debug/cargo run 时使用 console 子系统以支持 Ctrl+C
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, HBRUSH, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForSystem, SetProcessDpiAwareness, MDT_EFFECTIVE_DPI,
    PROCESS_PER_MONITOR_DPI_AWARE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use window::{wndproc, AppState};

/// Ctrl+C → 优雅退出 的自定义窗口消息
pub const WM_CTRLC_SHUTDOWN: u32 = WM_USER + 1;

const ERROR_LOG_FILE: &str = "catime_error.log";

/// 将错误/信息写入可执行文件同目录的 `catime_error.log`。
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

/// 安装全局 panic hook：捕获 panic 后写入日志文件和 `catime_error.log`。
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

fn dpi_for_point(x: i32, y: i32) -> u32 {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return GetDpiForSystem();
        }
        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() {
            dpi_x.max(1)
        } else {
            GetDpiForSystem()
        }
    }
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

    // 加载配置文件
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

    // 创建命令通道
    let (tx, rx) = mpsc::channel::<timer_core::AppCommand>();

    let mut state = Box::new(AppState {
        controller,
        rx,
        render,
        last_tick: Instant::now(),
    });

    // 按配置决定是否启动热更新监听
    if state.controller.config().hot_reload {
        watcher::spawn_watcher(config_path, tx.clone());
    } else {
        log::info!("hot-reload disabled by config");
    }

    let instance = unsafe { GetModuleHandleW(None).unwrap() };

    // 注册窗口类
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_HAND).unwrap() },
        // 背景由 WM_PAINT + 双缓冲统一绘制，hbrBackground 设为 null 避免闪烁
        hbrBackground: HBRUSH(null_mut()),
        lpszClassName: w!("CATIME_WINDOW"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };

    let (init_x, init_y, init_w, init_h, normalize_window_config) = {
        let cfg = state.controller.config();
        let target_dpi = dpi_for_point(cfg.window.x, cfg.window.y).max(1);
        let source_dpi = cfg.window.dpi.unwrap_or(target_dpi).max(1);
        let scaled_w = ((cfg.window.width as u64) * (target_dpi as u64) / (source_dpi as u64))
            .max(1)
            .min(i32::MAX as u64) as i32;
        let scaled_h = ((cfg.window.height as u64) * (target_dpi as u64) / (source_dpi as u64))
            .max(1)
            .min(i32::MAX as u64) as i32;
        let min_w = win_w.max(1);
        let min_h = win_h.max(1);
        let req_w = scaled_w;
        let req_h = scaled_h;
        let w = if cfg.window.width == 0 {
            min_w
        } else {
            req_w.max(min_w)
        };
        let h = if cfg.window.height == 0 {
            min_h
        } else {
            req_h.max(min_h)
        };
        let normalized = req_w != w || req_h != h || cfg.window.dpi != Some(target_dpi);
        (cfg.window.x, cfg.window.y, w, h, normalized)
    };

    // 创建主窗口（优先使用配置中的窗口位置与尺寸）
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW, // 置顶 + 不在任务栏显示
            w!("CATIME_WINDOW"),
            w!("Catime"),
            WS_POPUP, // 无标题栏、无边框
            init_x,
            init_y,
            init_w,
            init_h,
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

    // 旧版本过小窗口尺寸（如 300x120）迁移为当前最小可用尺寸并持久化。
    if normalize_window_config {
        state.controller.update_window_bounds(
            init_x,
            init_y,
            init_w as u32,
            init_h as u32,
            Some(dpi_for_point(init_x, init_y)),
        );
        if let Err(e) = state.controller.save_config() {
            log::error!("save normalized window size failed: {}", e);
        }
    }

    // 创建系统托盘图标
    let _ = tray::create_tray(hwnd, state.controller.config().tray.show_remaining_tooltip);

    // 注册 Ctrl+C 处理器，通过自定义消息优雅退出
    let hwnd_val = hwnd.0 as isize;
    ctrlc::set_handler(move || {
        log::info!("Ctrl+C received, shutting down gracefully");
        unsafe {
            PostMessageW(
                HWND(hwnd_val as *mut _),
                WM_CTRLC_SHUTDOWN,
                WPARAM(0),
                LPARAM(0),
            )
        };
    })
    .expect("failed to register Ctrl+C handler");

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
    }

    // Windows 消息主循环
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
