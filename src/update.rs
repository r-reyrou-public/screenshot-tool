//! アップデートの確認。GitHub の最新リリースのタグを、OS 標準の WinHTTP で取得して比べるだけで、
//! ダウンロードや自動更新はしない(新しい版があれば、リリースページを開いて案内する)。
//! 通信は数日に1回、バックグラウンドのスレッドで行い、失敗しても何も起きない。

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};

const REPO: &str = "glasssoundforest-lab/screenshot-tool";
/// 確認の間隔
pub const INTERVAL: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const MAX_BODY: usize = 512 * 1024;

/// 設定画面などに表示する署名(作者名)
pub const AUTHOR: &str = "Reyrou";

/// ソースコードのリポジトリ(画面に表示するURL)
pub fn repo_url() -> String {
    format!("https://github.com/{REPO}")
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 案内するリリースページ。レスポンスの内容は使わず、固定のURLだけを開く。
pub fn release_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// 確認の時期が来ているか。
pub fn is_due(last_check: u64) -> bool {
    now_secs().saturating_sub(last_check) >= INTERVAL.as_secs()
}

/// "v1.2.3" / "1.2.3" を数字の組にする。形式が違えば `None`。
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some(v)
}

/// `latest` が、いま動いている版より新しければ、表示用の版番号(先頭の v 抜き)を返す。
pub fn newer_than_current(latest: &str) -> Option<String> {
    let l = parse_version(latest)?;
    (l > parse_version(current_version())?).then(|| latest.trim().trim_start_matches('v').to_string())
}

/// 最新リリースのタグ名(例 "v0.7.0")。
fn extract_tag(body: &str) -> Option<&str> {
    let rest = body.split_once("\"tag_name\"")?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let tag = &rest[..rest.find('"')?];
    parse_version(tag).map(|_| tag)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// GitHub API から最新リリースの JSON を取る。
fn fetch_latest_release() -> Option<String> {
    let agent = wide(&format!("SnapMark/{}", current_version()));
    let host = wide("api.github.com");
    let path = wide(&format!("/repos/{REPO}/releases/latest"));
    let verb = wide("GET");
    let headers = wide("Accept: application/vnd.github+json\r\n");
    // SAFETY: すべて NUL 終端済みの UTF-16。開いたハンドルは `Handle` が必ず閉じる
    unsafe {
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        ));
        if session.0.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 10_000);
        let conn = Handle(WinHttpConnect(session.0, host.as_ptr(), 443, 0));
        if conn.0.is_null() {
            return None;
        }
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            verb.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return None;
        }
        if WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0
        {
            return None;
        }
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut _,
            &mut len,
            std::ptr::null_mut(),
        ) == 0
            || status != 200
        {
            return None;
        }
        let mut body = Vec::new();
        let mut buf = [0u8; 8192];
        while body.len() < MAX_BODY {
            let mut read = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read) == 0 || read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
        }
        String::from_utf8(body).ok()
    }
}

struct Handle(*mut std::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: WinHttp* が返した、まだ閉じていないハンドル
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// 最新リリースの版番号(先頭の v 抜き)。通信や解析に失敗したら `None`。
pub fn check_latest() -> Option<String> {
    let body = fetch_latest_release()?;
    extract_tag(&body).map(|t| t.trim_start_matches('v').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert!(newer_than_current("v999.0.0").is_some());
        assert!(newer_than_current(current_version()).is_none());
        assert!(newer_than_current("v0.0.1").is_none());
    }

    #[test]
    fn tag_extraction() {
        assert_eq!(extract_tag(r#"{"url":"x","tag_name": "v0.7.0","name":"y"}"#), Some("v0.7.0"));
        assert_eq!(extract_tag(r#"{"tag_name":"<script>"}"#), None);
        assert_eq!(extract_tag("{}"), None);
    }

    #[test]
    fn due() {
        assert!(is_due(0));
        assert!(!is_due(now_secs()));
    }

    /// 実際に GitHub へ接続する。ネットワークが要るので `cargo test -- --ignored` で実行する。
    #[test]
    #[ignore]
    fn live_check() {
        let v = check_latest().expect("latest release");
        assert!(parse_version(&v).is_some());
    }
}
