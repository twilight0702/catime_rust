//! 配置文件热更新监听模块（Win32 版）。
//! 使用 `notify` crate 监听 `config.toml` 所在目录的文件变更，防抖后发送重载命令。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use timer_core::AppCommand;

/// 启动配置文件监听线程。
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

/// 判断事件是否与目标配置文件相关。
/// 使用 `canonicalize` 做鲁棒的路径比较（兼容相对/绝对路径、大小写等差异）。
fn paths_match(event_path: &std::path::Path, config_path: &std::path::Path) -> bool {
    // 快速路径：直接相等
    if event_path == config_path {
        return true;
    }
    // Canonicalize 比较（处理符号链接、大小写等）
    if let (Ok(a), Ok(b)) = (event_path.canonicalize(), config_path.canonicalize()) {
        return a == b;
    }
    false
}

fn is_relevant(event: &notify::Event, config_path: &std::path::Path) -> bool {
    let kind_match = matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
    if !kind_match {
        return false;
    }
    event.paths.iter().any(|p| paths_match(p, config_path))
}

/// 监听线程主循环：100ms 轮询，300ms 防抖窗口。
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

    log::info!(
        "hot-reload active, watching dir: {}, target: {}",
        watch_dir.display(),
        config_path.display()
    );

    let mut last_relevant: Option<Instant> = None;

    loop {
        match evt_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) if is_relevant(&event, &config_path) => {
                log::debug!(
                    "watcher: relevant event kind={:?} paths={:?}",
                    event.kind,
                    event.paths
                );
                last_relevant = Some(Instant::now());
            }
            Ok(Ok(event)) => {
                // 无关事件，记录以便排查
                log::trace!(
                    "watcher: ignored event kind={:?} paths={:?}",
                    event.kind,
                    event.paths
                );
            }
            Ok(Err(e)) => {
                log::debug!("watcher: event error: {}", e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(ts) = last_relevant {
                    let elapsed = ts.elapsed();
                    if elapsed >= Duration::from_millis(300) {
                        log::info!(
                            "config changed (debounced after {:?}), triggering reload",
                            elapsed
                        );
                        if cmd_tx.send(AppCommand::ReloadConfig).is_err() {
                            log::info!("watcher: main thread disconnected, exiting");
                            break;
                        }
                        last_relevant = None;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}
