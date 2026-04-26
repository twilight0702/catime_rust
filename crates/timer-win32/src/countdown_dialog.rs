use std::io::Write;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::render::{self, RenderContext};

const EDIT_INSET_X: i32 = 10;
const EDIT_INSET_Y: i32 = 8;

struct DialogState {
    edit: HWND,
    ok_btn: HWND,
    cancel_btn: HWND,
    render: RenderContext,
    error_text: Option<String>,
    initial_secs: u64,
    result: Arc<Mutex<Option<u64>>>,
    done: Arc<AtomicBool>,
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

pub fn prompt_countdown_seconds(parent: HWND, current_secs: u64) -> Option<u64> {
    static REGISTER_ONCE: Once = Once::new();

    unsafe {
        log_warn(&format!(
            "prompt_countdown_seconds: enter parent_null={} current={}",
            parent.0.is_null(),
            current_secs
        ));
        let instance = GetModuleHandleW(None).ok()?;

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

        let (x, y) = if parent.0.is_null() {
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
            ok_btn: HWND(null_mut()),
            cancel_btn: HWND(null_mut()),
            render,
            error_text: None,
            initial_secs: current_secs,
            result: Arc::clone(&result),
            done: Arc::clone(&done),
        });
        let state_ptr = Box::into_raw(state);

        let hwnd = match CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS_NAME,
            w!("设置倒计时"),
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

        let _ = SetForegroundWindow(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        log_warn("prompt_countdown_seconds: entering message loop");

        let mut msg = MSG::default();
        while !done.load(Ordering::Acquire) {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 <= 0 {
                if ret.0 < 0 {
                    log_error("GetMessageW returned -1 in countdown dialog loop");
                }
                break;
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        log_warn("prompt_countdown_seconds: message loop ended");

        if IsWindow(hwnd).as_bool() {
            let _ = DestroyWindow(hwnd);
            log_warn("prompt_countdown_seconds: DestroyWindow from loop exit");
        }

        if !parent.0.is_null() {
            let _ = SetForegroundWindow(parent);
        }

        let out = result.lock().ok().and_then(|g| *g);
        log_warn(&format!("prompt_countdown_seconds: return {:?}", out));
        out
    }
}

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

            let _title = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("设置倒计时"),
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

            let _subtitle = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("支持：秒数、MM:SS、HH:MM:SS"),
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

            let _hint = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("例如：1500、25:00、01:25:00"),
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
                (*state_ptr).ok_btn = ok_btn;
                (*state_ptr).cancel_btn = cancel_btn;
                let initial = timer_core::TimerEngine::format_duration((*state_ptr).initial_secs);
                set_window_text(edit, &initial);
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
            if source == (*state_ptr).ok_btn {
                submit_dialog(hwnd, state_ptr);
                return LRESULT(0);
            }

            if source == (*state_ptr).cancel_btn {
                (*state_ptr).done.store(true, Ordering::Release);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }

            if source == (*state_ptr).edit {
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if code == EN_CHANGE as u32 {
                    if (*state_ptr).error_text.is_some() {
                        (*state_ptr).error_text = None;
                        InvalidateRect(hwnd, None, false).ok();
                    }
                }
            }
            LRESULT(0)
        }

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

        WM_NCDESTROY => {
            log_warn("dialog_wndproc: WM_NCDESTROY");
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState;
            if !state_ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = Box::from_raw(state_ptr);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

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
    match parse_duration_to_secs(&text) {
        Some(secs) if secs > 0 => {
            if let Ok(mut out) = (*state_ptr).result.lock() {
                *out = Some(secs);
            }
            (*state_ptr).done.store(true, Ordering::Release);
            let _ = DestroyWindow(hwnd);
        }
        _ => {
            log_warn(&format!("invalid countdown input: {}", text));
            (*state_ptr).error_text = Some("请输入有效时长（例如 1500 / 25:00 / 01:25:00）".into());
            InvalidateRect(hwnd, None, false).ok();
        }
    }
}

fn set_window_text(hwnd: HWND, text: &str) {
    let s = HSTRING::from(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(s.as_ptr()));
    }
}

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

fn parse_duration_to_secs(input: &str) -> Option<u64> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }

    if !text.contains(':') {
        return text.parse::<u64>().ok();
    }

    let parts: Vec<&str> = text.split(':').collect();
    match parts.as_slice() {
        [m, s] => {
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if s >= 60 {
                return None;
            }
            Some(m * 60 + s)
        }
        [h, m, s] => {
            let h = parse_part(h)?;
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if m >= 60 || s >= 60 {
                return None;
            }
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

fn parse_part(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    if !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_duration_to_secs;

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
        assert_eq!(parse_duration_to_secs("12:70"), None);
        assert_eq!(parse_duration_to_secs("1:70:00"), None);
    }
}
