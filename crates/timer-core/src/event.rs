/// 应用事件：内部状态变化后通知外部（UI / 托盘）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// 计时数据已更新，UI 需重绘
    TimerUpdated,
    /// 倒计时归零
    TimerFinished,
    WindowShouldShow,
    WindowShouldHide,
    AppShouldQuit,
}
