mod app;
mod tray;

use std::sync::mpsc;

use egui::{FontData, FontDefinitions, FontFamily};
use timer_app::AppController;
use timer_storage::{ConfigRepository, TomlConfigRepository};

use app::CatimeApp;

/// 加载 Windows 系统中的中文字体（微软雅黑 / 宋体 / 微软正黑），
/// 注册为 egui 的首选字体族，使中文能正常显示。
fn setup_cjk_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    let cjk_paths = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\msjh.ttc",
    ];

    for path in &cjk_paths {
        if let Ok(data) = std::fs::read(path) {
            log::info!("loaded CJK font: {}", path);
            let mut font_data = FontData::from_owned(data);
            font_data.index = 0;
            fonts
                .font_data
                .insert("CJK".to_owned(), std::sync::Arc::new(font_data));

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
    env_logger::init();

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

    let controller = AppController::new(config);

    let (tx, rx) = mpsc::channel::<timer_core::AppCommand>();

    // 在主线程创建托盘，与 eframe 共用 Windows 消息泵，确保点击事件能分发。
    // Box::leak 防止 TrayIcon 被析构导致图标消失。
    let _tray = Box::leak(Box::new(tray::create_tray(tx.clone())));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([300.0, 200.0])
            .with_title("Catime"),
        ..Default::default()
    };

    let fonts = setup_cjk_fonts();

    if let Err(e) = eframe::run_native(
        "Catime",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_fonts(fonts.clone());
            Ok(Box::new(CatimeApp::new(controller, rx, tx, config_repo)))
        }),
    ) {
        log::error!("eframe error: {}", e);
    }
}
