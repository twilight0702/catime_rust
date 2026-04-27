/// 应用事件：`TimerEngine` 处理命令后产生的事件，通知外部（UI / 托盘）响应。
/// 事件由引擎返回后经 `AppController` 分发给各 UI 组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AppEvent {
    /// 计时数据已更新（时间变化、模式切换等），UI 需重绘显示
    TimerUpdated,
    /// 倒计时归零，可触发提醒/动画
    TimerFinished,
    /// 应显示主窗口
    WindowShouldShow,
    /// 应隐藏主窗口
    WindowShouldHide,
    /// 应用应退出
    AppShouldQuit,
}
