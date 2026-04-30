use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Align2, Color32, FontId, RichText, Sense, Vec2};
use timer_app::AppController;
use timer_core::{AppCommand, AppEvent::*, TimerStatus};

use crate::ui_command::UiCommand;

const ERROR_LOG_FILE: &str = "catime_error.log";
const MIN_WINDOW_WIDTH: u32 = 320;
const MIN_WINDOW_HEIGHT: u32 = 280;

fn append_error_file(level: &str, msg: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let path = match std::env::current_exe() {
        Ok(mut p) => {
            p.pop();
            p.push(ERROR_LOG_FILE);
            p
        }
        Err(_) => return,
    };

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = std::io::Write::write_all(
            &mut f,
            format!("[{}][{}][app-egui] {}\n", ts, level, msg).as_bytes(),
        );
    }
}

/// egui 应用主结构：持有控制器和命令通道。
/// 每帧从通道中取出命令处理，渲染 UI，并请求每秒重绘以驱动 Tick。
pub struct CatimeApp {
    /// 应用协调器
    controller: AppController,
    /// 命令接收端（来自托盘 / 文件监听器）
    rx: Receiver<UiCommand>,
    /// 命令发送端（按钮点击时发送命令给自己）
    tx: Sender<UiCommand>,
    show_countdown_dialog: bool,
    countdown_input: String,
    countdown_error: Option<String>,
    show_opacity_dialog: bool,
    opacity_input: String,
    opacity_error: Option<String>,
    quitting: bool,
    pending_dialog_repaint_frames: u8,
    requested_hide: bool,
    last_applied_visible: Option<bool>,
    last_applied_opacity_alpha: Option<u8>,
    last_saved_window: Option<(i32, i32, u32, u32)>,
}

impl CatimeApp {
    pub fn new(
        controller: AppController,
        rx: Receiver<UiCommand>,
        tx: Sender<UiCommand>,
    ) -> Self {
        Self {
            countdown_input: String::new(),
            countdown_error: None,
            show_opacity_dialog: false,
            opacity_input: String::new(),
            opacity_error: None,
            quitting: false,
            pending_dialog_repaint_frames: 0,
            controller,
            rx,
            tx,
            requested_hide: false,
            show_countdown_dialog: false,
            last_applied_visible: None,
            last_applied_opacity_alpha: None,
            last_saved_window: None,
        }
    }
}

impl eframe::App for CatimeApp {
    /// 每帧回调：处理命令 → 自动推进 → 渲染 UI → 请求下一秒重绘。
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            self.handle_core_command(AppCommand::HideWindow, ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.drain_commands(ctx);
        self.apply_visibility(ctx);
        self.apply_opacity(ctx);
        self.sync_window_bounds(ctx);
        self.render_ui(ctx);
        self.render_countdown_dialog(ctx);
        self.render_opacity_dialog(ctx);
        self.drive_pending_dialog_repaint(ctx);
    }

    /// 退出时保存配置。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(e) = self.controller.save_config() {
            log::error!("failed to save config: {}", e);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
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
            match cmd {
                UiCommand::Core(cmd) => {
                    if !matches!(cmd, AppCommand::Tick) {
                        append_error_file("INFO", &format!("drain command: {:?}", cmd));
                    }
                    self.handle_core_command(cmd, ctx)
                }
                UiCommand::OpenSetCountdownDialog => {
                    self.prepare_dialog_open(ctx);
                    self.show_countdown_dialog = true;
                    self.countdown_error = None;
                    self.countdown_input = timer_core::TimerEngine::format_duration(
                        self.controller.view_state().countdown_duration_secs,
                    );
                }
                UiCommand::OpenSetOpacityDialog => {
                    self.prepare_dialog_open(ctx);
                    self.show_opacity_dialog = true;
                    self.opacity_error = None;
                    self.opacity_input = format!("{:.2}", self.controller.config().opacity);
                }
            }
        }
    }

    fn handle_core_command(&mut self, cmd: AppCommand, ctx: &egui::Context) {
        let events = self.controller.handle(cmd);
        self.process_events(events, ctx);
    }

    fn process_events(&mut self, events: Vec<timer_core::AppEvent>, ctx: &egui::Context) {
        for event in events {
            match event {
                AppShouldQuit => {
                    append_error_file("INFO", "received AppShouldQuit event");
                    self.quitting = true;
                    if let Err(e) = self.controller.save_config() {
                        log::error!("failed to save config before quit: {}", e);
                        append_error_file("ERROR", &format!("save config before quit failed: {}", e));
                    }
                    // egui 窗口关闭后仍可能有后台线程存活，直接退出进程保证“退出”立即生效。
                    append_error_file(
                        "INFO",
                        &format!("calling std::process::exit(0), pid={}", std::process::id()),
                    );
                    std::process::exit(0);
                }
                TimerFinished => {
                    self.handle_core_command(AppCommand::ShowWindow, ctx);
                }
                WindowShouldShow => {
                    self.requested_hide = false;
                }
                WindowShouldHide => {
                    self.requested_hide = true;
                }
                TimerUpdated => {}
            }
        }
    }

    /// 构建 egui UI 布局：模式标签 → 时间显示 → 状态标签 → 按钮行。
    fn render_ui(&mut self, ctx: &egui::Context) {
        let vs = self.controller.view_state();
        let window_locked = self.controller.config().window.locked;

        egui::CentralPanel::default().show(ctx, |ui| {
            let drag_height = 18.0;
            let drag_size = Vec2::new(ui.available_width(), drag_height);
            let (drag_rect, drag_response) = ui.allocate_exact_size(drag_size, Sense::drag());
            if !window_locked && drag_response.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            ui.painter().text(
                drag_rect.center(),
                Align2::CENTER_CENTER,
                "Catime",
                FontId::proportional(11.0),
                Color32::GRAY,
            );

            ui.vertical_centered(|ui| {
                ui.add_space(8.0);

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

                let primary_label = match vs.status {
                    TimerStatus::Running => "暂停",
                    TimerStatus::Paused => "继续",
                    _ => "开始",
                };
                let primary_cmd = match vs.status {
                    TimerStatus::Running => AppCommand::Pause,
                    TimerStatus::Paused => AppCommand::Resume,
                    _ => AppCommand::Start,
                };
                let switch_label = match vs.mode {
                    timer_core::TimerMode::Stopwatch => "切倒计时",
                    timer_core::TimerMode::Countdown => "切正计时",
                };
                let switch_mode = match vs.mode {
                    timer_core::TimerMode::Stopwatch => timer_core::TimerMode::Countdown,
                    timer_core::TimerMode::Countdown => timer_core::TimerMode::Stopwatch,
                };

                let available_width = ui.available_width().max(180.0);
                let button_height = 38.0;
                let spacing = ui.spacing().item_spacing.x;
                let wide_layout = available_width >= 360.0;
                let content_width = available_width.min(460.0);

                if wide_layout {
                    let row_width = content_width.min(420.0);
                    let button_width = ((row_width - spacing * 2.0) / 3.0).clamp(96.0, 132.0);
                    let exact_row_width = button_width * 3.0 + spacing * 2.0;
                    let left_pad = ((available_width - exact_row_width) / 2.0).max(0.0);

                    ui.horizontal(|ui| {
                        ui.add_space(left_pad);

                        if ui
                            .add_sized(
                                [button_width, button_height],
                                egui::Button::new(RichText::new(primary_label).size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self.tx.send(UiCommand::Core(primary_cmd));
                        }

                        if ui
                            .add_sized(
                                [button_width, button_height],
                                egui::Button::new(RichText::new("重置").size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self.tx.send(UiCommand::Core(AppCommand::Reset));
                        }

                        if ui
                            .add_sized(
                                [button_width, button_height],
                                egui::Button::new(RichText::new(switch_label).size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self
                                .tx
                                .send(UiCommand::Core(AppCommand::SwitchMode(switch_mode)));
                        }
                    });
                } else {
                    let top_row_width = content_width.min(280.0);
                    let top_button_width = ((top_row_width - spacing) / 2.0).clamp(92.0, 132.0);
                    let exact_top_row_width = top_button_width * 2.0 + spacing;
                    let bottom_button_width = content_width.min(180.0).clamp(120.0, 180.0);
                    let top_left_pad = ((available_width - exact_top_row_width) / 2.0).max(0.0);
                    let bottom_left_pad = ((available_width - bottom_button_width) / 2.0).max(0.0);

                    ui.horizontal(|ui| {
                        ui.add_space(top_left_pad);

                        if ui
                            .add_sized(
                                [top_button_width, button_height],
                                egui::Button::new(RichText::new(primary_label).size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self.tx.send(UiCommand::Core(primary_cmd));
                        }

                        if ui
                            .add_sized(
                                [top_button_width, button_height],
                                egui::Button::new(RichText::new("重置").size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self.tx.send(UiCommand::Core(AppCommand::Reset));
                        }
                    });

                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.add_space(bottom_left_pad);

                        if ui
                            .add_sized(
                                [bottom_button_width, button_height],
                                egui::Button::new(RichText::new(switch_label).size(18.0)),
                            )
                            .clicked()
                        {
                            let _ = self
                                .tx
                                .send(UiCommand::Core(AppCommand::SwitchMode(switch_mode)));
                        }
                    });
                }
            });

            let handle_size = 16.0;
            let handle_rect = egui::Rect::from_min_size(
                ui.max_rect().right_bottom() - Vec2::splat(handle_size),
                Vec2::splat(handle_size),
            );
            let resize_response =
                ui.interact(handle_rect, ui.id().with("resize_handle"), Sense::drag());
            if resize_response.hovered() || resize_response.dragged() {
                ui.ctx()
                    .set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            if resize_response.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(
                    egui::ResizeDirection::SouthEast,
                ));
            }

            let stroke = egui::Stroke::new(1.0, Color32::GRAY);
            let right = handle_rect.right();
            let bottom = handle_rect.bottom();
            ui.painter().line_segment(
                [
                    egui::pos2(right - 12.0, bottom),
                    egui::pos2(right, bottom - 12.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(right - 8.0, bottom),
                    egui::pos2(right, bottom - 8.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(right - 4.0, bottom),
                    egui::pos2(right, bottom - 4.0),
                ],
                stroke,
            );
        });
    }

    fn apply_visibility(&mut self, ctx: &egui::Context) {
        let target_visible = !self.requested_hide;
        if self.last_applied_visible == Some(target_visible) {
            return;
        }
        self.last_applied_visible = Some(target_visible);

        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(target_visible));
        if target_visible {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // Hidden -> visible often needs one more frame before child windows paint.
            ctx.request_repaint();
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn prepare_dialog_open(&mut self, ctx: &egui::Context) {
        self.handle_core_command(AppCommand::ShowWindow, ctx);
        self.requested_hide = false;
        self.last_applied_visible = None;
        self.pending_dialog_repaint_frames = 6;

        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn drive_pending_dialog_repaint(&mut self, ctx: &egui::Context) {
        if self.pending_dialog_repaint_frames == 0 {
            return;
        }
        self.pending_dialog_repaint_frames -= 1;
        ctx.request_repaint();
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn apply_opacity(&mut self, ctx: &egui::Context) {
        let raw_opacity = self.controller.config().opacity;
        let opacity = if raw_opacity.is_finite() {
            raw_opacity.clamp(0.05, 1.0)
        } else {
            0.85
        };
        let alpha = (opacity * 255.0).round() as u8;
        if self.last_applied_opacity_alpha == Some(alpha) {
            return;
        }
        self.last_applied_opacity_alpha = Some(alpha);

        ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(true));

        let mut style = (*ctx.style()).clone();
        let bg = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        style.visuals.window_fill = bg;
        style.visuals.panel_fill = bg;
        ctx.set_style(style);
    }

    fn render_countdown_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_countdown_dialog {
            return;
        }

        let dialog_frame = egui::Frame::window(&ctx.style()).fill(Color32::WHITE);
        let mut open = true;
        egui::Window::new("设置倒计时")
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .frame(dialog_frame)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("支持：秒数、MM:SS、HH:MM:SS");
                ui.text_edit_singleline(&mut self.countdown_input);
                if let Some(err) = &self.countdown_error {
                    ui.colored_label(Color32::RED, err);
                }
                ui.horizontal(|ui| {
                    if ui.button("确定").clicked() {
                        match parse_duration_to_secs(&self.countdown_input) {
                            Some(secs) if secs > 0 => {
                                let _ = self.tx.send(UiCommand::Core(AppCommand::SwitchMode(
                                    timer_core::TimerMode::Countdown,
                                )));
                                let _ =
                                    self.tx.send(UiCommand::Core(AppCommand::SetCountdown(secs)));
                                self.countdown_error = None;
                                self.show_countdown_dialog = false;
                            }
                            _ => {
                                self.countdown_error =
                                    Some("请输入有效时长（例如 1500 / 25:00 / 01:25:00）".into());
                            }
                        }
                    }
                    if ui.button("取消").clicked() {
                        self.show_countdown_dialog = false;
                        self.countdown_error = None;
                    }
                });
            });

        if !open {
            self.show_countdown_dialog = false;
            self.countdown_error = None;
        }
    }

    fn render_opacity_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_opacity_dialog {
            return;
        }

        let dialog_frame = egui::Frame::window(&ctx.style()).fill(Color32::WHITE);
        let mut open = true;
        egui::Window::new("设置透明度")
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .frame(dialog_frame)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("支持：0.05 ~ 1.00，或 5% ~ 100%");
                ui.text_edit_singleline(&mut self.opacity_input);
                if let Some(err) = &self.opacity_error {
                    ui.colored_label(Color32::RED, err);
                }
                ui.horizontal(|ui| {
                    if ui.button("确定").clicked() {
                        match parse_opacity_input(&self.opacity_input) {
                            Some(opacity) => {
                                self.controller.update_opacity(opacity);
                                if let Err(e) = self.controller.save_config() {
                                    log::error!("failed to save config after opacity change: {}", e);
                                }
                                self.opacity_error = None;
                                self.show_opacity_dialog = false;
                                self.apply_opacity(ctx);
                                ctx.request_repaint();
                            }
                            None => {
                                self.opacity_error =
                                    Some("请输入有效透明度，例如 0.85 / 85% / 100".into());
                            }
                        }
                    }
                    if ui.button("取消").clicked() {
                        self.show_opacity_dialog = false;
                        self.opacity_error = None;
                    }
                });
            });

        if !open {
            self.show_opacity_dialog = false;
            self.opacity_error = None;
        }
    }

    fn sync_window_bounds(&mut self, ctx: &egui::Context) {
        let viewport = ctx.input(|i| i.viewport().clone());
        let Some(outer) = viewport.outer_rect else {
            return;
        };
        let x = outer.min.x.round() as i32;
        let y = outer.min.y.round() as i32;
        let width = (outer.width().round() as i32).max(1) as u32;
        let height = (outer.height().round() as i32).max(1) as u32;
        let width = width.max(MIN_WINDOW_WIDTH);
        let height = height.max(MIN_WINDOW_HEIGHT);
        let current = (x, y, width, height);
        if self.last_saved_window == Some(current) {
            return;
        }
        self.last_saved_window = Some(current);
        self.controller.update_window_bounds(x, y, width, height, None);
        if let Err(e) = self.controller.save_config() {
            log::error!("failed to save config after window move/resize: {}", e);
        }
    }
}

fn parse_duration_to_secs(input: &str) -> Option<u64> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    if !text.contains(':') {
        return text.parse::<u64>().ok();
    }

    let parts: Vec<&str> = text.split(':').collect();
    match parts.as_slice() {
        [m, s] => {
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if s >= 60 {
                return None;
            }
            Some(m * 60 + s)
        }
        [h, m, s] => {
            let h = parse_part(h)?;
            let m = parse_part(m)?;
            let s = parse_part(s)?;
            if m >= 60 || s >= 60 {
                return None;
            }
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

fn parse_part(part: &str) -> Option<u64> {
    if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

fn parse_opacity_input(input: &str) -> Option<f32> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(percent) = text.strip_suffix('%') {
        let value = percent.trim().parse::<f32>().ok()?;
        return Some((value / 100.0).clamp(0.05, 1.0));
    }

    let value = text.parse::<f32>().ok()?;
    if value > 1.0 {
        Some((value / 100.0).clamp(0.05, 1.0))
    } else {
        Some(value.clamp(0.05, 1.0))
    }
}
