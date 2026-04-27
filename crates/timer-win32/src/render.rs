//! GDI 渲染模块：所有 UI 绘制逻辑（主窗口 + 倒计时对话框）。
//! 使用双缓冲避免闪烁，根据 DPI 缩放字体和布局。

use std::ptr::null_mut;

use timer_core::{TimerMode, TimerStatus, ViewState};
use windows::core::HSTRING;
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetDeviceCaps, GetStockObject,
    ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DRAW_TEXT_FORMAT, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HBRUSH, HDC, HFONT, LOGPIXELSY, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// 主窗口按钮点击结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ButtonHit {
    StartPause,
    Reset,
    SwitchMode,
    None,
}
/// 渲染上下文：持有所有 GDI 字体句柄和 DPI 缩放因子。
pub struct RenderContext {
    /// 时间显示大字（56pt）
    pub font_time: HFONT,
    /// 对话框标题字体（15pt）
    pub font_title: HFONT,
    /// 标签/副文本字体（12pt）
    pub font_label: HFONT,
    /// 按钮字体（11pt）
    pub font_btn: HFONT,
    /// DPI 缩放因子（当前 DPI / 96）
    pub scale: f32,
}

/// 倒计时对话框布局（DPI 缩放后的各元素位置）
pub struct CountdownDialogLayout {
    pub title: RECT,
    pub subtitle: RECT,
    pub input: RECT,
    pub hint: RECT,
    pub error: RECT,
    pub btn_confirm: RECT,
    pub btn_cancel: RECT,
}

// 主窗口按钮布局常量（基于 96 DPI 的设计尺寸）
const MAIN_BTN1_X: i32 = 20;
const MAIN_BTN1_W: i32 = 72;
const MAIN_BTN2_X: i32 = 104;
const MAIN_BTN2_W: i32 = 60;
const MAIN_BTN3_X: i32 = 176;
const MAIN_BTN3_W: i32 = 104;

/// 对话框基础尺寸（基于 96 DPI）
pub const DIALOG_BASE_W: i32 = 520;
pub const DIALOG_BASE_H: i32 = 260;

impl RenderContext {
    /// 创建渲染上下文，根据当前屏幕 DPI 初始化字体和缩放。
    pub fn new() -> Self {
        let dpi = screen_dpi();
        let scale = dpi as f32 / 96.0;

        Self {
            font_time: create_font_for_dpi(dpi, 56),
            font_title: create_font_for_dpi(dpi, 15),
            font_label: create_font_for_dpi(dpi, 12),
            font_btn: create_font_for_dpi(dpi, 11),
            scale,
        }
    }

    /// 在 DPI 变化时重建所有字体和缩放因子（响应 WM_DPICHANGED）。
    pub fn rebuild(&mut self, new_dpi: i32) {
        let scale = new_dpi as f32 / 96.0;

        // 先销毁旧字体
        unsafe {
            let _ = DeleteObject(self.font_time);
            let _ = DeleteObject(self.font_title);
            let _ = DeleteObject(self.font_label);
            let _ = DeleteObject(self.font_btn);
        }
        self.font_time = create_font_for_dpi(new_dpi, 56);
        self.font_title = create_font_for_dpi(new_dpi, 15);
        self.font_label = create_font_for_dpi(new_dpi, 12);
        self.font_btn = create_font_for_dpi(new_dpi, 11);
        self.scale = scale;
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font_time);
            let _ = DeleteObject(self.font_title);
            let _ = DeleteObject(self.font_label);
            let _ = DeleteObject(self.font_btn);
        }
    }
}

/// 将 Rust 字符串转为 UTF-16 宽字符数组。
fn wide(text: &str) -> Vec<u16> {
    HSTRING::from(text).as_wide().to_vec()
}

/// 构造 COLORREF（BGR 格式）。
fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// 获取当前屏幕 DPI（垂直方向）。
fn screen_dpi() -> i32 {
    unsafe {
        let hdc = GetDC(None);
        let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
        let _ = ReleaseDC(None, hdc);
        dpi
    }
}

/// 根据 DPI 创建指定字号的字体（使用微软雅黑）。
fn create_font_for_dpi(dpi: i32, pt: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -(pt * dpi / 72), // 负值 = 使用字符高度（而非 cell 高度）
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | 0x31) as u32, // FF_DONTCARE
            &HSTRING::from("微软雅黑"),
        )
    }
}

/// DPI 缩放辅助函数：将 96 DPI 基准坐标转换为实际像素。
fn s(x: i32, scale: f32) -> i32 {
    (x as f32 * scale) as i32
}

/// 计算对话框窗口的 DPI 缩放后尺寸。
pub fn dialog_window_size(scale: f32) -> (i32, i32) {
    (s(DIALOG_BASE_W, scale), s(DIALOG_BASE_H, scale))
}

/// 根据缩放和窗口宽度计算对话框中各元素的布局位置。
pub fn dialog_layout(scale: f32, client_w: i32) -> CountdownDialogLayout {
    let margin = s(26, scale);
    let input_top = s(84, scale);
    let input_h = s(36, scale);

    let btn_w = s(132, scale);
    let btn_h = s(38, scale);
    let btn_gap = s(18, scale);
    let all_w = btn_w * 2 + btn_gap;
    let btn_left = (client_w - all_w) / 2; // 按钮水平居中
    let btn_top = s(176, scale);

    CountdownDialogLayout {
        title: RECT {
            left: margin,
            top: s(20, scale),
            right: client_w - margin,
            bottom: s(48, scale),
        },
        subtitle: RECT {
            left: margin,
            top: s(48, scale),
            right: client_w - margin,
            bottom: s(74, scale),
        },
        input: RECT {
            left: margin,
            top: input_top,
            right: client_w - margin,
            bottom: input_top + input_h,
        },
        hint: RECT {
            left: margin,
            top: s(124, scale),
            right: client_w - margin,
            bottom: s(146, scale),
        },
        error: RECT {
            left: margin,
            top: s(148, scale),
            right: client_w - margin,
            bottom: s(170, scale),
        },
        btn_confirm: RECT {
            left: btn_left,
            top: btn_top,
            right: btn_left + btn_w,
            bottom: btn_top + btn_h,
        },
        btn_cancel: RECT {
            left: btn_left + btn_w + btn_gap,
            top: btn_top,
            right: btn_left + btn_w * 2 + btn_gap,
            bottom: btn_top + btn_h,
        },
    }
}

/// 绘制主窗口（WM_PAINT 处理）。
/// 使用双缓冲：先在内存 DC 中绘制完整场景，再一次性 Blt 到屏幕，避免闪烁。
pub fn paint(hwnd: HWND, render: &RenderContext, vs: &ViewState) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        let mut rect: RECT = Default::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        // 创建内存 DC 用于双缓冲
        let mem_dc = CreateCompatibleDC(hdc);
        if mem_dc.0 == null_mut() {
            // 双缓冲失败 → 降级为直接绘制
            draw_main_scene(hdc, render, vs, w, h);
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
        if mem_bmp.0 == null_mut() {
            let _ = DeleteDC(mem_dc);
            draw_main_scene(hdc, render, vs, w, h);
            let _ = EndPaint(hwnd, &ps);
            return;
        }

        // 渲染到内存 DC → 复制到屏幕 DC
        let old_bmp = SelectObject(mem_dc, mem_bmp);
        draw_main_scene(mem_dc, render, vs, w, h);
        let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

        // 清理资源
        let _ = SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// 主窗口按钮点击测试：根据坐标判断点击了哪个按钮。
pub fn hit_test_button(x: f32, y: f32, scale: f32) -> ButtonHit {
    let by = s(140, scale) as f32;
    let bh = s(32, scale) as f32;
    if y < by || y > by + bh {
        return ButtonHit::None;
    }
    let x1 = s(MAIN_BTN1_X, scale) as f32;
    let w1 = s(MAIN_BTN1_W, scale) as f32;
    let x2 = s(MAIN_BTN2_X, scale) as f32;
    let w2 = s(MAIN_BTN2_W, scale) as f32;
    let x3 = s(MAIN_BTN3_X, scale) as f32;
    let w3 = s(MAIN_BTN3_W, scale) as f32;
    if x >= x1 && x <= x1 + w1 {
        ButtonHit::StartPause
    } else if x >= x2 && x <= x2 + w2 {
        ButtonHit::Reset
    } else if x >= x3 && x <= x3 + w3 {
        ButtonHit::SwitchMode
    } else {
        ButtonHit::None
    }
}

/// 绘制主窗口的完整场景：模式标签 → 时间 → 状态 → 三个按钮。
fn draw_main_scene(hdc: HDC, render: &RenderContext, vs: &ViewState, w: i32, h: i32) {
    let sc = render.scale;
    unsafe {
        // 白色背景 + 透明文本背景
        fill_window_bg(hdc, w, h);

        let df_center = DT_CENTER | DT_VCENTER | DT_SINGLELINE;

        // 计时模式标签（灰色小字）
        let mode_text = match vs.mode {
            TimerMode::Stopwatch => "正计时",
            TimerMode::Countdown => "倒计时",
        };
        SelectObject(hdc, render.font_label);
        SetTextColor(hdc, rgb(0x80, 0x80, 0x80));
        let mut r = RECT {
            left: 0,
            top: s(16, sc),
            right: w,
            bottom: s(38, sc),
        };
        let mut ws = wide(mode_text);
        DrawTextW(hdc, &mut ws, &mut r, df_center);

        // 时间显示（大字体，根据状态改变颜色）
        SelectObject(hdc, render.font_time);
        let tc = match vs.status {
            TimerStatus::Finished => rgb(0xE0, 0x20, 0x20), // 红色：已结束
            TimerStatus::Paused => rgb(0x40, 0x40, 0x40),   // 深灰：暂停
            _ => rgb(0x00, 0x00, 0x00),                     // 黑色：运行/就绪
        };
        SetTextColor(hdc, tc);
        let mut r = RECT {
            left: 0,
            top: s(38, sc),
            right: w,
            bottom: s(108, sc),
        };
        let mut ws = wide(&vs.display_time);
        DrawTextW(hdc, &mut ws, &mut r, df_center);

        // 状态标签（灰色小字）
        SelectObject(hdc, render.font_label);
        SetTextColor(hdc, rgb(0x80, 0x80, 0x80));
        let st = match vs.status {
            TimerStatus::Idle => "就绪",
            TimerStatus::Running => "运行中",
            TimerStatus::Paused => "暂停",
            TimerStatus::Finished => "已结束",
        };
        let mut r = RECT {
            left: 0,
            top: s(110, sc),
            right: w,
            bottom: s(132, sc),
        };
        let mut ws = wide(st);
        DrawTextW(hdc, &mut ws, &mut r, df_center);

        // 三个按钮：开始/暂停/继续 | 重置 | 切正计时/切倒计时
        let b1 = match vs.status {
            TimerStatus::Running => "暂停",
            TimerStatus::Paused => "继续",
            _ => "开始",
        };
        let btn_y = s(140, sc);
        let btn_h = s(32, sc);
        draw_button(
            hdc,
            render.font_btn,
            RECT {
                left: s(MAIN_BTN1_X, sc),
                top: btn_y,
                right: s(MAIN_BTN1_X + MAIN_BTN1_W, sc),
                bottom: btn_y + btn_h,
            },
            b1,
            s(4, sc),
            df_center,
        );
        draw_button(
            hdc,
            render.font_btn,
            RECT {
                left: s(MAIN_BTN2_X, sc),
                top: btn_y,
                right: s(MAIN_BTN2_X + MAIN_BTN2_W, sc),
                bottom: btn_y + btn_h,
            },
            "重置",
            s(4, sc),
            df_center,
        );
        let sw = match vs.mode {
            TimerMode::Stopwatch => "切倒计时",
            TimerMode::Countdown => "切正计时",
        };
        draw_button(
            hdc,
            render.font_btn,
            RECT {
                left: s(MAIN_BTN3_X, sc),
                top: btn_y,
                right: s(MAIN_BTN3_X + MAIN_BTN3_W, sc),
                bottom: btn_y + btn_h,
            },
            sw,
            s(4, sc),
            df_center,
        );
    }
}

/// 填充窗口白色背景并设置文本背景为透明。
fn fill_window_bg(hdc: HDC, w: i32, h: i32) {
    unsafe {
        let bg = HBRUSH(GetStockObject(WHITE_BRUSH).0);
        let rect = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(hdc, &rect, bg);
        SetBkMode(hdc, TRANSPARENT);
    }
}

/// 绘制一个 GDI 按钮：浅灰填充 + 圆角矩形边框 + 居中文本。
pub fn draw_button(
    hdc: HDC,
    font: HFONT,
    rect: RECT,
    text: &str,
    radius: i32,
    fmt: DRAW_TEXT_FORMAT,
) {
    unsafe {
        SelectObject(hdc, font);
        let bg = CreateSolidBrush(rgb(0xE8, 0xE8, 0xE8));
        FillRect(hdc, &rect, bg);
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        SetTextColor(hdc, rgb(0x00, 0x00, 0x00));
        let mut ws = wide(text);
        let mut text_rect = rect;
        DrawTextW(hdc, &mut ws, &mut text_rect, fmt);
        let _ = DeleteObject(bg);
    }
}
