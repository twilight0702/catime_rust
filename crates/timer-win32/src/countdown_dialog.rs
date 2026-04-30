//! 倒计时设置对话框：Windows 原生模态对话框。
//! 使用 raw Win32 API 创建控件（EDIT、BUTTON、STATIC），运行独立消息循环。

use std::io::Write;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, SetBkMode, SetTextColor,
    UpdateWindow, HBRUSH, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::render::{self, RenderContext};

/// 输入框相对于布局边界的缩进
const EDIT_INSET_X: i32 = 10;
const EDIT_INSET_Y: i32 = 8;

/// 对话框内部状态：持有所有子控件句柄和结果通道。
struct DialogState {
    /// 输入框（EDIT 控件）
    edit: HWND,
    /// 错误提示标签（STATIC 控件）
    err_label: HWND,
    /// 确定按钮
    ok_btn: HWND,
    /// 取消按钮
    cancel_btn: HWND,
    /// 对话框背景画刷
    bg_brush: HBRUSH,
    /// 输入框背景画刷
    edit_brush: HBRUSH,
    /// 渲染上下文（字体 + DPI 缩放）
    render: RenderContext,
    /// 当前错误文本
    error_text: Option<String>,
    /// 初始倒计时秒数（用于预填输入框）
    initial_secs: u64,
    /// 输入对话框类型
    kind: DialogKind,
    /// 返回结果
    result: Arc<Mutex<Option<DialogResult>>>,
    /// 对话框是否已关闭
    done: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum DialogKind {
    Countdown,
    Opacity,
}

#[derive(Clone, Copy, Debug)]
enum DialogResult {
    Countdown(u64),
    Opacity(f32),
}

const CLASS_NAME: PCWSTR = w!("CATIME_SET_COUNTDOWN_DIALOG");
const ERROR_LOG_FILE: &str = "catime_error.log";

fn log_error(msg: &str) {
    log::error!("{}", msg);
    append_error_file("ERROR", msg);
}

fn log_warn(msg: &str) {
    log::warn!("{}", msg);
    append_error_file("WARN", msg);
}

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

/// 弹出倒计时设置对话框，返回用户输入的秒数（取消返回 None）。
///
/// `parent`：父窗口句柄，对话框将居中显示在其上方。
/// `current_secs`：当前倒计时秒数，用于预填输入框。
/// `anchor`：可选锚点坐标（屏幕坐标），传入时优先在该点附近弹出。
pub fn prompt_countdown_seconds(
    parent: HWND,
    current_secs: u64,
    anchor: Option<(i32, i32)>,
) -> Option<u64> {
    prompt_dialog(
        parent,
        current_secs,
        anchor,
        DialogKind::Countdown,
        "设置倒计时",
    )
    .and_then(|result| match result {
        DialogResult::Countdown(secs) => Some(secs),
        DialogResult::Opacity(_) => None,
    })
}

/// 弹出透明度设置对话框，返回 0.05 ~ 1.0 的不透明度值。
pub fn prompt_opacity(parent: HWND, current: f32, anchor: Option<(i32, i32)>) -> Option<f32> {
    let initial = (current.clamp(0.05, 1.0) * 100.0).round() as u64;
    prompt_dialog(parent, initial, anchor, DialogKind::Opacity, "设置透明度").and_then(
        |result| match result {
            DialogResult::Opacity(opacity) => Some(opacity),
            DialogResult::Countdown(_) => None,
        },
    )
}

fn prompt_dialog(
    parent: HWND,
    initial_value: u64,
    anchor: Option<(i32, i32)>,
    kind: DialogKind,
    window_title: &str,
) -> Option<DialogResult> {
    static REGISTER_ONCE: Once = Once::new();

    unsafe {
        log_warn(&format!(
            "prompt_countdown_seconds: enter parent_null={} current={}",
            parent.0.is_null(),
            initial_value
        ));
        let instance = GetModuleHandleW(None).ok()?;

        // 只注册一次窗口类（线程安全）
        REGISTER_ONCE.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(dialog_wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            let atom = RegisterClassExW(&wc);
            if atom == 0 {
                log_error("RegisterClassExW for countdown dialog failed");
            }
        });

        let render = RenderContext::new();
        let (width, height) = render::dialog_window_size(render.scale);
        let result = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));

        // 计算对话框位置（优先锚点，其次父窗口居中，最后固定位置）
        let (x, y) = if let Some((ax, ay)) = anchor {
            place_near_anchor(ax, ay, width, height)
        } else if parent.0.is_null() {
            (200, 160)
        } else {
            let mut parent_rect = RECT::default();
            let _ = GetWindowRect(parent, &mut parent_rect);
            (
                parent_rect.left + ((parent_rect.right - parent_rect.left - width) / 2).max(0),
                parent_rect.top + ((parent_rect.bottom - parent_rect.top - height) / 2).max(0),
            )
        };

        let state = Box::new(DialogState {
            edit: HWND(null_mut()),
            err_label: HWND(null_mut()),
            ok_btn: HWND(null_mut()),
            cancel_btn: HWND(null_mut()),
            bg_brush: HBRUSH(null_mut()),
            edit_brush: HBRUSH(null_mut()),
            render,
            error_text: None,
            initial_secs: initial_value,
            kind,
            result: Arc::clone(&result),
            done: Arc::clone(&done),
        });
        let state_ptr = Box::into_raw(state);
        let title = HSTRING::from(window_title);

        // 创建对话框窗口
        let hwnd = match CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_VISIBLE,
            x,
            y,
            width,
            height,
            parent,
            None,
            instance,
            Some(state_ptr as _),
        ) {
            Ok(h) => h,
            Err(e) => {
                log_error(&format!("create countdown dialog failed: {}", e));
                let _ = Box::from_raw(state_ptr);
                return None;
            }
        };
        log_warn("prompt_countdown_seconds: dialog window created");

        // 强制完成首帧显示与激活，避免窗口要等下一次鼠标消息才真正出现。
        let _ = InvalidateRect(hwnd, None, true);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = UpdateWindow(hwnd);
        let edit = (*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState)).edit;
        if !edit.0.is_null() {
            let _ = SetFocus(edit);
        }
        log_warn("prompt_countdown_seconds: entering message loop");

        // 独立消息循环：阻塞调用线程直到对话框关闭
        let mut msg = MSG::default();
        while !done.load(Ordering::Acquire) {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 <= 0 {
                if ret.0 < 0 {
                    log_error("GetMessageW returned -1 in countdown dialog loop");
                }
                break;
            }
            // 将对话框键盘导航消息分发给 IsDialogMessageW 处理
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        log_warn("prompt_countdown_seconds: message loop ended");

        // 确保窗口已销毁
        if IsWindow(hwnd).as_bool() {
            let _ = DestroyWindow(hwnd);
            log_warn("prompt_countdown_seconds: DestroyWindow from loop exit");
        }

        // 恢复父窗口焦点
        if !parent.0.is_null() {
            let _ = SetForegroundWindow(parent);
        }

        let out = result.lock().ok().and_then(|g| *g);
        log_warn(&format!("prompt_countdown_seconds: return {:?}", out));
        out
    }
}

/// 将对话框放在锚点右下侧，并限制在主屏幕可见区域内。
fn place_near_anchor(anchor_x: i32, anchor_y: i32, dialog_w: i32, dialog_h: i32) -> (i32, i32) {
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN).max(1);
        let screen_h = GetSystemMetrics(SM_CYSCREEN).max(1);
        let margin = 8;

        let mut x = anchor_x + 12;
        let mut y = anchor_y + 12;

        let max_x = (screen_w - dialog_w - margin).max(margin);
        let max_y = (screen_h - dialog_h - margin).max(margin);
        x = x.clamp(margin, max_x);
        y = y.clamp(margin, max_y);

        (x, y)
    }
}

/// 对话框窗口过程入口（带 panic 保护）。
unsafe extern "system" fn dialog_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dialog_wndproc_impl(hwnd, msg, wparam, lparam)
    })) {
        Ok(r) => r,
        Err(_) => {
            let m = format!("panic in countdown dialog wndproc, msg=0x{:X}", msg);
            log_error(&m);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
    }
}

/// 对话框窗口过程实现：处理 WM_CREATE（创建控件）、WM_COMMAND（按钮/输入）、颜色绘制等。
unsafe fn dialog_wndproc_impl(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            log_warn("dialog_wndproc: WM_CREATE");
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let state_ptr = cs.lpCreateParams as *mut DialogState;
            if state_ptr.is_null() {
                log_error("dialog WM_CREATE got null state_ptr");
                return LRESULT(-1);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

            let instance = GetModuleHandleW(None).unwrap_or_default();
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let layout =
                render::dialog_layout((*state_ptr).render.scale, client.right - client.left);
            let title_h = layout.title.bottom - layout.title.top;
            let subtitle_h = layout.subtitle.bottom - layout.subtitle.top;
            let hint_h = layout.hint.bottom - layout.hint.top;
            let error_h = layout.error.bottom - layout.error.top;
            let (title_text, subtitle_text, hint_text) = match (*state_ptr).kind {
                DialogKind::Countdown => ("设置倒计时", "支持：秒数、MM:SS、HH:MM:SS", "例如：1500、25:00、01:25:00"),
                DialogKind::Opacity => ("设置透明度", "支持：0.05 ~ 1.00，或 5% ~ 100%", "例如：0.85、85%、100"),
            };
            let title_text = HSTRING::from(title_text);
            let subtitle_text = HSTRING::from(subtitle_text);
            let hint_text = HSTRING::from(hint_text);

            // 标题（STATIC 控件）
            let _title = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR(title_text.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                layout.title.left,
                layout.title.top,
                layout.title.right - layout.title.left,
                title_h,
                hwnd,
                None,
                instance,
                None,
            );

            // 副标题：格式说明
            let _subtitle = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR(subtitle_text.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                layout.subtitle.left,
                layout.subtitle.top,
                layout.subtitle.right - layout.subtitle.left,
                subtitle_h,
                hwnd,
                None,
                instance,
                None,
            );

            // 输入框（EDIT 控件）
            let edit_style =
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32);
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                layout.input.left + EDIT_INSET_X,
                layout.input.top + EDIT_INSET_Y,
                layout.input.right - layout.input.left - EDIT_INSET_X * 2,
                layout.input.bottom - layout.input.top - EDIT_INSET_Y * 2,
                hwnd,
                None,
                instance,
                None,
            );

            // 输入提示
            let _hint = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR(hint_text.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                layout.hint.left,
                layout.hint.top,
                layout.hint.right - layout.hint.left,
                hint_h,
                hwnd,
                None,
                instance,
                None,
            );

            // 错误提示标签（初始隐藏）
            let _error = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_VISIBLE,
                layout.error.left,
                layout.error.top,
                layout.error.right - layout.error.left,
                error_h,
                hwnd,
                None,
                instance,
                None,
            );

            // 确定按钮（默认按钮，回车触发）
            let ok_style =
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32);
            let ok_btn = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("确定"),
                ok_style,
                layout.btn_confirm.left,
                layout.btn_confirm.top,
                layout.btn_confirm.right - layout.btn_confirm.left,
                layout.btn_confirm.bottom - layout.btn_confirm.top,
                hwnd,
                None,
                instance,
                None,
            );

            // 取消按钮
            let cancel_style =
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32);
            let cancel_btn = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("取消"),
                cancel_style,
                layout.btn_cancel.left,
                layout.btn_cancel.top,
                layout.btn_cancel.right - layout.btn_cancel.left,
                layout.btn_cancel.bottom - layout.btn_cancel.top,
                hwnd,
                None,
                instance,
                None,
            );

            if let (Ok(edit), Ok(ok_btn), Ok(cancel_btn)) = (edit, ok_btn, cancel_btn) {
                (*state_ptr).edit = edit;
                if let Ok(err_label) = _error {
                    (*state_ptr).err_label = err_label;
                    let _ = ShowWindow((*state_ptr).err_label, SW_HIDE);
                }
                (*state_ptr).ok_btn = ok_btn;
                (*state_ptr).cancel_btn = cancel_btn;

                // 预填当前倒计时时长
                let initial = match (*state_ptr).kind {
                    DialogKind::Countdown => {
                        timer_core::TimerEngine::format_duration((*state_ptr).initial_secs)
                    }
                    DialogKind::Opacity => format!("{}%", (*state_ptr).initial_secs),
                };
                set_window_text(edit, &initial);

                // 统一设置所有子控件字体
                let _ = SendMessageW(
                    edit,
                    WM_SETFONT,
                    WPARAM((*state_ptr).render.font_label.0 as usize),
                    LPARAM(1),
                );
                if let Ok(t) = _title {
                    let _ = SendMessageW(
                        t,
                        WM_SETFONT,
                        WPARAM((*state_ptr).render.font_title.0 as usize),
                        LPARAM(1),
                    );
                }
                if let Ok(s) = _subtitle {
                    let _ = SendMessageW(
                        s,
                        WM_SETFONT,
                        WPARAM((*state_ptr).render.font_label.0 as usize),
                        LPARAM(1),
                    );
                }
                if let Ok(h) = _hint {
                    let _ = SendMessageW(
                        h,
                        WM_SETFONT,
                        WPARAM((*state_ptr).render.font_label.0 as usize),
                        LPARAM(1),
                    );
                }
                if !(*state_ptr).err_label.0.is_null() {
                    let _ = SendMessageW(
                        (*state_ptr).err_label,
                        WM_SETFONT,
                        WPARAM((*state_ptr).render.font_label.0 as usize),
                        LPARAM(1),
                    );
                }
                let _ = SendMessageW(
                    ok_btn,
                    WM_SETFONT,
                    WPARAM((*state_ptr).render.font_btn.0 as usize),
                    LPARAM(1),
                );
                let _ = SendMessageW(
                    cancel_btn,
                    WM_SETFONT,
                    WPARAM((*state_ptr).render.font_btn.0 as usize),
                    LPARAM(1),
                );

                // 背景画刷
                (*state_ptr).bg_brush = CreateSolidBrush(COLORREF(0x00F8F8F8));
                (*state_ptr).edit_brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
            } else {
                log_error("CreateWindowExW controls for countdown dialog failed");
            }

            LRESULT(0)
        }

        WM_PAINT => {
            log_warn("dialog_wndproc: WM_PAINT");
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        WM_LBUTTONDOWN => DefWindowProcW(hwnd, msg, wparam, lparam),

        WM_COMMAND => {
            log_warn("dialog_wndproc: WM_COMMAND");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if state_ptr.is_null() {
                log_error("dialog WM_COMMAND with null state_ptr");
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }

            let source = HWND(lparam.0 as _);
            // 确定按钮 → 解析并提交
            if source == (*state_ptr).ok_btn {
                submit_dialog(hwnd, state_ptr);
                return LRESULT(0);
            }

            // 取消按钮 → 关闭对话框
            if source == (*state_ptr).cancel_btn {
                (*state_ptr).done.store(true, Ordering::Release);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }

            // 输入框文本变化 → 清除错误提示
            if source == (*state_ptr).edit {
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if code == EN_CHANGE as u32 {
                    if (*state_ptr).error_text.is_some() {
                        (*state_ptr).error_text = None;
                        if !(*state_ptr).err_label.0.is_null() {
                            set_window_text((*state_ptr).err_label, "");
                            let _ = ShowWindow((*state_ptr).err_label, SW_HIDE);
                        }
                        InvalidateRect(hwnd, None, false).ok();
                    }
                }
            }
            LRESULT(0)
        }

        // 自定义对话框背景色
        WM_CTLCOLORDLG => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() && !(*state_ptr).bg_brush.0.is_null() {
                return LRESULT((*state_ptr).bg_brush.0 as isize);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        // 自定义输入框背景色和文字色
        WM_CTLCOLOREDIT => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as _);
                let _ = SetBkMode(hdc, TRANSPARENT);
                let _ = SetTextColor(hdc, COLORREF(0x00101010));
                if !(*state_ptr).edit_brush.0.is_null() {
                    return LRESULT((*state_ptr).edit_brush.0 as isize);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        // 自定义 STATIC 控件文字色（错误标签为红色）
        WM_CTLCOLORSTATIC => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as _);
                let src = HWND(lparam.0 as _);
                let _ = SetBkMode(hdc, TRANSPARENT);
                if src == (*state_ptr).err_label {
                    let _ = SetTextColor(hdc, COLORREF(0x000000D0)); // 红色
                } else {
                    let _ = SetTextColor(hdc, COLORREF(0x00444444)); // 深灰
                }
                if !(*state_ptr).bg_brush.0.is_null() {
                    return LRESULT((*state_ptr).bg_brush.0 as isize);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        // 窗口缩放 → 重新计算输入框位置
        WM_SIZE => {
            log_warn("dialog_wndproc: WM_SIZE");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() && !(*state_ptr).edit.0.is_null() {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let layout =
                    render::dialog_layout((*state_ptr).render.scale, client.right - client.left);
                let _ = MoveWindow(
                    (*state_ptr).edit,
                    layout.input.left + EDIT_INSET_X,
                    layout.input.top + EDIT_INSET_Y,
                    layout.input.right - layout.input.left - EDIT_INSET_X * 2,
                    layout.input.bottom - layout.input.top - EDIT_INSET_Y * 2,
                    true,
                );
            }
            LRESULT(0)
        }

        // DPI 变更 → 重建字体并调整窗口尺寸
        WM_DPICHANGED => {
            log_warn("dialog_wndproc: WM_DPICHANGED");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                let new_dpi = (wparam.0 & 0xFFFF) as i32;
                (*state_ptr).render.rebuild(new_dpi);
                let suggested = &*(lparam.0 as *const RECT);
                SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .ok();
                InvalidateRect(hwnd, None, false).ok();
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            log_warn("dialog_wndproc: WM_CLOSE");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                (*state_ptr).done.store(true, Ordering::Release);
            } else {
                log_warn("dialog WM_CLOSE with null state_ptr");
            }
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_DESTROY => {
            log_warn("dialog_wndproc: WM_DESTROY");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                (*state_ptr).done.store(true, Ordering::Release);
            } else {
                log_warn("dialog WM_DESTROY with null state_ptr");
            }
            LRESULT(0)
        }

        // 最后一个窗口消息：清理 GDI 资源和 DialogState
        WM_NCDESTROY => {
            log_warn("dialog_wndproc: WM_NCDESTROY");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                if !(*state_ptr).bg_brush.0.is_null() {
                    let _ = DeleteObject((*state_ptr).bg_brush);
                }
                if !(*state_ptr).edit_brush.0.is_null() {
                    let _ = DeleteObject((*state_ptr).edit_brush);
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = Box::from_raw(state_ptr); // 释放 DialogState
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 提交对话框：解析输入框文本，有效则写入 result 并关闭，无效则显示错误。
unsafe fn submit_dialog(hwnd: HWND, state_ptr: *mut DialogState) {
    if state_ptr.is_null() {
        log_error("submit_dialog got null state_ptr");
        return;
    }
    if (*state_ptr).edit.0.is_null() {
        log_error("submit_dialog edit handle is null");
        return;
    }

    let text = get_window_text((*state_ptr).edit);
    let parsed = match (*state_ptr).kind {
        DialogKind::Countdown => parse_duration_to_secs(&text)
            .filter(|secs| *secs > 0)
            .map(DialogResult::Countdown),
        DialogKind::Opacity => parse_opacity_input(&text).map(DialogResult::Opacity),
    };

    if let Some(result) = parsed {
        if let Ok(mut out) = (*state_ptr).result.lock() {
            *out = Some(result);
        }
        (*state_ptr).done.store(true, Ordering::Release);
        let _ = DestroyWindow(hwnd);
        return;
    }

    let msg = match (*state_ptr).kind {
        DialogKind::Countdown => "请输入有效时长（例如 1500 / 25:00 / 01:25:00）",
        DialogKind::Opacity => "请输入有效透明度，例如 0.85 / 85% / 100",
    }
    .to_string();
    (*state_ptr).error_text = Some(msg.clone());
    if !(*state_ptr).err_label.0.is_null() {
        set_window_text((*state_ptr).err_label, &msg);
        let _ = ShowWindow((*state_ptr).err_label, SW_SHOW);
    }
    InvalidateRect(hwnd, None, false).ok();
}

/// 设置窗口/控件文本（UTF-16）。
fn set_window_text(hwnd: HWND, text: &str) {
    let s = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(s.as_ptr()));
    }
}

/// 获取窗口/控件文本（UTF-16 → Rust String）。
fn get_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let _ = GetWindowTextW(hwnd, &mut buf);
        let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..nul]).trim().to_string()
    }
}

/// 解析用户输入的时长字符串为秒数。
/// 支持格式：纯数字（秒数）、MM:SS、HH:MM:SS。
fn parse_duration_to_secs(input: &str) -> Option<u64> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }

    // 纯数字 → 直接当秒数
    if !text.contains(':') {
        return text.parse::<u64>().ok();
    }

    // 冒号分隔 → 解析 MM:SS 或 HH:MM:SS
    let parts: Vec<&str> = text.split(':').collect();
    match parts.as_slice() {
        [m, s] => {
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if s >= 60 {
                return None; // 秒数不能 >= 60
            }
            Some(m * 60 + s)
        }
        [h, m, s] => {
            let h = parse_part(h)?;
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if m >= 60 || s >= 60 {
                return None; // 分/秒不能 >= 60
            }
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

/// 解析时长字符串中的一个数字部分。
fn parse_part(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    if !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

fn parse_opacity_input(input: &str) -> Option<f32> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(percent) = text.strip_suffix('%') {
        let value = percent.trim().parse::<f32>().ok()?;
        return Some((value / 100.0).clamp(0.05, 1.0));
    }

    let value = text.parse::<f32>().ok()?;
    if value > 1.0 {
        Some((value / 100.0).clamp(0.05, 1.0))
    } else {
        Some(value.clamp(0.05, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_duration_to_secs, parse_opacity_input};

    #[test]
    fn parses_plain_seconds() {
        assert_eq!(parse_duration_to_secs("1500"), Some(1500));
    }

    #[test]
    fn parses_mm_ss() {
        assert_eq!(parse_duration_to_secs("25:00"), Some(1500));
        assert_eq!(parse_duration_to_secs("70:05"), Some(4205));
    }

    #[test]
    fn parses_hh_mm_ss() {
        assert_eq!(parse_duration_to_secs("01:25:00"), Some(5100));
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(parse_duration_to_secs(""), None);
        assert_eq!(parse_duration_to_secs("abc"), None);
        assert_eq!(parse_duration_to_secs("12:70"), None); // 秒 >= 60
        assert_eq!(parse_duration_to_secs("1:70:00"), None); // 分 >= 60
    }

    #[test]
    fn parses_opacity_input() {
        assert_eq!(parse_opacity_input("0.85"), Some(0.85));
        assert_eq!(parse_opacity_input("85%"), Some(0.85));
        assert_eq!(parse_opacity_input("100"), Some(1.0));
    }
}
