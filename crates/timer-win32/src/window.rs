use std::io::Write;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use timer_app::AppController;
use timer_core::{AppCommand, AppEvent, TimerStatus};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::countdown_dialog;
use crate::render::{self, ButtonHit, RenderContext};
use crate::tray;

pub struct AppState {
    pub controller: AppController,
    pub rx: Receiver<AppCommand>,
    pub render: RenderContext,
    pub last_tick: Instant,
}

const ID_TICK: usize = 1;
const ERROR_LOG_FILE: &str = "catime_error.log";

fn append_error_file(level: &str, msg: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{}][{}] {}", ts, level, msg);

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ERROR_LOG_FILE)
    {
        let _ = writeln!(f, "{}", line);
    }

    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop();
        exe.push(ERROR_LOG_FILE);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(exe)
        {
            let _ = writeln!(f, "{}", line);
        }
    }
}

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wndproc_impl(hwnd, msg, wparam, lparam)
    })) {
        Ok(r) => r,
        Err(_) => {
            let m = format!("panic in wndproc, msg=0x{:X}", msg);
            append_error_file("PANIC", &m);
            log::error!("{}", m);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
    }
}

unsafe fn wndproc_impl(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let state_ptr = cs.lpCreateParams as *mut AppState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            SetTimer(hwnd, ID_TICK, 1000, None);
            LRESULT(0)
        }

        WM_PAINT => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            drain_commands(hwnd, state);
            let vs = state.controller.view_state().clone();
            render::paint(hwnd, &state.render, &vs);
            LRESULT(0)
        }

        // 背景由 render::paint 统一处理，避免默认擦背景造成闪烁
        WM_ERASEBKGND => LRESULT(1),

        WM_TIMER => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            drain_commands(hwnd, state);
            // 仅运行时 Tick
            if state.controller.view_state().status == TimerStatus::Running {
                let events = state.controller.handle(AppCommand::Tick);
                process_events(hwnd, state, &events);
                state.last_tick = Instant::now();
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let x = (lparam.0 & 0xFFFF) as u16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as f32;
            let status = state.controller.view_state().status;
            let mode = state.controller.view_state().mode;

            let scale = state.render.scale;
            match render::hit_test_button(x, y, scale) {
                ButtonHit::StartPause => {
                    let cmd = match status {
                        TimerStatus::Running => AppCommand::Pause,
                        TimerStatus::Paused => AppCommand::Resume,
                        _ => AppCommand::Start,
                    };
                    let events = state.controller.handle(cmd);
                    process_events(hwnd, state, &events);
                }
                ButtonHit::Reset => {
                    let events = state.controller.handle(AppCommand::Reset);
                    process_events(hwnd, state, &events);
                }
                ButtonHit::SwitchMode => {
                    let new_mode = match mode {
                        timer_core::TimerMode::Stopwatch => timer_core::TimerMode::Countdown,
                        timer_core::TimerMode::Countdown => timer_core::TimerMode::Stopwatch,
                    };
                    let events = state.controller.handle(AppCommand::SwitchMode(new_mode));
                    process_events(hwnd, state, &events);
                }
                ButtonHit::None => {}
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 托盘自定义消息
        msg if msg == tray::WM_APP_TRAY => {
            let lo = (lparam.0 & 0xFFFF) as u32;
            match lo {
                WM_LBUTTONUP => {
                    let Some(state) = try_get_state(hwnd) else {
                        return DefWindowProcW(hwnd, msg, wparam, lparam);
                    };
                    let events = state.controller.handle(AppCommand::ShowWindow);
                    process_events(hwnd, state, &events);
                }
                WM_RBUTTONUP => {
                    if let Some(menu_id) = tray::show_tray_menu(hwnd) {
                        if menu_id == tray::MENU_SET_COUNTDOWN {
                            apply_countdown_setting(hwnd);
                        } else if let Some(cmd) = tray::menu_id_to_command(menu_id) {
                            let Some(state) = try_get_state(hwnd) else {
                                return DefWindowProcW(hwnd, msg, wparam, lparam);
                            };
                            let events = state.controller.handle(cmd);
                            process_events(hwnd, state, &events);
                        }
                    }
                }
                _ => {}
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_COMMAND => {
            let menu_id = wparam.0 as usize;
            if menu_id == tray::MENU_SET_COUNTDOWN {
                apply_countdown_setting(hwnd);
                InvalidateRect(hwnd, None, false).ok();
            } else if let Some(cmd) = tray::menu_id_to_command(menu_id) {
                let Some(state) = try_get_state(hwnd) else {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                };
                let events = state.controller.handle(cmd);
                process_events(hwnd, state, &events);
                InvalidateRect(hwnd, None, false).ok();
            }
            LRESULT(0)
        }

        WM_SIZE => {
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 窗口移动到不同 DPI 的屏幕时 Windows 发送此消息
        WM_DPICHANGED => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let new_dpi = (wparam.0 & 0xFFFF) as i32;
            state.render.rebuild(new_dpi);

            // 用系统建议的尺寸重新调整窗口
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
            LRESULT(0)
        }

        WM_CLOSE => {
            // 隐藏而非退出，并同步 controller 的 window_visible 状态
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let events = state.controller.handle(AppCommand::HideWindow);
            process_events(hwnd, state, &events);
            LRESULT(0)
        }

        WM_DESTROY => {
            tray::remove_tray(hwnd);
            KillTimer(hwnd, ID_TICK);
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn try_get_state(hwnd: HWND) -> Option<&'static mut AppState> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
        if ptr.is_null() {
            log::warn!("wndproc message before state init");
            None
        } else {
            Some(&mut *ptr)
        }
    }
}

fn drain_commands(hwnd: HWND, state: &mut AppState) {
    loop {
        let cmd = match state.rx.try_recv() {
            Ok(cmd) => cmd,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        };
        let events = state.controller.handle(cmd);
        process_events(hwnd, state, &events);
    }
}

fn process_events(hwnd: HWND, state: &mut AppState, events: &[AppEvent]) {
    for event in events {
        match event {
            AppEvent::WindowShouldShow => unsafe {
                ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            },
            AppEvent::WindowShouldHide => unsafe {
                ShowWindow(hwnd, SW_HIDE);
            },
            AppEvent::AppShouldQuit => {
                if let Err(e) = state.controller.save_config() {
                    log::error!("save config before quit: {}", e);
                }
                unsafe { PostQuitMessage(0) };
                return;
            }
            AppEvent::TimerFinished => {
                let extra = state.controller.handle(AppCommand::ShowWindow);
                process_events(hwnd, state, &extra);
            }
            AppEvent::TimerUpdated => {}
        }
    }
}

fn apply_countdown_setting(hwnd: HWND) {
    append_error_file("INFO", "apply_countdown_setting: enter");
    let (current, owner) = {
        let Some(state) = try_get_state(hwnd) else {
            append_error_file(
                "WARN",
                "apply_countdown_setting: state missing before dialog",
            );
            return;
        };
        let current = state.controller.view_state().countdown_duration_secs;
        let owner = unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                hwnd
            } else {
                HWND(std::ptr::null_mut())
            }
        };
        (current, owner)
    };
    append_error_file(
        "INFO",
        &format!(
            "apply_countdown_setting: current_secs={}, owner_null={}",
            current,
            owner.0.is_null()
        ),
    );

    if let Some(secs) = countdown_dialog::prompt_countdown_seconds(owner, current) {
        append_error_file(
            "INFO",
            &format!("apply_countdown_setting: dialog returned secs={}", secs),
        );
        let Some(state) = try_get_state(hwnd) else {
            append_error_file(
                "WARN",
                "apply_countdown_setting: state missing after dialog",
            );
            return;
        };
        let mut events = Vec::new();
        events.extend(
            state
                .controller
                .handle(AppCommand::SwitchMode(timer_core::TimerMode::Countdown)),
        );
        events.extend(state.controller.handle(AppCommand::SetCountdown(secs)));
        process_events(hwnd, state, &events);
        if let Err(e) = state.controller.save_config() {
            log::error!("save config after set countdown: {}", e);
        }
    } else {
        append_error_file("INFO", "apply_countdown_setting: dialog canceled or failed");
    }
}
