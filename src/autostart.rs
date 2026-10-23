//! Windows 起動時の自動起動(スタートアップフォルダへのショートカット)。
//!
//! 管理者権限は要らず、現在のユーザーのスタートアップフォルダに、このツール専用のショートカット
//! (`SnapMark.lnk`)を1つ置く/消すだけ。他のアプリのショートカットには触れない。
//! 設定アプリの「スタートアップ アプリ」にも表示され、そこからも切れる。

use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile,
    STGM_READ,
};
use windows::Win32::UI::Shell::{FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{HSTRING, Interface};

const LINK_NAME: &str = "SnapMark.lnk";
/// 旧名称(ScreenshotTool)で作ったショートカット。見つかれば消して、二重起動の元を断つ
const LEGACY_LINK_NAME: &str = "ScreenshotTool.lnk";

/// 現在のユーザーのスタートアップフォルダ。
fn startup_dir() -> Result<PathBuf, String> {
    // SAFETY: 返された文字列は使い終わったら CoTaskMemFree で解放する決まり
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("スタートアップフォルダを取得できません: {e}"))?;
        let s = p.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(p.as_ptr() as *const _));
        s.map(PathBuf::from)
    }
}

fn link_path() -> Result<PathBuf, String> {
    Ok(startup_dir()?.join(LINK_NAME))
}

fn init_com() {
    // SAFETY: すでに別のモードで初期化済みなら失敗するが、そのまま使える
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// ショートカットの指す先。ショートカットが無い/読めないときは `None`。
fn current_target(link: &Path) -> Option<PathBuf> {
    if !link.exists() {
        return None;
    }
    // SAFETY: シェルリンクの COM 呼び出し。失敗は Err で返るので None にする
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = shell_link.cast().ok()?;
        file.Load(&HSTRING::from(link.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        shell_link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
    }
}

fn create_link(link: &Path, exe: &Path) -> Result<(), String> {
    // SAFETY: シェルリンクの COM 呼び出し。失敗は Err で返す
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ショートカットを作れません: {e}"))?;
        shell_link.SetPath(&HSTRING::from(exe.as_os_str())).map_err(|e| e.to_string())?;
        if let Some(dir) = exe.parent() {
            shell_link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str())).map_err(|e| e.to_string())?;
        }
        shell_link.SetDescription(&HSTRING::from("SnapMark")).map_err(|e| e.to_string())?;
        let file: IPersistFile = shell_link.cast().map_err(|e| e.to_string())?;
        file.Save(&HSTRING::from(link.as_os_str()), true).map_err(|e| format!("ショートカットを保存できません: {e}"))
    }
}

/// 自動起動を設定に合わせる。`enabled` が真なら、いま実行中のこの exe へのショートカットを置く
/// (exe を移動した後でも、起動のたびに正しい場所へ更新される)。偽なら、ショートカットを消す。
pub fn sync(enabled: bool) -> Result<(), String> {
    init_com();
    let link = link_path()?;
    if let Some(old) = link.parent().map(|d| d.join(LEGACY_LINK_NAME)) {
        let _ = std::fs::remove_file(old);
    }
    if enabled {
        let exe = std::env::current_exe().map_err(|e| format!("実行ファイルの場所を取得できません: {e}"))?;
        let up_to_date = current_target(&link).is_some_and(|t| t == exe);
        if up_to_date {
            return Ok(());
        }
        create_link(&link, &exe)
    } else if link.exists() {
        std::fs::remove_file(&link).map_err(|e| format!("自動起動のショートカットを消せませんでした: {e}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ショートカットの作成 → 指す先の確認 → 削除を、実際のスタートアップフォルダで行う。
    /// ユーザーの環境を書き換えるので、通常の `cargo test` では動かさない
    /// (`cargo test -- --ignored` で実行する)。すでに登録済みの環境では何もしない。
    #[test]
    #[ignore]
    fn create_and_remove_shortcut() {
        let link = link_path().unwrap();
        if link.exists() {
            return;
        }
        sync(true).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(current_target(&link).unwrap(), exe);
        sync(false).unwrap();
        assert!(!link.exists());
    }
}
