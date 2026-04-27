use timer_core::{AppCommand, AppEvent, TimerEngine, ViewState};
use timer_storage::{AppConfig, ConfigRepository};

/// 应用协调层：接收命令 → 分发到引擎 → 更新 ViewState。
///
/// `AppController` 是整个应用的"大脑"：
/// - 持有 `TimerEngine`（计时逻辑）
/// - 持有 `AppConfig`（运行时配置）
/// - 持有 `ConfigRepository`（持久化）
/// - 持有 `ViewState`（UI 只读快照）
///
/// UI / 托盘不应直接调用引擎方法或读写配置，所有操作通过 Controller 中转。
pub struct AppController {
    /// 计时引擎（纯状态机）
    engine: TimerEngine,
    /// 当前运行时配置
    config: AppConfig,
    /// 配置持久化后端
    config_repo: Box<dyn ConfigRepository>,
    /// UI 只读状态快照
    view_state: ViewState,
}

impl AppController {
    /// 创建新的控制器实例。
    /// 根据配置初始化计时引擎和初始 ViewState。
    pub fn new(config: AppConfig, config_repo: Box<dyn ConfigRepository>) -> Self {
        let core_mode = config.mode.to_core_mode();
        let engine = TimerEngine::new(core_mode, config.duration_secs);

        let view_state = ViewState {
            display_time: engine.display_time(),
            mode: core_mode,
            status: engine.status,
            countdown_duration_secs: config.duration_secs,
            window_visible: true,
        };

        Self {
            engine,
            config,
            config_repo,
            view_state,
        }
    }

    /// 处理命令并返回待通知的事件列表。
    /// 这是 UI 层与控制器交互的唯一入口。
    pub fn handle(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        match cmd {
            // 计时相关命令 → 转发给引擎
            AppCommand::Start
            | AppCommand::Pause
            | AppCommand::Resume
            | AppCommand::Reset
            | AppCommand::Tick
            | AppCommand::SetCountdown(_)
            | AppCommand::SwitchMode(_) => self.handle_timer_command(cmd),

            // 窗口控制命令 → 直接处理
            AppCommand::ToggleWindow => self.handle_toggle_window(),
            AppCommand::ShowWindow => self.handle_show_window(),
            AppCommand::HideWindow => self.handle_hide_window(),

            // 系统命令
            AppCommand::ReloadConfig => self.handle_reload_config(),
            AppCommand::Quit => vec![AppEvent::AppShouldQuit],
        }
    }

    /// 将计时命令转发给引擎，同步 ViewState 和 Config。
    fn handle_timer_command(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        let events = self.engine.handle(cmd);
        self.sync_view_state(&events);
        self.sync_config_from_engine();
        events
    }

    /// 重新加载配置文件，将变更即时应用到引擎。
    /// 若模式或倒计时时长变化，向引擎发送对应命令。
    fn handle_reload_config(&mut self) -> Vec<AppEvent> {
        let new_config = match self.config_repo.load() {
            Ok(c) => c,
            Err(e) => {
                log::error!("config reload failed: {}", e);
                return Vec::new();
            }
        };

        let mut events = Vec::new();

        // 模式变更 → 发送 SwitchMode
        if self.config.mode != new_config.mode {
            let core_mode = new_config.mode.to_core_mode();
            events.extend(self.engine.handle(AppCommand::SwitchMode(core_mode)));
        }

        // 倒计时时长变更 → 发送 SetCountdown
        if self.config.duration_secs != new_config.duration_secs {
            events.extend(
                self.engine
                    .handle(AppCommand::SetCountdown(new_config.duration_secs)),
            );
        }

        self.config = new_config;

        self.sync_view_state(&events);

        // 确保至少有一次 UI 刷新
        if events.is_empty() {
            events.push(AppEvent::TimerUpdated);
        }

        log::info!("config reloaded");
        events
    }

    /// 保存当前配置到持久化存储。
    pub fn save_config(&self) -> anyhow::Result<()> {
        self.config_repo.save(&self.config)
    }

    /// 更新窗口位置和尺寸（由 UI 层在窗口移动/缩放时调用）。
    pub fn update_window_bounds(&mut self, x: i32, y: i32, width: u32, height: u32) {
        self.config.window.x = x;
        self.config.window.y = y;
        self.config.window.width = width;
        self.config.window.height = height;
    }

    /// 根据引擎事件同步 ViewState。
    /// 仅在 TimerUpdated / TimerFinished 时刷新，避免不必要的更新。
    fn sync_view_state(&mut self, events: &[AppEvent]) {
        let needs_sync = events
            .iter()
            .any(|e| matches!(e, AppEvent::TimerUpdated | AppEvent::TimerFinished));
        if needs_sync {
            self.view_state.display_time = self.engine.display_time();
            self.view_state.mode = self.engine.mode;
            self.view_state.status = self.engine.status;
            self.view_state.countdown_duration_secs = self.engine.countdown_duration_secs;
        }
    }

    /// 将引擎当前状态回写到 Config（用于持久化保存）。
    fn sync_config_from_engine(&mut self) {
        self.config.mode = match self.engine.mode {
            timer_core::TimerMode::Stopwatch => timer_storage::TimerModeConfig::Stopwatch,
            timer_core::TimerMode::Countdown => timer_storage::TimerModeConfig::Countdown,
        };
        self.config.duration_secs = self.engine.countdown_duration_secs;
    }

    /// 切换窗口可见性。
    fn handle_toggle_window(&mut self) -> Vec<AppEvent> {
        self.view_state.window_visible = !self.view_state.window_visible;
        if self.view_state.window_visible {
            vec![AppEvent::WindowShouldShow]
        } else {
            vec![AppEvent::WindowShouldHide]
        }
    }

    /// 显示窗口（幂等：已可见时无操作）。
    fn handle_show_window(&mut self) -> Vec<AppEvent> {
        if !self.view_state.window_visible {
            self.view_state.window_visible = true;
            vec![AppEvent::WindowShouldShow]
        } else {
            Vec::new()
        }
    }

    /// 隐藏窗口（幂等：已隐藏时无操作）。
    fn handle_hide_window(&mut self) -> Vec<AppEvent> {
        if self.view_state.window_visible {
            self.view_state.window_visible = false;
            vec![AppEvent::WindowShouldHide]
        } else {
            Vec::new()
        }
    }

    /// 获取当前 ViewState 的不可变引用。
    pub fn view_state(&self) -> &ViewState {
        &self.view_state
    }

    /// 获取当前配置的不可变引用。
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 获取引擎的不可变引用（仅供 UI 层读取额外信息）。
    pub fn engine(&self) -> &TimerEngine {
        &self.engine
    }
}
