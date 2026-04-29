//! Win32 系统托盘模块：使用 `Shell_NotifyIconW` 创建托盘图标和右键菜单。

use timer_core::AppCommand;
use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyMenu, GetCursorPos,
    SetForegroundWindow, TrackPopupMenu, HICON, LR_DEFAULTCOLOR, MF_STRING, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD,
};

/// 托盘回调消息 ID（通过 WM_APP + 1 避免冲突）
pub const WM_APP_TRAY: u32 = 0x8001;

// 右键菜单项 ID
pub const MENU_START: usize = 1001;
pub const MENU_PAUSE: usize = 1002;
pub const MENU_RESET: usize = 1003;
pub const MENU_STOPWATCH: usize = 1004;
pub const MENU_COUNTDOWN: usize = 1005;
pub const MENU_TOGGLE: usize = 1006;
pub const MENU_QUIT: usize = 1007;
pub const MENU_SET_COUNTDOWN: usize = 1008;

/// 从编译期嵌入的 `assets/icon.ico` 创建托盘图标句柄。
/// 解析 ICO 文件格式，选取最大尺寸的图标条目。
fn create_hicon() -> HICON {
    let icon_bytes = include_bytes!("../../../assets/icon.ico");

    // ICO 文件头：偏移 4-5 为图标数量
    let count = u16::from_le_bytes([icon_bytes[4], icon_bytes[5]]) as usize;
    assert!(count >= 1, "ico must have at least one entry");

    let entry = 6; // 第一个条目从偏移 6 开始
    let mut best_idx = 0;
    let mut best_w = 0u8;
    // 遍历所有条目，选取宽度最大的
    for i in 0..count {
        let off = entry + i * 16;
        let w = icon_bytes[off]; // ICO 条目偏移 0 为宽度（0 表示 256）
        let actual = if w == 0 { 256 } else { w as u32 };
        if actual > best_w as u32 {
            best_w = w;
            best_idx = i;
        }
    }

    // 读取选中条目的图像数据偏移和大小
    let e = entry + best_idx * 16;
    let img_offset = u32::from_le_bytes([
        icon_bytes[e + 12],
        icon_bytes[e + 13],
        icon_bytes[e + 14],
        icon_bytes[e + 15],
    ]) as usize;
    let img_size = u32::from_le_bytes([
        icon_bytes[e + 8],
        icon_bytes[e + 9],
        icon_bytes[e + 10],
        icon_bytes[e + 11],
    ]) as usize;

    let img_data = &icon_bytes[img_offset..img_offset + img_size];

    unsafe {
        CreateIconFromResourceEx(img_data, BOOL(1), 0x00030000, 0, 0, LR_DEFAULTCOLOR)
            .expect("failed to create tray icon")
    }
}

/// 创建系统托盘图标。
/// 托盘消息通过 `WM_APP_TRAY` 发送到 `hwnd` 的窗口过程。
pub fn create_tray(
    hwnd: HWND,
    show_tooltip: bool,
) -> Result<(), windows::core::Error> {
    let hicon = create_hicon();

    let tip = windows::core::w!("Catime\0");
    let mut flags = NIF_ICON | NIF_MESSAGE;
    if show_tooltip {
        flags |= NIF_TIP;
    }
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: flags,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: hicon,
        ..Default::default()
    };
    if show_tooltip {
        // szTip 是固定 128 元素数组，需手动拷贝
        let tip_bytes = unsafe { tip.as_wide() };
        let len = tip_bytes.len().min(127);
        nid.szTip[..len].copy_from_slice(&tip_bytes[..len]);
    }

    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &mut nid) };
    if ok.as_bool() {
        log::info!("tray icon created");
        Ok(())
    } else {
        Err(windows::core::Error::from_win32())
    }
}

/// 移除系统托盘图标（窗口销毁时调用）。
pub fn remove_tray(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    unsafe { Shell_NotifyIconW(NIM_DELETE, &mut nid) };
}

/// 弹出托盘右键菜单，返回用户选择的菜单项 ID。
pub fn show_tray_menu(hwnd: HWND) -> Option<usize> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        AppendMenuW(menu, MF_STRING, MENU_START, w!("开始"));
        AppendMenuW(menu, MF_STRING, MENU_PAUSE, w!("暂停"));
        AppendMenuW(menu, MF_STRING, MENU_RESET, w!("重置"));
        AppendMenuW(menu, MF_STRING, MENU_STOPWATCH, w!("正计时"));
        AppendMenuW(menu, MF_STRING, MENU_COUNTDOWN, w!("倒计时"));
        AppendMenuW(menu, MF_STRING, MENU_SET_COUNTDOWN, w!("设置倒计时..."));
        AppendMenuW(menu, MF_STRING, MENU_TOGGLE, w!("显示/隐藏"));
        AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("退出"));

        let mut pt = Default::default();
        GetCursorPos(&mut pt); // 在鼠标位置弹出菜单
        SetForegroundWindow(hwnd);

        let cmd = TrackPopupMenu(
            menu,
            TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );

        let _ = DestroyMenu(menu);
        (cmd.0 != 0).then_some(cmd.0 as usize)
    }
}

/// 将菜单项 ID 映射为对应的 `AppCommand`。
pub fn menu_id_to_command(menu_id: usize) -> Option<AppCommand> {
    match menu_id {
        MENU_START => Some(AppCommand::Start),
        MENU_PAUSE => Some(AppCommand::Pause),
        MENU_RESET => Some(AppCommand::Reset),
        MENU_STOPWATCH => Some(AppCommand::SwitchMode(timer_core::TimerMode::Stopwatch)),
        MENU_COUNTDOWN => Some(AppCommand::SwitchMode(timer_core::TimerMode::Countdown)),
        MENU_TOGGLE => Some(AppCommand::ToggleWindow),
        MENU_QUIT => Some(AppCommand::Quit),
        // MENU_SET_COUNTDOWN 单独处理（弹出对话框）
        _ => None,
    }
}
