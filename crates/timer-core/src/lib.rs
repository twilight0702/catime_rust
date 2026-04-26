//! 计时器核心领域层：纯计时逻辑，零外部依赖。
//! 不依赖任何 UI 框架、文件系统或操作系统 API，
//! 可在纯 Rust 环境下独立测试。

pub mod command;
pub mod event;
pub mod timer;
pub mod view_state;

pub use command::AppCommand;
pub use event::AppEvent;
pub use timer::{TimerEngine, TimerMode, TimerStatus};
pub use view_state::ViewState;
