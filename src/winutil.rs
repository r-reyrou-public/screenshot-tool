use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SW_HIDE, SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};

/// ウィンドウをタスクバー / Alt+Tab に表示するか切り替える。
/// 常駐中の待機ウィンドウは非表示、編集・設定ウィンドウは表示にする。
/// winit がウィンドウ装飾の更新時に拡張スタイルを上書きするため、毎フレーム呼んでよい
/// (差があるときだけ書き換える)。
pub fn set_listed(frame: &eframe::Frame, listed: bool) {
    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    // SAFETY: hwnd は eframe が所有する有効なウィンドウ。UI スレッドから呼ぶ。
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let (tool, app) = (WS_EX_TOOLWINDOW as isize, WS_EX_APPWINDOW as isize);
        let new = if listed { (ex & !tool) | app } else { (ex | tool) & !app };
        if new != ex {
            // 拡張スタイルの変更を一覧に反映させるため、一度隠してから戻す
            ShowWindow(hwnd, SW_HIDE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// Windows のインストール先(`%SystemRoot%`)。環境変数が無ければ `None`。
fn windows_dir() -> Option<PathBuf> {
    std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")).map(PathBuf::from)
}

/// システムのフォントフォルダ。
pub fn fonts_dir() -> Option<PathBuf> {
    windows_dir().map(|d| d.join("Fonts"))
}

/// エクスプローラーでフォルダを開く。
/// 名前だけで起動すると、実行ファイルのあるフォルダに置かれた偽物が先に見つかり得るため、
/// OS のフォルダにある `explorer.exe` をフルパスで起動する。
pub fn open_in_explorer(dir: &std::path::Path) {
    let Some(exe) = windows_dir().map(|d| d.join("explorer.exe")) else { return };
    if dir.is_dir() {
        let _ = std::process::Command::new(exe).arg(dir).spawn();
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// コンソールの無いリリース版でもエラーが分かるよう、メッセージボックスで知らせる。
pub fn error_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let (text, title) = (wide(text), wide("スクショツール"));
    // SAFETY: どちらも NUL 終端済みの UTF-16。親ウィンドウは無し
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
}

/// 二重起動を防ぐ名前付きミューテックスを作る。すでに別のプロセスが持っていれば `false`。
/// 返したハンドルは閉じない(プロセスの終了まで保持して、ミューテックスを生かし続ける)。
pub fn acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide("Local\\ScreenshotTool-single-instance");
    // SAFETY: name は NUL 終端済み。直後に GetLastError を読む
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}
