//! 配置文件热更新监听模块（Win32 版）。
//! 使用 `notify` crate 监听 `config.toml` 文件变更，经防抖后发送重载命令。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use timer_core::AppCommand;

/// 启动配置文件监听线程。
/// 检测到 `config.toml` 变更后，经 300ms 防抖，发送 `AppCommand::ReloadConfig`。
pub fn spawn_watcher(config_path: PathBuf, cmd_tx: mpsc::Sender<AppCommand>) {
    // 监听父目录（因为编辑器可能通过 rename+write 保存）
    let parent = match config_path.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            log::warn!("config has no parent directory, hot-reload disabled");
            return;
        }
    };

    std::thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || run_watcher(parent, config_path, cmd_tx))
        .expect("failed to spawn config watcher thread");
}

/// 监听线程主循环。
fn run_watcher(watch_dir: PathBuf, config_path: PathBuf, cmd_tx: mpsc::Sender<AppCommand>) {
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

    if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
        log::warn!("failed to watch config directory: {}", e);
        return;
    }

    log::info!("hot-reload active, watching: {}", config_path.display());

    loop {
        // 阻塞等待第一个文件变更事件
        let event = match evt_rx.recv() {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                log::debug!("file watcher event error: {}", e);
                continue;
            }
            Err(_) => break,
        };

        // 只处理目标配置文件的 Modify / Create 事件
        if !is_relevant_event(&event, &config_path) {
            continue;
        }

        // 防抖循环：300ms 内收到新事件则重置计时，超时则触发重载
        loop {
            match evt_rx.recv_timeout(Duration::from_millis(300)) {
                Ok(Ok(e)) if !is_relevant_event(&e, &config_path) => continue,
                Ok(Ok(_)) => continue, // 收到新事件，重置计时
                Ok(Err(e)) => {
                    log::debug!("file watcher event error: {}", e);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break, // 静默期结束 → 触发
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        log::info!("config changed, triggering reload");
        let _ = cmd_tx.send(AppCommand::ReloadConfig);
    }
}

/// 判断文件系统事件是否与目标配置文件相关。
fn is_relevant_event(event: &notify::Event, config_path: &PathBuf) -> bool {
    let path_matches = event.paths.iter().any(|p| p == config_path);
    if !path_matches {
        return false;
    }
    matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
}
