// 仅在 Windows release 构建隐藏控制台窗口；其他平台不受影响。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Catime egui 前端入口。
//! 使用 egui/eframe 渲染跨平台 GUI，通过 mpsc 通道与托盘和文件监听器通信。

mod app;
mod tray;
mod ui_command;
mod watcher;

use std::sync::mpsc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{FontData, FontDefinitions, FontFamily};
use timer_app::AppController;
use timer_core::AppCommand;
use timer_storage::{ConfigRepository, TomlConfigRepository};

use app::CatimeApp;
use ui_command::UiCommand;

const ERROR_LOG_FILE: &str = "catime_error.log";
const MIN_WINDOW_WIDTH: f32 = 320.0;
const MIN_WINDOW_HEIGHT: f32 = 280.0;

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
            format!("[{}][{}][main-egui] {}\n", ts, level, msg).as_bytes(),
        );
    }
}

#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// 加载系统中的中文字体
/// 注册为 egui 的首选字体族，使中文能正常显示。
/// 遍历候选字体列表，使用第一个找到的。
fn setup_cjk_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    let cjk_paths: Vec<&str> = if cfg!(target_os = "windows") {
        vec![
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\simsun.ttc",
            "C:\\Windows\\Fonts\\msjh.ttc",
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
    } else {
        vec![]
    };

    for path in &cjk_paths {
        if let Ok(data) = std::fs::read(path) {
            log::info!("loaded CJK font: {}", path);
            let mut font_data = FontData::from_owned(data);
            font_data.index = 0; // 使用字体的第一个 face
                                 // 注册为 "CJK" 字体数据
            fonts
                .font_data
                .insert("CJK".to_owned(), std::sync::Arc::new(font_data));

            // 插入到 Proportional 和 Monospace 字体族的最前面
            fonts
                .families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .insert(0, "CJK".to_owned());

            fonts
                .families
                .get_mut(&FontFamily::Monospace)
                .unwrap()
                .insert(0, "CJK".to_owned());

            return fonts;
        }
    }

    log::warn!("no CJK font found, Chinese characters may display as boxes");
    fonts
}

fn normalized_window_bounds(
    config: &timer_storage::AppConfig,
) -> (f32, f32, f32, f32) {
    let width = (config.window.width as f32).max(MIN_WINDOW_WIDTH);
    let height = (config.window.height as f32).max(MIN_WINDOW_HEIGHT);

    #[cfg(windows)]
    {
        let virtual_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f32;
        let virtual_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f32;
        let virtual_w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f32;
        let virtual_h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f32;
        let clamped_w = width.min(virtual_w);
        let clamped_h = height.min(virtual_h);
        let max_x = virtual_x + (virtual_w - clamped_w).max(0.0);
        let max_y = virtual_y + (virtual_h - clamped_h).max(0.0);
        let x = (config.window.x as f32).clamp(virtual_x, max_x);
        let y = (config.window.y as f32).clamp(virtual_y, max_y);
        return (x, y, clamped_w, clamped_h);
    }

    #[cfg(not(windows))]
    {
        (config.window.x as f32, config.window.y as f32, width, height)
    }
}

fn spawn_tick_thread(tx: mpsc::Sender<UiCommand>, repaint_ctx: egui::Context) {
    std::thread::Builder::new()
        .name("egui-ticker".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if tx.send(UiCommand::Core(AppCommand::Tick)).is_err() {
                    break;
                }
                repaint_ctx.request_repaint();
            }
        })
        .expect("failed to spawn egui ticker thread");
}

fn main() {
    let pid = std::process::id();
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    append_error_file("INFO", &format!("process start pid={} exe={}", pid, exe));

    // 初始化日志系统
    env_logger::init();

    // Ctrl+C 优雅退出
    ctrlc::set_handler(|| {
        log::info!("Ctrl+C received, exiting");
        std::process::exit(0);
    })
    .expect("failed to register Ctrl+C handler");

    // 加载配置文件
    let config_path = match TomlConfigRepository::default_path() {
        Ok(p) => p,
        Err(e) => {
            log::error!("failed to get config path: {}", e);
            return;
        }
    };
    log::info!("config path: {}", config_path.display());

    let config_repo = TomlConfigRepository::new(config_path.clone());
    let config = match config_repo.load() {
        Ok(c) => c,
        Err(e) => {
            log::error!("failed to load config: {}", e);
            return;
        }
    };

    // 创建命令通道：托盘 / 文件监听器 → egui 主循环
    let (tx, rx) = mpsc::channel::<UiCommand>();

    let controller = AppController::new(config.clone(), Box::new(config_repo));
    let (x, y, width, height) = normalized_window_bounds(&config);

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([width, height])
        .with_min_inner_size([MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT])
        .with_position([x, y])
        .with_clamp_size_to_monitor_size(true)
        .with_transparent(true)
        .with_decorations(false)
        .with_taskbar(false)
        .with_title("Catime");
    if config.always_on_top {
        viewport = viewport.with_always_on_top();
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let fonts = setup_cjk_fonts();

    // 启动 egui 应用主循环
    if let Err(e) = eframe::run_native(
        "Catime",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_fonts(fonts.clone());
            spawn_tick_thread(tx.clone(), cc.egui_ctx.clone());
            #[cfg(windows)]
            let _tray = Box::leak(Box::new(tray::create_tray(
                tx.clone(),
                cc.egui_ctx.clone(),
                config.tray.show_remaining_tooltip,
            )));

            if config.hot_reload {
                #[cfg(windows)]
                watcher::spawn_watcher(config_path, tx.clone(), cc.egui_ctx.clone());
            } else {
                log::info!("hot-reload disabled by config");
            }

            Ok(Box::new(CatimeApp::new(controller, rx, tx)))
        }),
    ) {
        log::error!("eframe error: {}", e);
    }
}
