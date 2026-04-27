use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Color32, FontId, RichText};
use timer_app::AppController;
use timer_core::{AppCommand, AppEvent::*, TimerStatus};

/// egui 应用主结构：持有控制器和命令通道。
/// 每帧从通道中取出命令处理，渲染 UI，并请求每秒重绘以驱动 Tick。
pub struct CatimeApp {
    /// 应用协调器
    controller: AppController,
    /// 命令接收端（来自托盘 / 文件监听器）
    rx: Receiver<AppCommand>,
    /// 命令发送端（按钮点击时发送命令给自己）
    tx: Sender<AppCommand>,
    /// 上次 Tick 时刻，用于 1 秒间隔控制
    last_tick: Instant,
}

impl CatimeApp {
    pub fn new(
        controller: AppController,
        rx: Receiver<AppCommand>,
        tx: Sender<AppCommand>,
    ) -> Self {
        Self {
            controller,
            rx,
            tx,
            last_tick: Instant::now(),
        }
    }
}

impl eframe::App for CatimeApp {
    /// 每帧回调：处理命令 → 自动推进 → 渲染 UI → 请求下一秒重绘。
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_commands(ctx);
        self.auto_tick();
        self.render_ui(ctx);
        // 确保每秒至少重绘一次，驱动 auto_tick
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    /// 退出时保存配置。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(e) = self.controller.save_config() {
            log::error!("failed to save config: {}", e);
        }
    }
}

impl CatimeApp {
    /// 从命令通道取出所有待处理命令并逐一执行。
    /// 非阻塞：通道为空时立即返回。
    fn drain_commands(&mut self, ctx: &egui::Context) {
        loop {
            let cmd = match self.rx.try_recv() {
                Ok(cmd) => cmd,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            };

            let events = self.controller.handle(cmd);

            // 处理控制器返回的事件
            for event in events {
                match event {
                    AppShouldQuit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    TimerFinished => {
                        // 倒计时结束时自动弹出窗口
                        self.controller.handle(AppCommand::ShowWindow);
                    }
                    _ => {}
                }
            }
        }
    }

    /// 若计时器 Running 且距上次 Tick 满 1 秒，自动发送 Tick 命令。
    fn auto_tick(&mut self) {
        let vs = self.controller.view_state();
        if vs.status == TimerStatus::Running
            && self.last_tick.elapsed() >= Duration::from_secs(1)
        {
            self.controller.handle(AppCommand::Tick);
            self.last_tick = Instant::now();
        }
    }

    /// 构建 egui UI 布局：模式标签 → 时间显示 → 状态标签 → 按钮行。
    fn render_ui(&self, ctx: &egui::Context) {
        let vs = self.controller.view_state();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);

                // 计时模式标签
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

                // 时间显示：根据状态改变颜色
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

                // 状态标签
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

                // 按钮行：开始/暂停/继续 | 重置 | 切换模式
                ui.horizontal(|ui| {
                    // 主操作按钮：根据状态动态切换文字和命令
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

                    // 模式切换按钮
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
