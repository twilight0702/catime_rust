//! 应用协调层：AppController 负责接收 AppCommand，
//! 调用 TimerEngine 处理，并同步 ViewState 给 UI 层。

pub mod controller;

pub use controller::AppController;
