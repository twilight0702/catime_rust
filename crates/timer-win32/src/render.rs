use timer_core::{TimerMode, TimerStatus, ViewState};
use windows::core::HSTRING;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, GetDC, GetDeviceCaps, GetStockObject, RoundRect, SelectObject,
    SetBkMode, SetTextColor, HBRUSH, HDC, HFONT, PAINTSTRUCT,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, LOGPIXELSY,
    OUT_DEFAULT_PRECIS, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ButtonHit { StartPause, Reset, SwitchMode, None }

pub struct RenderContext {
    pub font_time: HFONT,
    pub font_label: HFONT,
    pub font_btn: HFONT,
    /// DPI 缩放因子（相对于 96 DPI）
    pub scale: f32,
}

impl RenderContext {
    pub fn new() -> Self {
        let dpi = unsafe {
            let hdc = GetDC(None);
            GetDeviceCaps(hdc, LOGPIXELSY)
        };
        let scale = dpi as f32 / 96.0;

        let mk = |pt: i32| -> HFONT {
            unsafe {
                CreateFontW(
                    -(pt * dpi / 72), 0, 0, 0,
                    FW_NORMAL.0 as i32, 0, 0, 0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_DEFAULT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    CLEARTYPE_QUALITY.0 as u32,
                    (DEFAULT_PITCH.0 | 0x31) as u32,
                    &HSTRING::from("微软雅黑"),
                )
            }
        };

        Self {
            font_time: mk(56),
            font_label: mk(12),
            font_btn: mk(11),
            scale,
        }
    }

    /// 在 DPI 变化时重建所有字体和缩放因子
    pub fn rebuild(&mut self, new_dpi: i32) {
        let scale = new_dpi as f32 / 96.0;

        let mk = |pt: i32| -> HFONT {
            unsafe {
                CreateFontW(
                    -(pt * new_dpi / 72), 0, 0, 0,
                    FW_NORMAL.0 as i32, 0, 0, 0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_DEFAULT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    CLEARTYPE_QUALITY.0 as u32,
                    (DEFAULT_PITCH.0 | 0x31) as u32,
                    &HSTRING::from("微软雅黑"),
                )
            }
        };

        // 先删旧字体再建新的
        unsafe {
            let _ = DeleteObject(self.font_time);
            let _ = DeleteObject(self.font_label);
            let _ = DeleteObject(self.font_btn);
        }
        self.font_time = mk(56);
        self.font_label = mk(12);
        self.font_btn = mk(11);
        self.scale = scale;
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font_time);
            let _ = DeleteObject(self.font_label);
            let _ = DeleteObject(self.font_btn);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    HSTRING::from(text).as_wide().to_vec()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// DPI 缩放辅助：按 scale 倍数计算坐标
fn s(x: i32, scale: f32) -> i32 { (x as f32 * scale) as i32 }

pub fn paint(hwnd: HWND, render: &RenderContext, vs: &ViewState) {
    let sc = render.scale;
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        let mut rect: RECT = Default::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let w = rect.right;
        let bg = HBRUSH(GetStockObject(WHITE_BRUSH).0);

        FillRect(hdc, &rect, bg);
        SetBkMode(hdc, TRANSPARENT);

        let df = DT_CENTER | DT_VCENTER | DT_SINGLELINE;

        // 模式标签
        let mode_text = match vs.mode { TimerMode::Stopwatch => "正计时", TimerMode::Countdown => "倒计时" };
        SelectObject(hdc, render.font_label);
        SetTextColor(hdc, rgb(0x80, 0x80, 0x80));
        let mut r = RECT { left: 0, top: s(16, sc), right: w, bottom: s(38, sc) };
        let mut ws = wide(mode_text);
        DrawTextW(hdc, &mut ws, &mut r, df);

        // 时间
        SelectObject(hdc, render.font_time);
        let tc = match vs.status {
            TimerStatus::Finished => rgb(0xE0, 0x20, 0x20),
            TimerStatus::Paused => rgb(0x40, 0x40, 0x40),
            _ => rgb(0x00, 0x00, 0x00),
        };
        SetTextColor(hdc, tc);
        let mut r = RECT { left: 0, top: s(38, sc), right: w, bottom: s(108, sc) };
        let mut ws = wide(&vs.display_time);
        DrawTextW(hdc, &mut ws, &mut r, df);

        // 状态
        SelectObject(hdc, render.font_label);
        SetTextColor(hdc, rgb(0x80, 0x80, 0x80));
        let st = match vs.status {
            TimerStatus::Idle => "就绪", TimerStatus::Running => "运行中",
            TimerStatus::Paused => "暂停", TimerStatus::Finished => "已结束",
        };
        let mut r = RECT { left: 0, top: s(110, sc), right: w, bottom: s(132, sc) };
        let mut ws = wide(st);
        DrawTextW(hdc, &mut ws, &mut r, df);

        // 按钮
        let b1 = match vs.status { TimerStatus::Running => "暂停", TimerStatus::Paused => "继续", _ => "开始" };
        let btn_y = s(140, sc);
        let btn_h = s(32, sc);
        draw_btn(hdc, render.font_btn, s(BTN1_X, sc), btn_y, s(BTN1_W, sc), btn_h, b1, s(4, sc), df);
        draw_btn(hdc, render.font_btn, s(BTN2_X, sc), btn_y, s(BTN2_W, sc), btn_h, "重置", s(4, sc), df);
        let sw = match vs.mode { TimerMode::Stopwatch => "切倒计时", TimerMode::Countdown => "切正计时" };
        draw_btn(hdc, render.font_btn, s(BTN3_X, sc), btn_y, s(BTN3_W, sc), btn_h, sw, s(4, sc), df);

        let _ = EndPaint(hwnd, &ps);
    }
}

const BTN1_X: i32 = 20;  const BTN1_W: i32 = 72;
const BTN2_X: i32 = 104; const BTN2_W: i32 = 60;
const BTN3_X: i32 = 176; const BTN3_W: i32 = 104;

fn draw_btn(
    hdc: HDC, font: HFONT,
    x: i32, y: i32, w: i32, h: i32,
    text: &str, radius: i32, fmt: DRAW_TEXT_FORMAT,
) {
    unsafe {
        SelectObject(hdc, font);
        let bg = CreateSolidBrush(rgb(0xE8, 0xE8, 0xE8));
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        FillRect(hdc, &r, bg);
        RoundRect(hdc, x, y, x + w, y + h, radius, radius);
        SetTextColor(hdc, rgb(0x00, 0x00, 0x00));
        let mut ws = wide(text);
        let mut r2 = r;
        DrawTextW(hdc, &mut ws, &mut r2, fmt);
        let _ = DeleteObject(bg);
    }
}

use windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT;

pub fn hit_test_button(x: f32, y: f32, scale: f32) -> ButtonHit {
    let by = s(140, scale) as f32;
    let bh = s(32, scale) as f32;
    if y < by || y > by + bh { return ButtonHit::None; }
    let x1 = s(BTN1_X, scale) as f32; let w1 = s(BTN1_W, scale) as f32;
    let x2 = s(BTN2_X, scale) as f32; let w2 = s(BTN2_W, scale) as f32;
    let x3 = s(BTN3_X, scale) as f32; let w3 = s(BTN3_W, scale) as f32;
    if x >= x1 && x <= x1 + w1 { ButtonHit::StartPause }
    else if x >= x2 && x <= x2 + w2 { ButtonHit::Reset }
    else if x >= x3 && x <= x3 + w3 { ButtonHit::SwitchMode }
    else { ButtonHit::None }
}
