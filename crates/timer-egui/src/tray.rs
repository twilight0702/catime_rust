//! 系统托盘图标模块：托盘右键菜单 + 左键点击事件。

use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::Context;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

use timer_core::AppCommand;

use crate::ui_command::UiCommand;

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
        let _ = std::io::Write::write_all(
            &mut f,
            format!("[{}][{}][tray-egui] {}\n", ts, level, msg).as_bytes(),
        );
    }
}

#[cfg(windows)]
use windows::core::w;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, FindWindowW, SetForegroundWindow, ShowWindow, SW_SHOWNORMAL,
};

#[cfg(windows)]
fn wake_native_window() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(None, w!("Catime")) {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn wake_window_for_dialog(ctx: &Context) {
    #[cfg(windows)]
    wake_native_window();

    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
    ctx.request_repaint_after(std::time::Duration::from_millis(16));
}

/// 从 `assets/icon.ico` 编译期嵌入图标并解码为 RGBA 格式。
/// 使用 `include_bytes!` 在编译时将图标数据嵌入二进制。
fn create_icon() -> Icon {
    let icon_bytes = include_bytes!("../../../assets/icon.ico");
    let img = image::load_from_memory(icon_bytes)
        .expect("failed to decode icon.ico")
        .into_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();

    Icon::from_rgba(rgba, width, height).expect("failed to create tray icon from rgba")
}

/// 创建托盘图标并设置事件回调。
/// 必须在主线程调用以共享 Windows 消息泵。
/// 返回的 `TrayIcon` 句柄须保持存活（`Box::leak`），否则图标会消失。
pub fn create_tray(
    tx: mpsc::Sender<UiCommand>,
    repaint_ctx: Context,
    show_tooltip: bool,
) -> tray_icon::TrayIcon {
    let icon = create_icon();

    // 构建右键菜单
    let menu = Menu::new();
    let item_start = MenuItem::new("开始", true, None);
    let item_pause = MenuItem::new("暂停", true, None);
    let item_reset = MenuItem::new("重置", true, None);
    let item_stopwatch = MenuItem::new("正计时", true, None);
    let item_countdown = MenuItem::new("倒计时", true, None);
    let item_set_countdown = MenuItem::new("设置倒计时...", true, None);
    let item_set_opacity = MenuItem::new("设置透明度...", true, None);
    let item_toggle = MenuItem::new("显示/隐藏", true, None);
    let item_quit = MenuItem::new("退出", true, None);

    menu.append(&item_start).ok();
    menu.append(&item_pause).ok();
    menu.append(&item_reset).ok();
    menu.append(&item_stopwatch).ok();
    menu.append(&item_countdown).ok();
    menu.append(&item_set_countdown).ok();
    menu.append(&item_set_opacity).ok();
    menu.append(&item_toggle).ok();
    menu.append(&item_quit).ok();

    let mut builder = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu));
    if show_tooltip {
        builder = builder.with_tooltip("Catime");
    }
    let tray = builder.build().expect("failed to create tray icon");

    // 托盘左键/双击 → 由控制器按配置决定行为
    let tx_click = tx.clone();
    let repaint_click = repaint_ctx.clone();
    TrayIconEvent::set_event_handler(Some(Box::new(move |event: TrayIconEvent| {
        if matches!(
            event,
            TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
        ) {
            let _ = tx_click.send(UiCommand::Core(AppCommand::TrayLeftClick));
            repaint_click.request_repaint();
        }
    })));

    // 右键菜单项 → 对应 AppCommand
    let tx_menu = tx;
    let repaint_menu = repaint_ctx;
    let id_start = item_start.id().clone();
    let id_pause = item_pause.id().clone();
    let id_reset = item_reset.id().clone();
    let id_stopwatch = item_stopwatch.id().clone();
    let id_countdown = item_countdown.id().clone();
    let id_set_countdown = item_set_countdown.id().clone();
    let id_set_opacity = item_set_opacity.id().clone();
    let id_toggle = item_toggle.id().clone();
    let id_quit = item_quit.id().clone();
    MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
        append_error_file("INFO", &format!("menu clicked id={:?}", event.id));
        let cmd = if event.id == id_start {
            Some(UiCommand::Core(AppCommand::Start))
        } else if event.id == id_pause {
            Some(UiCommand::Core(AppCommand::Pause))
        } else if event.id == id_reset {
            Some(UiCommand::Core(AppCommand::Reset))
        } else if event.id == id_stopwatch {
            Some(UiCommand::Core(AppCommand::SwitchMode(
                timer_core::TimerMode::Stopwatch,
            )))
        } else if event.id == id_countdown {
            Some(UiCommand::Core(AppCommand::SwitchMode(
                timer_core::TimerMode::Countdown,
            )))
        } else if event.id == id_set_countdown {
            wake_window_for_dialog(&repaint_menu);
            Some(UiCommand::OpenSetCountdownDialog)
        } else if event.id == id_set_opacity {
            wake_window_for_dialog(&repaint_menu);
            Some(UiCommand::OpenSetOpacityDialog)
        } else if event.id == id_toggle {
            Some(UiCommand::Core(AppCommand::ToggleWindow))
        } else if event.id == id_quit {
            append_error_file("INFO", "menu matched: quit");
            Some(UiCommand::Core(AppCommand::Quit))
        } else {
            None
        };
        if let Some(cmd) = cmd {
            append_error_file("INFO", &format!("sending ui command: {:?}", cmd));
            let _ = tx_menu.send(cmd);
            repaint_menu.request_repaint();
        }
    })));

    tray
}
