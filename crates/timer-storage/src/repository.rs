use anyhow::Context;
use std::path::PathBuf;

use crate::config::AppConfig;

/// 配置持久化抽象：不关心存储格式或位置
pub trait ConfigRepository {
    fn load(&self) -> anyhow::Result<AppConfig>;
    fn save(&self, config: &AppConfig) -> anyhow::Result<()>;
}

/// TOML 文件实现：可执行文件同目录下的 config.toml
pub struct TomlConfigRepository {
    path: PathBuf,
}

impl TomlConfigRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 返回可执行文件同目录下的默认路径
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let mut exe_dir = std::env::current_exe()
            .context("failed to get executable path")?;
        exe_dir.pop();
        Ok(exe_dir.join("config.toml"))
    }
}

impl ConfigRepository for TomlConfigRepository {
    fn load(&self) -> anyhow::Result<AppConfig> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                let config: AppConfig = toml::from_str(&content)
                    .with_context(|| format!("failed to parse {}", self.path.display()))?;
                Ok(config)
            }
            // 配置文件不存在 → 写入默认配置并返回
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = AppConfig::default();
                if let Err(save_err) = self.save(&config) {
                    log::warn!("failed to save default config: {}", save_err);
                }
                Ok(config)
            }
            Err(e) => Err(e).context("failed to read config file"),
        }
    }

    fn save(&self, config: &AppConfig) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(config)
            .context("failed to serialize config")?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .context("failed to create config directory")?;
        }
        std::fs::write(&self.path, content)
            .context("failed to write config file")?;
        Ok(())
    }
}
