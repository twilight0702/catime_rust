//! 数据持久化层：管理 TOML 配置文件的读写。
//! 提供 ConfigRepository trait 以便后续替换为 JSON / SQLite 实现。

pub mod config;
pub mod repository;

pub use config::{AppConfig, FontConfig, TimerModeConfig, TrayConfig, TrayLeftClickAction, WindowConfig};
pub use repository::{ConfigRepository, TomlConfigRepository};
