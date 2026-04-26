use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Instant;

use timer_app::AppController;
use timer_core::{AppCommand, AppEvent, TimerStatus};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::render::{self, ButtonHit, RenderContext};
use crate::tray;

pub struct AppState {
    pub controller: AppController,
    pub tx: Sender<AppCommand>,
    pub rx: Receiver<AppCommand>,
    pub render: RenderContext,
    pub last_tick: Instant,
}

const ID_TICK: usize = 1;

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let state_ptr = cs.lpCreateParams as *mut AppState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            SetTimer(hwnd, ID_TICK, 1000, None);
            LRESULT(0)
        }

        WM_PAINT => {
            let state = get_state(hwnd);
            drain_commands(state);
            let vs = state.controller.view_state().clone();
            render::paint(hwnd, &state.render, &vs);
            LRESULT(0)
        }

        WM_TIMER => {
            let state = get_state(hwnd);
            drain_commands(state);
            // 仅运行时 Tick
            if state.controller.view_state().status == TimerStatus::Running {
                state.controller.handle(AppCommand::Tick);
                state.last_tick = Instant::now();
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let state = get_state(hwnd);
            let x = (lparam.0 & 0xFFFF) as u16 as f32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as f32;
            let vs = state.controller.view_state();

            let scale = state.render.scale;
            match render::hit_test_button(x, y, scale) {
                ButtonHit::StartPause => {
                    let cmd = match vs.status {
                        TimerStatus::Running => AppCommand::Pause,
                        TimerStatus::Paused => AppCommand::Resume,
                        _ => AppCommand::Start,
                    };
                    let _ = state.tx.send(cmd);
                }
                ButtonHit::Reset => {
                    let _ = state.tx.send(AppCommand::Reset);
                }
                ButtonHit::SwitchMode => {
                    let new_mode = match vs.mode {
                        timer_core::TimerMode::Stopwatch => timer_core::TimerMode::Countdown,
                        timer_core::TimerMode::Countdown => timer_core::TimerMode::Stopwatch,
                    };
                    let _ = state.tx.send(AppCommand::SwitchMode(new_mode));
                }
                ButtonHit::None => {}
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 托盘自定义消息
        msg if msg == tray::WM_APP_TRAY => {
            let state = get_state(hwnd);
            let lo = (lparam.0 & 0xFFFF) as u32;
            match lo {
                WM_LBUTTONUP => {
                    let _ = state.tx.send(AppCommand::ToggleWindow);
                }
                WM_RBUTTONUP => {
                    if let Some(menu_id) = tray::show_tray_menu(hwnd) {
                        if let Some(cmd) = tray::menu_id_to_command(menu_id) {
                            let _ = state.tx.send(cmd);
                        }
                    }
                }
                _ => {}
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_COMMAND => {
            let state = get_state(hwnd);
            let menu_id = wparam.0 as usize;
            if let Some(cmd) = tray::menu_id_to_command(menu_id) {
                let _ = state.tx.send(cmd);
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
            let state = get_state(hwnd);
            let new_dpi = (wparam.0 & 0xFFFF) as i32;
            state.render.rebuild(new_dpi);

            // 用系统建议的尺寸重新调整窗口
            let suggested = &*(lparam.0 as *const RECT);
            SetWindowPos(
                hwnd, None,
                suggested.left, suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            ).ok();

            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_CLOSE => {
            // 隐藏而非退出
            ShowWindow(hwnd, SW_HIDE);
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

fn get_state(hwnd: HWND) -> &'static mut AppState {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
        assert!(!ptr.is_null());
        &mut *ptr
    }
}

fn drain_commands(state: &mut AppState) {
    loop {
        let cmd = match state.rx.try_recv() {
            Ok(cmd) => cmd,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        };
        let events = state.controller.handle(cmd);
        for event in events {
            match event {
                AppEvent::AppShouldQuit => {
                    let _ = state.controller.save_config();
                    unsafe { PostQuitMessage(0) };
                    return;
                }
                AppEvent::TimerFinished => {
                    state.controller.handle(AppCommand::ShowWindow);
                }
                _ => {}
            }
        }
    }
}
