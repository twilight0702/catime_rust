use crate::command::AppCommand;
use crate::event::AppEvent;

/// 计时模式：正计时（秒表）或倒计时。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TimerMode {
    /// 正计时：从 0 开始累计，无上限
    Stopwatch,
    /// 倒计时：从设定时长倒数到 0
    Countdown,
}

/// 计时器运行状态。
///
/// 状态转换图：
/// ```text
/// Idle ──Start──▶ Running ──Pause──▶ Paused
///   ▲              │    ▲              │
///   │              │    └──Resume──────┘
///   │              │
///   └──Reset───────┘    (从 Running/Paused/Finished)
///
/// Running ──(remaining==0)──▶ Finished ──Start/Reset──▶ Idle
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TimerStatus {
    /// 就绪（重置后或初始状态）
    Idle,
    /// 运行中（正在计时）
    Running,
    /// 暂停（可恢复继续）
    Paused,
    /// 倒计时归零（仅倒计时模式可达此状态）
    Finished,
}

/// 计时引擎：纯状态机，不依赖任何时间源。
/// 外部负责以合适的频率（如每秒一次）发送 `Tick` 命令来推进时间。
/// 引擎本身不启动线程、不使用 `sleep`、不访问系统时钟。
pub struct TimerEngine {
    /// 当前计时模式
    pub mode: TimerMode,
    /// 当前运行状态
    pub status: TimerStatus,
    /// 正计时模式下累计的秒数
    pub elapsed_secs: u64,
    /// 倒计时模式下剩余的秒数
    pub remaining_secs: u64,
    /// 倒计时总时长（秒），用于切换模式或重置后恢复
    pub countdown_duration_secs: u64,
}

impl TimerEngine {
    /// 创建新的计时引擎实例。
    ///
    /// - `mode`: 初始计时模式
    /// - `duration_secs`: 倒计时总时长（正计时模式下可传 0）
    pub fn new(mode: TimerMode, duration_secs: u64) -> Self {
        let remaining = if mode == TimerMode::Countdown {
            duration_secs
        } else {
            0
        };

        Self {
            mode,
            status: TimerStatus::Idle,
            elapsed_secs: 0,
            remaining_secs: remaining,
            countdown_duration_secs: duration_secs,
        }
    }

    /// 处理一条命令，返回由此产生的事件列表。
    /// 这是引擎的唯一对外入口——所有状态变更都通过此方法驱动。
    pub fn handle(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        match cmd {
            AppCommand::Start => self.handle_start(),
            AppCommand::Pause => self.handle_pause(),
            AppCommand::Resume => self.handle_resume(),
            AppCommand::Reset => self.handle_reset(),
            AppCommand::Tick => self.handle_tick(),
            AppCommand::SetCountdown(n) => self.handle_set_countdown(n),
            AppCommand::SwitchMode(mode) => self.handle_switch_mode(mode),
            _ => Vec::new(),
        }
    }

    /// 处理 Start 命令。
    /// - Idle/Finished → 重置计数器并进入 Running
    /// - Paused → 等同于 Resume（继续计时）
    /// - Running → 无操作（幂等）
    fn handle_start(&mut self) -> Vec<AppEvent> {
        match self.status {
            TimerStatus::Idle | TimerStatus::Finished => {
                if self.mode == TimerMode::Countdown {
                    self.remaining_secs = self.countdown_duration_secs;
                } else {
                    self.elapsed_secs = 0;
                }
                self.status = TimerStatus::Running;
                vec![AppEvent::TimerUpdated]
            }
            TimerStatus::Paused => self.handle_resume(),
            TimerStatus::Running => Vec::new(),
        }
    }

    /// 处理 Pause 命令：仅在 Running 状态下有效，进入 Paused。
    fn handle_pause(&mut self) -> Vec<AppEvent> {
        match self.status {
            TimerStatus::Running => {
                self.status = TimerStatus::Paused;
                vec![AppEvent::TimerUpdated]
            }
            _ => Vec::new(),
        }
    }

    /// 处理 Resume 命令：仅在 Paused 状态下有效，恢复 Running。
    fn handle_resume(&mut self) -> Vec<AppEvent> {
        match self.status {
            TimerStatus::Paused => {
                self.status = TimerStatus::Running;
                vec![AppEvent::TimerUpdated]
            }
            _ => Vec::new(),
        }
    }

    /// 处理 Reset 命令：将计时器恢复到初始 Idle 状态。
    /// 倒计时模式恢复满时长，正计时模式清零。
    fn handle_reset(&mut self) -> Vec<AppEvent> {
        if self.mode == TimerMode::Countdown {
            self.remaining_secs = self.countdown_duration_secs;
        } else {
            self.elapsed_secs = 0;
        }
        self.status = TimerStatus::Idle;
        vec![AppEvent::TimerUpdated]
    }

    /// 处理 Tick 命令：推进 1 秒。
    /// - 仅在 Running 状态下生效
    /// - 正计时：`elapsed_secs += 1`
    /// - 倒计时：`remaining_secs -= 1`，归零时进入 Finished 并触发 `TimerFinished` 事件
    fn handle_tick(&mut self) -> Vec<AppEvent> {
        match self.status {
            TimerStatus::Running => match self.mode {
                TimerMode::Stopwatch => {
                    self.elapsed_secs += 1;
                    vec![AppEvent::TimerUpdated]
                }
                TimerMode::Countdown => {
                    if self.remaining_secs > 0 {
                        self.remaining_secs -= 1;
                        if self.remaining_secs == 0 {
                            self.status = TimerStatus::Finished;
                            vec![AppEvent::TimerFinished]
                        } else {
                            vec![AppEvent::TimerUpdated]
                        }
                    } else {
                        Vec::new()
                    }
                }
            },
            _ => Vec::new(),
        }
    }

    /// 设置倒计时时长。
    /// - 更新 `countdown_duration_secs` 基准值
    /// - 若当前处于 Idle 状态且为倒计时模式，立即更新 `remaining_secs`
    /// - 运行中只更新基准值，下次 Reset 后生效
    fn handle_set_countdown(&mut self, secs: u64) -> Vec<AppEvent> {
        self.countdown_duration_secs = secs;
        if self.mode == TimerMode::Countdown && self.status == TimerStatus::Idle {
            self.remaining_secs = secs;
        }
        vec![AppEvent::TimerUpdated]
    }

    /// 切换计时模式（正计时 ↔ 倒计时）。
    /// 切换后回到 Idle 状态，计数器根据新模式初始化。
    fn handle_switch_mode(&mut self, mode: TimerMode) -> Vec<AppEvent> {
        self.mode = mode;
        self.status = TimerStatus::Idle;
        if mode == TimerMode::Countdown {
            self.remaining_secs = self.countdown_duration_secs;
            self.elapsed_secs = 0;
        } else {
            self.elapsed_secs = 0;
            self.remaining_secs = 0;
        }
        vec![AppEvent::TimerUpdated]
    }

    /// 获取当前模式下的展示时间字符串（MM:SS 或 HH:MM:SS）。
    /// 正计时返回 `elapsed_secs`，倒计时返回 `remaining_secs`。
    pub fn display_time(&self) -> String {
        let total_secs = match self.mode {
            TimerMode::Stopwatch => self.elapsed_secs,
            TimerMode::Countdown => self.remaining_secs,
        };

        Self::format_duration(total_secs)
    }

    /// 将总秒数格式化为 MM:SS 或 HH:MM:SS。
    /// 小于 1 小时使用 MM:SS，否则使用 HH:MM:SS。
    pub fn format_duration(total_secs: u64) -> String {
        let h = total_secs / 3600;
        let m = (total_secs % 3600) / 60;
        let s = total_secs % 60;

        if h > 0 {
            format!("{:02}:{:02}:{:02}", h, m, s)
        } else {
            format!("{:02}:{:02}", m, s)
        }
    }

    /// 获取当前模式的显示标签（中文）。
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            TimerMode::Stopwatch => "正计时",
            TimerMode::Countdown => "倒计时",
        }
    }

    /// 获取当前状态的显示标签（中文）。
    pub fn status_label(&self) -> &'static str {
        match self.status {
            TimerStatus::Idle => "就绪",
            TimerStatus::Running => "运行中",
            TimerStatus::Paused => "暂停",
            TimerStatus::Finished => "已结束",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_under_one_hour() {
        assert_eq!(TimerEngine::format_duration(0), "00:00");
        assert_eq!(TimerEngine::format_duration(5), "00:05");
        assert_eq!(TimerEngine::format_duration(60), "01:00");
        assert_eq!(TimerEngine::format_duration(1500), "25:00");
        assert_eq!(TimerEngine::format_duration(3599), "59:59");
    }

    #[test]
    fn format_duration_over_one_hour() {
        assert_eq!(TimerEngine::format_duration(3600), "01:00:00");
        assert_eq!(TimerEngine::format_duration(3661), "01:01:01");
        assert_eq!(TimerEngine::format_duration(45296), "12:34:56");
    }

    #[test]
    fn new_stopwatch_starts_idle_at_zero() {
        let engine = TimerEngine::new(TimerMode::Stopwatch, 0);
        assert_eq!(engine.mode, TimerMode::Stopwatch);
        assert_eq!(engine.status, TimerStatus::Idle);
        assert_eq!(engine.elapsed_secs, 0);
        assert_eq!(engine.display_time(), "00:00");
    }

    #[test]
    fn new_countdown_starts_idle_at_duration() {
        let engine = TimerEngine::new(TimerMode::Countdown, 1500);
        assert_eq!(engine.mode, TimerMode::Countdown);
        assert_eq!(engine.status, TimerStatus::Idle);
        assert_eq!(engine.remaining_secs, 1500);
        assert_eq!(engine.display_time(), "25:00");
    }

    #[test]
    fn countdown_full_cycle() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 3);

        let events = engine.handle(AppCommand::Start);
        assert_eq!(engine.status, TimerStatus::Running);
        assert_eq!(events, vec![AppEvent::TimerUpdated]);

        let events = engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 2);
        assert_eq!(events, vec![AppEvent::TimerUpdated]);

        let events = engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 1);
        assert_eq!(events, vec![AppEvent::TimerUpdated]);

        let events = engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 0);
        assert_eq!(engine.status, TimerStatus::Finished);
        assert_eq!(events, vec![AppEvent::TimerFinished]);
    }

    #[test]
    fn stopwatch_full_cycle() {
        let mut engine = TimerEngine::new(TimerMode::Stopwatch, 0);

        engine.handle(AppCommand::Start);
        assert_eq!(engine.status, TimerStatus::Running);

        engine.handle(AppCommand::Tick);
        assert_eq!(engine.elapsed_secs, 1);
        assert_eq!(engine.display_time(), "00:01");

        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Tick);
        assert_eq!(engine.elapsed_secs, 3);
    }

    #[test]
    fn pause_and_resume() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 8);

        engine.handle(AppCommand::Pause);
        assert_eq!(engine.status, TimerStatus::Paused);

        engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 8);

        engine.handle(AppCommand::Resume);
        assert_eq!(engine.status, TimerStatus::Running);

        engine.handle(AppCommand::Tick);
        assert_eq!(engine.remaining_secs, 7);
    }

    #[test]
    fn start_from_paused_is_resume() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Pause);
        engine.handle(AppCommand::Start);
        assert_eq!(engine.status, TimerStatus::Running);
    }

    #[test]
    fn double_start_is_noop() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        let events = engine.handle(AppCommand::Start);
        assert!(events.is_empty());
        assert_eq!(engine.status, TimerStatus::Running);
    }

    #[test]
    fn pause_when_not_running_is_noop() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        let events = engine.handle(AppCommand::Pause);
        assert!(events.is_empty());
    }

    #[test]
    fn tick_when_not_running_is_noop() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        let events = engine.handle(AppCommand::Tick);
        assert!(events.is_empty());
    }

    #[test]
    fn reset_from_running() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Reset);

        assert_eq!(engine.status, TimerStatus::Idle);
        assert_eq!(engine.remaining_secs, 10);
        assert_eq!(engine.display_time(), "00:10");
    }

    #[test]
    fn reset_from_finished() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 2);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Tick);
        assert_eq!(engine.status, TimerStatus::Finished);

        engine.handle(AppCommand::Reset);
        assert_eq!(engine.status, TimerStatus::Idle);
        assert_eq!(engine.remaining_secs, 2);
    }

    #[test]
    fn switch_mode_resets() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::SwitchMode(TimerMode::Stopwatch));

        assert_eq!(engine.mode, TimerMode::Stopwatch);
        assert_eq!(engine.status, TimerStatus::Idle);
        assert_eq!(engine.elapsed_secs, 0);
    }

    #[test]
    fn set_countdown_updates_duration() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::SetCountdown(60));

        assert_eq!(engine.countdown_duration_secs, 60);
        assert_eq!(engine.remaining_secs, 60);
        assert_eq!(engine.display_time(), "01:00");
    }

    #[test]
    fn set_countdown_does_not_affect_running_timer() {
        let mut engine = TimerEngine::new(TimerMode::Countdown, 10);
        engine.handle(AppCommand::Start);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::Tick);
        engine.handle(AppCommand::SetCountdown(60));

        // 运行中修改时长只更新 base，不改变剩余时间和状态
        assert_eq!(engine.countdown_duration_secs, 60);
        assert_eq!(engine.remaining_secs, 8);
        assert_eq!(engine.status, TimerStatus::Running);
    }

    #[test]
    fn stopwatch_never_finishes() {
        let mut engine = TimerEngine::new(TimerMode::Stopwatch, 0);
        engine.handle(AppCommand::Start);
        for _ in 0..100 {
            engine.handle(AppCommand::Tick);
        }
        assert_eq!(engine.status, TimerStatus::Running);
        assert_eq!(engine.elapsed_secs, 100);
    }

    #[test]
    fn labels() {
        let countdown = TimerEngine::new(TimerMode::Countdown, 10);
        assert_eq!(countdown.mode_label(), "\u{5012}\u{8ba1}\u{65f6}"); // 倒计时

        let stopwatch = TimerEngine::new(TimerMode::Stopwatch, 0);
        assert_eq!(stopwatch.mode_label(), "\u{6b63}\u{8ba1}\u{65f6}"); // 正计时
    }
}
