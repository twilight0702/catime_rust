//! 系统托盘图标模块：托盘右键菜单 + 左键点击事件。

use std::sync::mpsc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

use timer_core::AppCommand;

/// 从 `assets/icon.ico` 编译期嵌入图标并解码为 RGBA 格式。
/// 使用 `include_bytes!` 在编译时将图标数据嵌入二进制。
fn create_icon() -> Icon {
    let icon_bytes = include_bytes!("../../../assets/icon.ico");
    let img = image::load_from_memory(icon_bytes)
        .expect("failed to decode icon.ico")
        .into_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();

    Icon::from_rgba(rgba, width, height)
        .expect("failed to create tray icon from rgba")
}

/// 创建托盘图标并设置事件回调。
/// 必须在主线程调用以共享 Windows 消息泵。
/// 返回的 `TrayIcon` 句柄须保持存活（`Box::leak`），否则图标会消失。
pub fn create_tray(tx: mpsc::Sender<AppCommand>) -> tray_icon::TrayIcon {
    let icon = create_icon();

    // 构建右键菜单
    let menu = Menu::new();
    menu.append(&MenuItem::new("开始", true, None)).ok();
    menu.append(&MenuItem::new("暂停", true, None)).ok();
    menu.append(&MenuItem::new("重置", true, None)).ok();
    menu.append(&MenuItem::new("正计时", true, None)).ok();
    menu.append(&MenuItem::new("倒计时", true, None)).ok();
    menu.append(&MenuItem::new("显示/隐藏", true, None)).ok();
    menu.append(&MenuItem::new("退出", true, None)).ok();

    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("Catime")
        .build()
        .expect("failed to create tray icon");

    // 托盘左键/双击 → 切换窗口可见性
    let tx_click = tx.clone();
    TrayIconEvent::set_event_handler(Some(Box::new(move |event: TrayIconEvent| {
        if matches!(
            event,
            TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
        ) {
            let _ = tx_click.send(AppCommand::ToggleWindow);
        }
    })));

    // 右键菜单项 → 对应 AppCommand
    let tx_menu = tx;
    MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
        let cmd = match event.id.0.as_str() {
            "开始" => Some(AppCommand::Start),
            "暂停" => Some(AppCommand::Pause),
            "重置" => Some(AppCommand::Reset),
            "正计时" => Some(AppCommand::SwitchMode(timer_core::TimerMode::Stopwatch)),
            "倒计时" => Some(AppCommand::SwitchMode(timer_core::TimerMode::Countdown)),
            "显示/隐藏" => Some(AppCommand::ToggleWindow),
            "退出" => Some(AppCommand::Quit),
            _ => None,
        };
        if let Some(cmd) = cmd {
            let _ = tx_menu.send(cmd);
        }
    })));

    tray
}
