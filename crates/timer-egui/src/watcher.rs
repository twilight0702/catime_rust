//! 配置文件热更新监听模块。
//! 使用 `notify` crate 监听 `config.toml` 的文件变更，经防抖后触发重载。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::Context;
use notify::{EventKind, RecursiveMode, Watcher};

use timer_core::AppCommand;

use crate::ui_command::UiCommand;

/// 启动配置文件监听线程。
///
/// 检测到 `config.toml` 变更后，经 300ms 防抖，发送 `AppCommand::ReloadConfig`。
/// 所有错误仅记日志，不会导致程序崩溃。
pub fn spawn_watcher(config_path: PathBuf, cmd_tx: mpsc::Sender<UiCommand>, repaint_ctx: Context) {
    // 监听配置文件的父目录（因为编辑器可能通过 rename+write 保存）
    let parent = match config_path.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            log::warn!("config has no parent directory, hot-reload disabled");
            return;
        }
    };

    std::thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || run_watcher(parent, config_path, cmd_tx, repaint_ctx))
        .expect("failed to spawn config watcher thread");
}

/// 监听线程主循环。
fn run_watcher(
    watch_dir: PathBuf,
    config_path: PathBuf,
    cmd_tx: mpsc::Sender<UiCommand>,
    repaint_ctx: Context,
) {
    // 创建内部通道转发 notify 事件
    let (evt_tx, evt_rx) = mpsc::channel();

    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = evt_tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("failed to create file watcher: {}", e);
            return;
        }
    };

    // 只监听目录本身，不递归
    if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
        log::warn!("failed to watch config directory: {}", e);
        return;
    }

    log::info!("hot-reload active, watching: {}", config_path.display());

    let mut last_relevant: Option<Instant> = None;

    loop {
        match evt_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) if is_relevant_event(&event, &config_path) => {
                last_relevant = Some(Instant::now());
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                log::debug!("file watcher event error: {}", e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(ts) = last_relevant {
                    if ts.elapsed() >= Duration::from_millis(300) {
                        log::info!("config changed, triggering reload");
                        let _ = cmd_tx.send(UiCommand::Core(AppCommand::ReloadConfig));
                        repaint_ctx.request_repaint();
                        last_relevant = None;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn paths_match(event_path: &std::path::Path, config_path: &std::path::Path) -> bool {
    if event_path == config_path {
        return true;
    }
    if let (Ok(a), Ok(b)) = (event_path.canonicalize(), config_path.canonicalize()) {
        return a == b;
    }
    false
}

/// 判断文件系统事件是否与目标配置文件相关。
/// 仅 `Modify` 或 `Create` 且路径匹配时才返回 true。
fn is_relevant_event(event: &notify::Event, config_path: &PathBuf) -> bool {
    if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
        return false;
    }
    event
        .paths
        .iter()
        .any(|p| paths_match(p, config_path.as_path()))
}
