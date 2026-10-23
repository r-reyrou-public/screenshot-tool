//! トップレベルウィンドウの列挙と前面化(ウィンドウ自動検出用)。

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

#[derive(Clone, Debug)]
pub struct WinInfo {
    pub hwnd: isize,
    /// 物理ピクセルでの (left, top, right, bottom)。影を除いた見た目の枠
    pub rect: (i32, i32, i32, i32),
    pub title: String,
}

impl WinInfo {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.0 && x < self.rect.2 && y >= self.rect.1 && y < self.rect.3
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 見た目の枠(DWM の拡張フレーム境界)。取れなければ GetWindowRect。
unsafe fn frame_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: r は RECT と同じ大きさの有効な書き込み先
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if hr == 0 {
        return Some(r);
    }
    // SAFETY: 同上
    (unsafe { GetWindowRect(hwnd, &mut r) } != 0).then_some(r)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: lparam は list_windows が渡した Vec への有効な排他ポインタ
    let list = unsafe { &mut *(lparam as *mut Vec<WinInfo>) };
    // SAFETY: 以降の Win32 呼び出しは、列挙中の有効な hwnd に対するもの
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId() {
            return 1;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let tool = ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0;
        if tool || ex & (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT) != 0 {
            return 1;
        }
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut c_void, 4);
        if cloaked != 0 {
            return 1;
        }
        let mut class = [0u16; 64];
        GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        if matches!(wide_to_string(&class).as_str(), "Progman" | "WorkerW") {
            return 1;
        }
        let mut title = [0u16; 256];
        GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        let title = wide_to_string(&title);
        if title.is_empty() {
            return 1;
        }
        let Some(r) = frame_rect(hwnd) else { return 1 };
        if r.right - r.left < 40 || r.bottom - r.top < 30 {
            return 1;
        }
        list.push(WinInfo { hwnd: hwnd as isize, rect: (r.left, r.top, r.right, r.bottom), title });
    }
    1
}

/// 画面上のウィンドウを、手前にあるものから順に返す。
pub fn list_windows() -> Vec<WinInfo> {
    let mut list: Vec<WinInfo> = Vec::new();
    // SAFETY: コールバックは呼び出し中のみ list を使う
    unsafe { EnumWindows(Some(enum_proc), &mut list as *mut Vec<WinInfo> as LPARAM) };
    list
}

/// ウィンドウを前面に出してフォーカスを当てる(最小化されていれば戻す)。
pub fn focus_window(hwnd: isize) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
    let hwnd = hwnd as HWND;
    // SAFETY: hwnd は列挙時に得たウィンドウ。無効になっていても各 API は失敗を返すだけ
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        // 前面化の制限を回避するため、Alt の押下を挟む
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
}
