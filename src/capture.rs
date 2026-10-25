use anyhow::{Result, anyhow};
use image::RgbaImage;

pub struct Shot {
    pub image: RgbaImage,
    /// モニタ左上の物理ピクセル座標
    pub origin: (i32, i32),
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: p は有効な書き込み先
    if unsafe { GetCursorPos(&mut p) } != 0 { Some((p.x, p.y)) } else { None }
}

/// カーソルのあるモニタの矩形 (x, y, w, h) を物理ピクセルで返す。
pub fn cursor_monitor_rect() -> Option<(i32, i32, u32, u32)> {
    let (cx, cy) = cursor_pos()?;
    let m = xcap::Monitor::from_point(cx, cy).ok()?;
    Some((m.x().ok()?, m.y().ok()?, m.width().ok()?, m.height().ok()?))
}

/// マウスカーソルのあるモニタ全体を撮影する。
pub fn capture_cursor_monitor() -> Result<Shot> {
    let (cx, cy) = cursor_pos().unwrap_or((0, 0));
    capture_monitor_at(cx, cy)
}

/// 物理座標 (x, y) を含むモニタ全体を撮影する。
pub fn capture_monitor_at(cx: i32, cy: i32) -> Result<Shot> {
    let monitor = xcap::Monitor::from_point(cx, cy)
        .or_else(|_| xcap::Monitor::all()?.into_iter().next().ok_or_else(|| xcap::XCapError::new("no monitor")))
        .map_err(|e| anyhow!("モニタを取得できません: {e}"))?;
    let origin = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
    // HDR 表示のモニターは、明るさの持ち上げを戻した元の色で撮る(撮れなければ従来の方法)
    if let (Ok(mw), Ok(mh)) = (monitor.width(), monitor.height())
        && let Some(image) = crate::hdr::capture_monitor(origin.0 + mw as i32 / 2, origin.1 + mh as i32 / 2)
        && image.dimensions() == (mw, mh)
    {
        return Ok(Shot { image, origin });
    }
    let img = monitor.capture_image().map_err(|e| anyhow!("撮影に失敗: {e}"))?;
    let (w, h) = (img.width(), img.height());
    Ok(Shot { image: RgbaImage::from_raw(w, h, img.into_raw()).ok_or_else(|| anyhow!("画像変換に失敗"))?, origin })
}
