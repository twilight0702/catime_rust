use timer_core::AppCommand;
use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyMenu, GetCursorPos,
    SetForegroundWindow, TrackPopupMenu, HICON, LR_DEFAULTCOLOR, MF_STRING,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN,
};

pub const WM_APP_TRAY: u32 = 0x8001;

pub const MENU_START: usize = 1001;
pub const MENU_PAUSE: usize = 1002;
pub const MENU_RESET: usize = 1003;
pub const MENU_STOPWATCH: usize = 1004;
pub const MENU_COUNTDOWN: usize = 1005;
pub const MENU_TOGGLE: usize = 1006;
pub const MENU_QUIT: usize = 1007;

fn create_hicon() -> HICON {
    let icon_bytes = include_bytes!("../../../assets/icon.ico");

    let count = u16::from_le_bytes([icon_bytes[4], icon_bytes[5]]) as usize;
    assert!(count >= 1, "ico must have at least one entry");

    let entry = 6;
    let mut best_idx = 0;
    let mut best_w = 0u8;
    for i in 0..count {
        let off = entry + i * 16;
        let w = icon_bytes[off];
        let actual = if w == 0 { 256 } else { w as u32 };
        if actual > best_w as u32 {
            best_w = w;
            best_idx = i;
        }
    }

    let e = entry + best_idx * 16;
    let img_offset = u32::from_le_bytes([
        icon_bytes[e + 12], icon_bytes[e + 13], icon_bytes[e + 14], icon_bytes[e + 15],
    ]) as usize;
    let img_size = u32::from_le_bytes([
        icon_bytes[e + 8], icon_bytes[e + 9], icon_bytes[e + 10], icon_bytes[e + 11],
    ]) as usize;

    let img_data = &icon_bytes[img_offset..img_offset + img_size];

    unsafe {
        CreateIconFromResourceEx(
            img_data,
            BOOL(1),
            0x00030000,
            0,
            0,
            LR_DEFAULTCOLOR,
        )
        .expect("failed to create tray icon")
    }
}

pub fn create_tray(hwnd: HWND) -> Result<(), windows::core::Error> {
    let hicon = create_hicon();

    let tip = windows::core::w!("Catime\0");
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: hicon,
        ..Default::default()
    };
    let tip_bytes = unsafe { tip.as_wide() };
    let len = tip_bytes.len().min(127);
    nid.szTip[..len].copy_from_slice(&tip_bytes[..len]);

    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &mut nid) };
    if ok.as_bool() {
        log::info!("tray icon created");
        Ok(())
    } else {
        Err(windows::core::Error::from_win32())
    }
}

pub fn remove_tray(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    unsafe { Shell_NotifyIconW(NIM_DELETE, &mut nid) };
}

pub fn show_tray_menu(hwnd: HWND) -> Option<usize> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        AppendMenuW(menu, MF_STRING, MENU_START, w!("开始"));
        AppendMenuW(menu, MF_STRING, MENU_PAUSE, w!("暂停"));
        AppendMenuW(menu, MF_STRING, MENU_RESET, w!("重置"));
        AppendMenuW(menu, MF_STRING, MENU_STOPWATCH, w!("正计时"));
        AppendMenuW(menu, MF_STRING, MENU_COUNTDOWN, w!("倒计时"));
        AppendMenuW(menu, MF_STRING, MENU_TOGGLE, w!("显示/隐藏"));
        AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("退出"));

        let mut pt = Default::default();
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);

        let cmd = TrackPopupMenu(
            menu,
            TPM_BOTTOMALIGN | TPM_LEFTALIGN,
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

pub fn menu_id_to_command(menu_id: usize) -> Option<AppCommand> {
    match menu_id {
        MENU_START => Some(AppCommand::Start),
        MENU_PAUSE => Some(AppCommand::Pause),
        MENU_RESET => Some(AppCommand::Reset),
        MENU_STOPWATCH => Some(AppCommand::SwitchMode(timer_core::TimerMode::Stopwatch)),
        MENU_COUNTDOWN => Some(AppCommand::SwitchMode(timer_core::TimerMode::Countdown)),
        MENU_TOGGLE => Some(AppCommand::ToggleWindow),
        MENU_QUIT => Some(AppCommand::Quit),
        _ => None,
    }
}
