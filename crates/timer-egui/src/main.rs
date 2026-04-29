//! Catime egui 前端入口。
//! 使用 egui/eframe 渲染跨平台 GUI，通过 mpsc 通道与托盘和文件监听器通信。

mod app;
mod tray;
mod watcher;

use std::sync::mpsc;

use egui::{FontData, FontDefinitions, FontFamily};
use timer_app::AppController;
use timer_storage::{ConfigRepository, TomlConfigRepository};

use app::CatimeApp;

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

fn main() {
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
    let (tx, rx) = mpsc::channel::<timer_core::AppCommand>();

    // 在主线程创建托盘，与 eframe 共用 Windows 消息泵
    // Box::leak 确保托盘句柄在程序整个生命周期内有效
    #[cfg(windows)]
    let _tray = Box::leak(Box::new(tray::create_tray(
        tx.clone(),
        config.tray.show_remaining_tooltip,
    )));

    // 按配置决定是否启动热更新监听
    if config.hot_reload {
        #[cfg(windows)]
    watcher::spawn_watcher(config_path, tx.clone());
    } else {
        log::info!("hot-reload disabled by config");
    }

    let controller = AppController::new(config, Box::new(config_repo));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([300.0, 200.0])
            .with_title("Catime"),
        ..Default::default()
    };

    let fonts = setup_cjk_fonts();

    // 启动 egui 应用主循环
    if let Err(e) = eframe::run_native(
        "Catime",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_fonts(fonts.clone());
            Ok(Box::new(CatimeApp::new(controller, rx, tx)))
        }),
    ) {
        log::error!("eframe error: {}", e);
    }
}
