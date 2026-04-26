use timer_core::{AppCommand, AppEvent, TimerEngine, ViewState};
use timer_storage::{AppConfig, ConfigRepository};

/// 应用协调层：接收命令 → 分发到引擎 → 更新 ViewState。
/// UI / 托盘不应直接调用引擎方法或读写配置。
pub struct AppController {
    engine: TimerEngine,
    config: AppConfig,
    config_repo: Box<dyn ConfigRepository>,
    view_state: ViewState,
}

impl AppController {
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

    /// 处理命令并返回待通知的事件列表
    pub fn handle(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        match cmd {
            AppCommand::Start
            | AppCommand::Pause
            | AppCommand::Resume
            | AppCommand::Reset
            | AppCommand::Tick
            | AppCommand::SetCountdown(_)
            | AppCommand::SwitchMode(_) => self.handle_timer_command(cmd),

            AppCommand::ToggleWindow => self.handle_toggle_window(),
            AppCommand::ShowWindow => self.handle_show_window(),
            AppCommand::HideWindow => self.handle_hide_window(),

            AppCommand::ReloadConfig => self.handle_reload_config(),
            AppCommand::Quit => vec![AppEvent::AppShouldQuit],
        }
    }

    /// 将计时命令转发给引擎，并同步 ViewState
    fn handle_timer_command(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        let events = self.engine.handle(cmd);
        self.sync_view_state(&events);
        self.sync_config_from_engine();
        events
    }

    /// 重新加载配置，将变更即时应用到引擎
    fn handle_reload_config(&mut self) -> Vec<AppEvent> {
        let new_config = match self.config_repo.load() {
            Ok(c) => c,
            Err(e) => {
                log::error!("config reload failed: {}", e);
                return Vec::new();
            }
        };

        let mut events = Vec::new();

        // 模式变更
        if self.config.mode != new_config.mode {
            let core_mode = new_config.mode.to_core_mode();
            events.extend(self.engine.handle(AppCommand::SwitchMode(core_mode)));
        }

        // 倒计时时长变更（仅在倒计时模式下立即生效）
        if self.config.duration_secs != new_config.duration_secs {
            events.extend(
                self.engine
                    .handle(AppCommand::SetCountdown(new_config.duration_secs)),
            );
        }

        self.config = new_config;

        self.sync_view_state(&events);

        if events.is_empty() {
            events.push(AppEvent::TimerUpdated);
        }

        log::info!("config reloaded");
        events
    }

    /// 保存当前配置到文件
    pub fn save_config(&self) -> anyhow::Result<()> {
        self.config_repo.save(&self.config)
    }

    /// 更新窗口位置和尺寸（用于持久化）
    pub fn update_window_bounds(&mut self, x: i32, y: i32, width: u32, height: u32) {
        self.config.window.x = x;
        self.config.window.y = y;
        self.config.window.width = width;
        self.config.window.height = height;
    }

    /// 根据引擎事件同步 ViewState
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

    fn sync_config_from_engine(&mut self) {
        self.config.mode = match self.engine.mode {
            timer_core::TimerMode::Stopwatch => timer_storage::TimerModeConfig::Stopwatch,
            timer_core::TimerMode::Countdown => timer_storage::TimerModeConfig::Countdown,
        };
        self.config.duration_secs = self.engine.countdown_duration_secs;
    }

    fn handle_toggle_window(&mut self) -> Vec<AppEvent> {
        self.view_state.window_visible = !self.view_state.window_visible;
        if self.view_state.window_visible {
            vec![AppEvent::WindowShouldShow]
        } else {
            vec![AppEvent::WindowShouldHide]
        }
    }

    fn handle_show_window(&mut self) -> Vec<AppEvent> {
        if !self.view_state.window_visible {
            self.view_state.window_visible = true;
            vec![AppEvent::WindowShouldShow]
        } else {
            Vec::new()
        }
    }

    fn handle_hide_window(&mut self) -> Vec<AppEvent> {
        if self.view_state.window_visible {
            self.view_state.window_visible = false;
            vec![AppEvent::WindowShouldHide]
        } else {
            Vec::new()
        }
    }

    pub fn view_state(&self) -> &ViewState {
        &self.view_state
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn engine(&self) -> &TimerEngine {
        &self.engine
    }
}
