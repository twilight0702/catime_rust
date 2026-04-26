use crate::timer::{TimerMode, TimerStatus};

/// UI 只读状态快照：UI 层不应直接读取引擎内部状态，
/// 只能通过此结构体获取已格式化的展示数据。
#[derive(Debug, Clone)]
pub struct ViewState {
    /// 格式化的时间字符串（MM:SS / HH:MM:SS）
    pub display_time: String,
    pub mode: TimerMode,
    pub status: TimerStatus,
    pub countdown_duration_secs: u64,
    pub window_visible: bool,
}
