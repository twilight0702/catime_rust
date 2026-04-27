# Catime — Rust 轻量级 Windows 计时器

常驻系统托盘的桌面计时器，支持正计时 / 倒计时，使用 Rust 编写。

## 功能

- **正计时 / 倒计时** — 切换模式
- **开始 / 暂停 / 继续 / 重置** — 完整计时控制
- **系统托盘** — 左键切换窗口显隐，右键菜单操作（含倒计时时长设置）
- **配置持久化** — TOML 配置文件，自愈逻辑（损坏自动重置）
- **配置热更新** — 外部修改配置文件自动同步
- **置顶窗口** — 始终保持在最前
- **Ctrl+C 优雅退出** — debug 构建支持终端 Ctrl+C 退出
- **轻量** — raw Win32 API，无 UI 框架依赖

## 快速开始

```bash
cargo run -p timer-win32
```

首次运行自动在可执行文件同目录生成 `config.toml`。

## 文件架构

```
catime_rust/
├── crates/
│   ├── timer-core/        # 纯计时逻辑，零依赖
│   │   ├── timer.rs       # 计时引擎与状态机
│   │   ├── command.rs     # 命令枚举（来自 UI / 托盘）
│   │   ├── event.rs       # 事件枚举（通知外部）
│   │   └── view_state.rs  # UI 只读状态快照
│   ├── timer-storage/     # 配置持久化
│   │   ├── config.rs      # 配置结构与 serde 序列化
│   │   └── repository.rs  # 配置读写（TOML）
│   ├── timer-app/         # 应用协调层
│   │   └── controller.rs  # AppController：接收命令 → 调用引擎 → 更新状态
│   └── timer-win32/       # Win32 原生 UI（主可执行文件）
│       ├── main.rs        # 入口：窗口创建、消息循环、Ctrl+C 处理
│       ├── window.rs      # WndProc 消息处理与事件分发
│       ├── render.rs      # GDI 双缓冲渲染
│       ├── tray.rs        # 系统托盘与右键菜单
│       ├── watcher.rs     # 配置文件热更新监听
│       └── countdown_dialog.rs  # 倒计时时长设置对话框
├── config.toml            # 默认配置文件
└── assets/icon.ico        # 托盘图标
```

## 分层设计

```
UI（timer-win32）→ AppCommand
    ↓
协调层（timer-app）→ AppController
    ↓
领域层（timer-core）→ TimerEngine
    ↓
持久化（timer-storage）→ TOML 配置
```

各层通过命令/事件解耦，核心计时逻辑不依赖任何 UI 框架。

## 技术栈

| 组件 | 选型 |
|---|---|
| GUI | raw Win32 API (`windows` crate) |
| 托盘 | Shell_NotifyIcon (Win32) |
| 渲染 | GDI 双缓冲 |
| 配置 | toml + serde |
| 图标 | 程序化生成 RGBA icon |

## 后续计划

- 透明/无边框窗口
- 倒计时结束提示音
- 历史记录（SQLite）
- 多显示器 DPI 适配
