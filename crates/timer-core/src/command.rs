use crate::timer::TimerMode;

/// 应用命令：来自 UI / 托盘 / 系统事件的输入。
/// 每个命令对应一个明确的用户或系统操作。
/// 命令由 UI 层发出，经 `AppController` 路由到 `TimerEngine` 处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AppCommand {
    // --- 计时控制 ---
    /// 开始计时（Idle/Finished → Running，Paused → Resume）
    Start,
    /// 暂停计时（Running → Paused）
    Pause,
    /// 从暂停恢复（Paused → Running）
    Resume,
    /// 重置计时器到初始状态（→ Idle）
    Reset,
    /// 每秒一次的时间推进，由外部 tick 线程驱动
    Tick,
    /// 设置倒计时时长（秒）
    SetCountdown(u64),
    /// 切换计时模式（正计时 ↔ 倒计时），切换后回到 Idle
    SwitchMode(TimerMode),

    // --- 窗口控制 ---
    /// 切换窗口可见性
    ToggleWindow,
    /// 显示窗口
    ShowWindow,
    /// 隐藏窗口
    HideWindow,
    /// 托盘左键点击（由配置决定具体行为）
    TrayLeftClick,

    // --- 系统 ---
    /// 重新加载配置文件
    ReloadConfig,
    /// 退出应用
    Quit,
}
