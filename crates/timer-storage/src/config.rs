use serde::{Deserialize, Serialize};

/// TOML 配置文件中的计时模式枚举（小写）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimerModeConfig {
    Stopwatch,
    Countdown,
}

impl TimerModeConfig {
    pub fn to_core_mode(&self) -> timer_core::TimerMode {
        match self {
            TimerModeConfig::Stopwatch => timer_core::TimerMode::Stopwatch,
            TimerModeConfig::Countdown => timer_core::TimerMode::Countdown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub locked: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 300,
            height: 120,
            locked: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub color: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Segoe UI".into(),
            size: 56.0,
            color: "#FFFFFF".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLeftClickAction {
    ToggleWindow,
    ShowWindow,
    Nothing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    pub left_click_action: TrayLeftClickAction,
    pub show_remaining_tooltip: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            left_click_action: TrayLeftClickAction::ToggleWindow,
            show_remaining_tooltip: true,
        }
    }
}

/// 顶层应用配置，与 config.toml 一一对应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mode: TimerModeConfig,
    /// 倒计时默认时长（秒）
    pub duration_secs: u64,
    pub always_on_top: bool,
    pub opacity: f32,
    /// 配置热更新开关（第二阶段实现）
    pub hot_reload: bool,
    pub window: WindowConfig,
    pub font: FontConfig,
    pub tray: TrayConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: TimerModeConfig::Countdown,
            duration_secs: 1500,
            always_on_top: true,
            opacity: 0.85,
            hot_reload: true,
            window: WindowConfig::default(),
            font: FontConfig::default(),
            tray: TrayConfig::default(),
        }
    }
}
