use crate::timer::TimerMode;

/// 应用命令：来自 UI / 托盘 / 系统事件的输入。
/// 每个命令对应一个明确的用户或系统操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    // --- 计时控制 ---
    Start,
    Pause,
    Resume,
    Reset,
    /// 每秒一次的时间推进
    Tick,
    SetCountdown(u64),
    SwitchMode(TimerMode),

    // --- 窗口控制 ---
    ToggleWindow,
    ShowWindow,
    HideWindow,

    // --- 系统 ---
    ReloadConfig,
    Quit,
}
