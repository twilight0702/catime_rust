use serde::{Deserialize, Serialize};

fn default_opacity() -> f32 {
    0.85
}

/// 配置文件中的计时模式枚举。
/// 序列化为小写（`"stopwatch"` / `"countdown"`），与 TOML 格式一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimerModeConfig {
    /// 正计时（秒表）
    Stopwatch,
    /// 倒计时
    Countdown,
}

impl TimerModeConfig {
    /// 将配置枚举转换为 `timer_core` 的 `TimerMode`。
    pub fn to_core_mode(&self) -> timer_core::TimerMode {
        match self {
            TimerModeConfig::Stopwatch => timer_core::TimerMode::Stopwatch,
            TimerModeConfig::Countdown => timer_core::TimerMode::Countdown,
        }
    }
}

/// 窗口位置与大小配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// 窗口左上角 X 坐标
    pub x: i32,
    /// 窗口左上角 Y 坐标
    pub y: i32,
    /// 窗口宽度（像素）
    pub width: u32,
    /// 窗口高度（像素）
    pub height: u32,
    /// 记录窗口尺寸时的 DPI（用于跨屏恢复尺寸）
    #[serde(default)]
    pub dpi: Option<u32>,
    /// 是否锁定窗口位置（禁止拖动）
    pub locked: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 320,
            height: 220,
            dpi: None,
            locked: false,
        }
    }
}

/// 计时器显示字体配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    /// 字体名称（如 "Segoe UI"、"Microsoft YaHei"）
    pub family: String,
    /// 字号（像素）
    pub size: f32,
    /// 字体颜色（CSS 十六进制格式，如 "#FFFFFF"）
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

/// 托盘图标左键点击行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLeftClickAction {
    /// 切换窗口可见性
    ToggleWindow,
    /// 显示窗口
    ShowWindow,
    /// 无操作
    Nothing,
}

/// 系统托盘相关配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    /// 左键点击托盘图标的行为
    pub left_click_action: TrayLeftClickAction,
    /// 鼠标悬停时是否显示剩余时间提示
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

/// 顶层应用配置，与 `config.toml` 结构一一对应。
/// 所有 UI 后端共享同一套配置格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 默认计时模式
    pub mode: TimerModeConfig,
    /// 倒计时默认时长（秒）
    pub duration_secs: u64,
    /// 窗口是否置顶
    pub always_on_top: bool,
    /// 窗口不透明度（0.0 ~ 1.0）
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// 是否启用配置文件热更新
    pub hot_reload: bool,
    /// 窗口位置与大小
    pub window: WindowConfig,
    /// 显示字体设置
    pub font: FontConfig,
    /// 系统托盘设置
    pub tray: TrayConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: TimerModeConfig::Countdown,
            duration_secs: 1500, // 默认 25 分钟
            always_on_top: true,
            opacity: default_opacity(),
            hot_reload: true,
            window: WindowConfig::default(),
            font: FontConfig::default(),
            tray: TrayConfig::default(),
        }
    }
}
