use timer_core::{AppCommand, AppEvent, TimerEngine, ViewState};
use timer_storage::AppConfig;

/// 应用协调层：接收命令 → 分发到引擎 → 更新 ViewState。
/// UI / 托盘不应直接调用引擎方法或读写配置。
pub struct AppController {
    engine: TimerEngine,
    config: AppConfig,
    view_state: ViewState,
}

impl AppController {
    pub fn new(config: AppConfig) -> Self {
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

            AppCommand::ReloadConfig => Vec::new(),
            AppCommand::Quit => vec![AppEvent::AppShouldQuit],
        }
    }

    /// 将计时命令转发给引擎，并同步 ViewState
    fn handle_timer_command(&mut self, cmd: AppCommand) -> Vec<AppEvent> {
        let events = self.engine.handle(cmd);

        for event in &events {
            match event {
                AppEvent::TimerUpdated | AppEvent::TimerFinished => {
                    self.view_state.display_time = self.engine.display_time();
                    self.view_state.mode = self.engine.mode;
                    self.view_state.status = self.engine.status;
                }
                _ => {}
            }
        }

        events
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

    #[allow(dead_code)]
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn engine(&self) -> &TimerEngine {
        &self.engine
    }
}
