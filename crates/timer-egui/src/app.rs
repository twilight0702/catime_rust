use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Color32, FontId, RichText};
use timer_app::AppController;
use timer_core::{AppCommand, AppEvent::*, TimerStatus};
use timer_storage::{ConfigRepository, TomlConfigRepository};

/// egui 应用主结构：持有控制器、命令通道和配置仓库。
/// 每帧从通道中取出命令处理，渲染 UI，并请求每秒重绘。
pub struct CatimeApp {
    controller: AppController,
    rx: Receiver<AppCommand>,
    tx: Sender<AppCommand>,
    config_repo: TomlConfigRepository,
    last_tick: Instant,
}

impl CatimeApp {
    pub fn new(
        controller: AppController,
        rx: Receiver<AppCommand>,
        tx: Sender<AppCommand>,
        config_repo: TomlConfigRepository,
    ) -> Self {
        Self {
            controller,
            rx,
            tx,
            config_repo,
            last_tick: Instant::now(),
        }
    }
}

impl eframe::App for CatimeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_commands(ctx);
        self.auto_tick();
        self.render_ui(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(e) = self.config_repo.save(self.controller.config()) {
            log::error!("failed to save config: {}", e);
        }
    }
}

impl CatimeApp {
    /// 从通道接收命令并处理，直到通道清空
    fn drain_commands(&mut self, ctx: &egui::Context) {
        loop {
            let cmd = match self.rx.try_recv() {
                Ok(cmd) => cmd,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            };

            let events = self.controller.handle(cmd);

            for event in events {
                match event {
                    AppShouldQuit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    TimerFinished => {
                        // 倒计时结束自动显示窗口
                        self.controller.handle(AppCommand::ShowWindow);
                    }
                    _ => {}
                }
            }
        }
    }

    /// 如果计时器正在运行且距离上次 Tick 超过 1 秒，自动推进
    fn auto_tick(&mut self) {
        let vs = self.controller.view_state();
        if vs.status == TimerStatus::Running
            && self.last_tick.elapsed() >= Duration::from_secs(1)
        {
            self.controller.handle(AppCommand::Tick);
            self.last_tick = Instant::now();
        }
    }

    /// 构建 egui UI 布局：模式标题 → 大号时间 → 状态文字 → 操作按钮
    fn render_ui(&self, ctx: &egui::Context) {
        let vs = self.controller.view_state();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);

                let mode_text = match vs.mode {
                    timer_core::TimerMode::Stopwatch => "正计时",
                    timer_core::TimerMode::Countdown => "倒计时",
                };
                ui.label(
                    RichText::new(mode_text)
                        .font(FontId::proportional(16.0))
                        .color(Color32::DARK_GRAY),
                );

                ui.add_space(8.0);

                let time_color = match vs.status {
                    TimerStatus::Running => Color32::BLACK,
                    TimerStatus::Paused => Color32::DARK_GRAY,
                    TimerStatus::Finished => Color32::RED,
                    TimerStatus::Idle => Color32::BLACK,
                };
                ui.label(
                    RichText::new(&vs.display_time)
                        .font(FontId::monospace(56.0))
                        .color(time_color),
                );

                ui.add_space(8.0);

                let status_text = match vs.status {
                    TimerStatus::Idle => "就绪",
                    TimerStatus::Running => "运行中",
                    TimerStatus::Paused => "暂停",
                    TimerStatus::Finished => "已结束",
                };
                ui.label(
                    RichText::new(status_text)
                        .font(FontId::proportional(12.0))
                        .color(Color32::DARK_GRAY),
                );

                ui.add_space(16.0);

                // 按钮行：开始/暂停、重置、模式切换
                ui.horizontal(|ui| {
                    let btn_label = match vs.status {
                        TimerStatus::Running => "暂停",
                        TimerStatus::Paused => "继续",
                        _ => "开始",
                    };

                    if ui.button(RichText::new(btn_label).size(18.0)).clicked() {
                        let cmd = match vs.status {
                            TimerStatus::Running => AppCommand::Pause,
                            TimerStatus::Paused => AppCommand::Resume,
                            _ => AppCommand::Start,
                        };
                        let _ = self.tx.send(cmd);
                    }

                    if ui.button(RichText::new("重置").size(18.0)).clicked() {
                        let _ = self.tx.send(AppCommand::Reset);
                    }

                    let switch_label = match vs.mode {
                        timer_core::TimerMode::Stopwatch => "切倒计时",
                        timer_core::TimerMode::Countdown => "切正计时",
                    };
                    if ui.button(RichText::new(switch_label).size(18.0)).clicked() {
                        let new_mode = match vs.mode {
                            timer_core::TimerMode::Stopwatch => {
                                timer_core::TimerMode::Countdown
                            }
                            timer_core::TimerMode::Countdown => {
                                timer_core::TimerMode::Stopwatch
                            }
                        };
                        let _ = self.tx.send(AppCommand::SwitchMode(new_mode));
                    }
                });
            });
        });
    }
}
