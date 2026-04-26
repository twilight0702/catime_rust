use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use timer_core::AppCommand;

/// 启动配置文件监听线程。
///
/// 检测到 config.toml 变更后，经 300ms 防抖，发送 AppCommand::ReloadConfig。
/// 所有错误仅记日志，不会导致程序崩溃。
pub fn spawn_watcher(config_path: PathBuf, cmd_tx: mpsc::Sender<AppCommand>) {
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

fn run_watcher(
    watch_dir: PathBuf,
    config_path: PathBuf,
    cmd_tx: mpsc::Sender<AppCommand>,
) {
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
        // 等待第一个文件变更事件
        let event = match evt_rx.recv() {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                log::debug!("file watcher event error: {}", e);
                continue;
            }
            Err(_) => break,
        };

        // 只处理 Modify / Create 事件，且路径匹配 config.toml
        if !is_relevant_event(&event, &config_path) {
            continue;
        }

        // 防抖：300ms 内不再收到新事件才触发重载
        loop {
            match evt_rx.recv_timeout(Duration::from_millis(300)) {
                Ok(Ok(e)) if !is_relevant_event(&e, &config_path) => continue,
                Ok(Ok(_)) => continue, // 收到新事件，重置计时
                Ok(Err(e)) => {
                    log::debug!("file watcher event error: {}", e);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break, // 300ms 静默 → 触发
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        log::info!("config changed, triggering reload");
        let _ = cmd_tx.send(AppCommand::ReloadConfig);
    }
}

/// 判断事件是否与目标配置文件相关
fn is_relevant_event(
    event: &notify::Event,
    config_path: &PathBuf,
) -> bool {
    let path_matches = event.paths.iter().any(|p| p == config_path);
    if !path_matches {
        return false;
    }
    matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_)
    )
}
