//! 主窗口消息处理（WndProc）和事件分发。
//! 处理所有 Windows 消息：绘制、计时器、鼠标点击、托盘消息、DPI 变更等。

use std::io::Write;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use timer_app::AppController;
use timer_core::{AppCommand, AppEvent, TimerStatus};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::countdown_dialog;
use crate::render::{self, ButtonHit, RenderContext};
use crate::tray;

/// 窗口关联的应用状态，通过 `GWLP_USERDATA` 存储在窗口中。
pub struct AppState {
    pub controller: AppController,
    /// 命令接收端（来自文件监听器线程）
    pub rx: Receiver<AppCommand>,
    pub render: RenderContext,
    /// 上次 Tick 时刻
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

    // 写入当前目录和 exe 同目录两个位置（调试用）
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

/// 窗口过程入口：捕获 panic 防止崩溃导致整个程序退出。
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
            // 从 CREATESTRUCT 中取出 AppState 指针存入窗口
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let state_ptr = cs.lpCreateParams as *mut AppState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            // 启动 1 秒间隔的定时器
            SetTimer(hwnd, ID_TICK, 1000, None);
            LRESULT(0)
        }

        WM_PAINT => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            // 先处理队列中的命令，再渲染
            drain_commands(hwnd, state);
            let vs = state.controller.view_state().clone();
            render::paint(hwnd, &state.render, &vs);
            LRESULT(0)
        }

        // 阻止系统默认擦除背景（由 render::paint 双缓冲统一处理）
        WM_ERASEBKGND => LRESULT(1),

        WM_TIMER => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            drain_commands(hwnd, state);
            // 仅 Running 状态推进 Tick
            if state.controller.view_state().status == TimerStatus::Running {
                let events = state.controller.handle(AppCommand::Tick);
                process_events(hwnd, state, &events);
                state.last_tick = Instant::now();
            }
            // 触发重绘
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            // 从 lparam 解析鼠标坐标
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
                ButtonHit::None => {
                    // 点击空白区域 → 拖动无标题栏窗口
                    unsafe {
                        let _ = ReleaseCapture();
                        PostMessageW(
                            hwnd,
                            WM_NCLBUTTONDOWN,
                            WPARAM(HTCAPTION as usize),
                            LPARAM(0),
                        );
                    }
                }
            }
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 托盘图标消息（通过 WM_APP_TRAY 自定义消息）
        msg if msg == tray::WM_APP_TRAY => {
            let lo = (lparam.0 & 0xFFFF) as u32;
            match lo {
                // 左键点击托盘图标 → 按配置执行
                WM_LBUTTONUP => {
                    let Some(state) = try_get_state(hwnd) else {
                        return DefWindowProcW(hwnd, msg, wparam, lparam);
                    };
                    let events = state.controller.handle(AppCommand::TrayLeftClick);
                    process_events(hwnd, state, &events);
                }
                // 右键点击托盘图标 → 弹出菜单
                WM_RBUTTONUP => {
                    if let Some(menu_id) = tray::show_tray_menu(hwnd) {
                        if menu_id == tray::MENU_SET_COUNTDOWN {
                            let _ = PostMessageW(
                                hwnd,
                                crate::WM_OPEN_SET_COUNTDOWN,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        } else if menu_id == tray::MENU_SET_OPACITY {
                            let _ = PostMessageW(
                                hwnd,
                                crate::WM_OPEN_SET_OPACITY,
                                WPARAM(0),
                                LPARAM(0),
                            );
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

        // 菜单/加速键命令
        WM_COMMAND => {
            let menu_id = wparam.0 as usize;
            if menu_id == tray::MENU_SET_COUNTDOWN {
                let _ = PostMessageW(hwnd, crate::WM_OPEN_SET_COUNTDOWN, WPARAM(0), LPARAM(0));
            } else if menu_id == tray::MENU_SET_OPACITY {
                let _ = PostMessageW(hwnd, crate::WM_OPEN_SET_OPACITY, WPARAM(0), LPARAM(0));
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

        msg if msg == crate::WM_OPEN_SET_COUNTDOWN => {
            apply_countdown_setting(hwnd);
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        msg if msg == crate::WM_OPEN_SET_OPACITY => {
            apply_opacity_setting(hwnd);
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 窗口尺寸变化 → 重绘
        WM_SIZE => {
            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // 拖拽/缩放结束：记忆窗口位置与尺寸
        WM_EXITSIZEMOVE => {
            let Some(state) = try_get_state(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            persist_window_bounds(hwnd, state);
            LRESULT(0)
        }

        // DPI 变更（窗口被拖到不同 DPI 的显示器）：重建字体并调整窗口大小。
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
            persist_window_bounds(hwnd, state);

            InvalidateRect(hwnd, None, false).ok();
            LRESULT(0)
        }

        // Ctrl+C 自定义关闭消息 → 销毁窗口并退出程序
        msg if msg == crate::WM_CTRLC_SHUTDOWN => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        // 关闭按钮 → 隐藏窗口（不退出）
        WM_CLOSE => {
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
            PostQuitMessage(0); // 通知消息循环退出
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 从窗口的 GWLP_USERDATA 取出 AppState 指针。
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

/// 从 mpsc 通道取出所有待处理的命令并执行（非阻塞）。
fn drain_commands(hwnd: HWND, state: &mut AppState) {
    loop {
        let cmd = match state.rx.try_recv() {
            Ok(cmd) => cmd,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        };
        let is_reload = matches!(&cmd, AppCommand::ReloadConfig);
        let events = state.controller.handle(cmd);
        if is_reload {
            crate::apply_window_opacity(hwnd, state.controller.config().opacity);
        }
        process_events(hwnd, state, &events);
    }
}

/// 读取当前窗口矩形并写回配置，随后立即持久化。
fn persist_window_bounds(hwnd: HWND, state: &mut AppState) {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let width = (rect.right - rect.left).max(1) as u32;
            let height = (rect.bottom - rect.top).max(1) as u32;
            state.controller.update_window_bounds(
                rect.left,
                rect.top,
                width,
                height,
                Some(GetDpiForWindow(hwnd).max(1)),
            );
            if let Err(e) = state.controller.save_config() {
                log::error!("save config after window move/resize failed: {}", e);
            }
        }
    }
}

/// 处理引擎返回的事件：显示/隐藏窗口、退出、倒计时结束自动弹窗。
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
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return;
            }
            AppEvent::TimerFinished => {
                // 倒计时结束时自动弹出窗口
                let extra = state.controller.handle(AppCommand::ShowWindow);
                process_events(hwnd, state, &extra);
            }
            AppEvent::TimerUpdated => {}
        }
    }
}

/// 弹出倒计时设置对话框，将用户输入的时长应用到引擎。
fn apply_countdown_setting(hwnd: HWND) {
    append_error_file("INFO", "apply_countdown_setting: enter");
    // 获取当前倒计时时长和父窗口句柄
    let (current, owner) = {
        let Some(state) = try_get_state(hwnd) else {
            append_error_file(
                "WARN",
                "apply_countdown_setting: state missing before dialog",
            );
            return;
        };
        let current = state.controller.view_state().countdown_duration_secs;
        // 若窗口可见则作为对话框所有者，否则传 null（独立弹出）
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

    // 记录托盘点击附近坐标，使对话框出现在托盘位置边上
    let mut cursor = POINT::default();
    let anchor = unsafe {
        if GetCursorPos(&mut cursor).is_ok() {
            Some((cursor.x, cursor.y))
        } else {
            None
        }
    };

    // 弹出对话框
    if let Some(secs) = countdown_dialog::prompt_countdown_seconds(owner, current, anchor) {
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
        // 切换到倒计时模式
        events.extend(
            state
                .controller
                .handle(AppCommand::SwitchMode(timer_core::TimerMode::Countdown)),
        );
        // 应用新时长
        events.extend(state.controller.handle(AppCommand::SetCountdown(secs)));
        process_events(hwnd, state, &events);
        if let Err(e) = state.controller.save_config() {
            log::error!("save config after set countdown: {}", e);
        }
    } else {
        append_error_file("INFO", "apply_countdown_setting: dialog canceled or failed");
    }
}

/// 弹出透明度设置对话框，将结果即时应用并落盘。
fn apply_opacity_setting(hwnd: HWND) {
    let (current, owner) = {
        let Some(state) = try_get_state(hwnd) else {
            return;
        };
        let current = state.controller.config().opacity;
        let owner = unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                hwnd
            } else {
                HWND(std::ptr::null_mut())
            }
        };
        (current, owner)
    };

    let mut cursor = POINT::default();
    let anchor = unsafe {
        if GetCursorPos(&mut cursor).is_ok() {
            Some((cursor.x, cursor.y))
        } else {
            None
        }
    };

    if let Some(opacity) = countdown_dialog::prompt_opacity(owner, current, anchor) {
        let Some(state) = try_get_state(hwnd) else {
            return;
        };
        state.controller.update_opacity(opacity);
        if let Err(e) = state.controller.save_config() {
            log::error!("save config after set opacity failed: {}", e);
        }
        crate::apply_window_opacity(hwnd, state.controller.config().opacity);
        unsafe {
            InvalidateRect(hwnd, None, false).ok();
        }
    }
}
