use timer_core::AppCommand;

/// UI 层内部命令：封装核心命令和仅 UI 消费的动作。
#[derive(Debug, Clone)]
pub enum UiCommand {
    Core(AppCommand),
    OpenSetCountdownDialog,
    OpenSetOpacityDialog,
}
