//! HDR 表示のモニターを、元の色で撮る。
//!
//! Windows の HDR をオンにすると、画面の「白」は SDR の白レベル(既定 80 ニト)より明るく表示される。
//! アプリによっては(Discord など)、GDI で画面を取ると、この明るさの持ち上げが含まれた値が返り、
//! 撮った画像を普通のアプリで見ると、さらに持ち上げられて白っぽくなる。
//! そこで HDR のときは、Windows のキャプチャ API で浮動小数点(scRGB)のまま受け取り、
//! SDR の白レベルの倍率で割ってから sRGB に戻す。

use image::RgbaImage;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromPoint};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::Interface;

/// `hmon` が HDR(高度な色)で表示中なら、SDR の白レベルの倍率(80 ニト = 1.0)を返す。
fn sdr_white_scale(hmon: HMONITOR) -> Option<f32> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: info は有効な書き込み先で、cbSize を設定済み
    if !unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut _) }.as_bool() {
        return None;
    }
    let (mut np, mut nm) = (0u32, 0u32);
    // SAFETY: 出力先はどちらも有効な u32
    unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) }.ok().ok()?;
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    // SAFETY: 配列は GetDisplayConfigBufferSizes が返した個数で確保している
    unsafe { QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) }
        .ok()
        .ok()?;
    for p in paths.iter().take(np as usize) {
        let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: header の種類と大きさは、渡す構造体に合わせてある
        if unsafe { DisplayConfigGetDeviceInfo(&mut src.header) } != 0 || src.viewGdiDeviceName != info.szDevice {
            continue;
        }
        let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: 同上
        if unsafe { DisplayConfigGetDeviceInfo(&mut color.header) } != 0 {
            return None;
        }
        // SAFETY: 共用体の読み取り(どの見方でも u32 として有効)
        let value = unsafe { color.Anonymous.value };
        // bit0: 対応 / bit1: 有効。有効でなければ SDR 表示なので、従来どおり
        if value & 0b10 == 0 {
            return None;
        }
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: 同上
        if unsafe { DisplayConfigGetDeviceInfo(&mut white.header) } != 0 {
            return None;
        }
        // 1000 が 80 ニト(倍率 1.0)
        let scale = white.SDRWhiteLevel as f32 / 1000.0;
        return (scale.is_finite() && scale >= 1.0).then_some(scale);
    }
    None
}

/// 半精度浮動小数(ビット列)を f32 にする。
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let man = (h & 0x3ff) as f32;
    let v = match exp {
        0 => man / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        _ => (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    };
    sign * v
}

/// 線形の値(0..1)を sRGB の 8 ビットにする。
fn srgb8(linear: f32) -> u8 {
    let l = if linear.is_nan() { 0.0 } else { linear.clamp(0.0, 1.0) };
    let v = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0 + 0.5) as u8
}

/// 半精度の値ごとの結果(倍率で割って sRGB にしたもの)の表。倍率が同じ間は使い回す。
fn table_for(scale: f32) -> std::sync::Arc<Vec<u8>> {
    use std::sync::{Arc, Mutex};
    static CACHE: Mutex<Option<(u32, Arc<Vec<u8>>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((bits, t)) = cache.as_ref()
        && *bits == scale.to_bits()
    {
        return t.clone();
    }
    let t = Arc::new((0..=u16::MAX).map(|h| srgb8(f16_to_f32(h) / scale)).collect::<Vec<u8>>());
    *cache = Some((scale.to_bits(), t.clone()));
    t
}

/// (cx, cy) のあるモニターが HDR 表示中なら、そのモニター全体を元の色で撮る。
/// HDR でない・撮れない場合は `None`(呼び出し側が従来の方法で撮る)。
pub fn capture_monitor(cx: i32, cy: i32) -> Option<RgbaImage> {
    // SAFETY: 値渡しの座標のみ
    let hmon = unsafe { MonitorFromPoint(POINT { x: cx, y: cy }, MONITOR_DEFAULTTONEAREST) };
    let scale = sdr_white_scale(hmon)?;
    grab(hmon, scale).ok()
}

fn grab(hmon: HMONITOR, scale: f32) -> windows::core::Result<RgbaImage> {
    // SAFETY: 以下は Windows の COM / WinRT の呼び出しで、引数はすべてこの関数内の有効な値
    unsafe {
        let mut dev: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )?;
        let (dev, ctx) = (dev.ok_or(windows::core::Error::empty())?, ctx.ok_or(windows::core::Error::empty())?);
        let dxgi: IDXGIDevice = dev.cast()?;
        let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(hmon)?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::R16G16B16A16Float, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture()?;
        let started = std::time::Instant::now();
        let frame = loop {
            if let Ok(f) = pool.TryGetNextFrame() {
                break f;
            }
            if started.elapsed() > std::time::Duration::from_secs(2) {
                return Err(windows::core::Error::empty());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let tex: ID3D11Texture2D = access.GetInterface()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        let mut sdesc = desc;
        sdesc.Usage = D3D11_USAGE_STAGING;
        sdesc.BindFlags = 0;
        sdesc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        sdesc.MiscFlags = 0;
        let mut staging: Option<ID3D11Texture2D> = None;
        dev.CreateTexture2D(&sdesc, None, Some(&mut staging))?;
        let staging = staging.ok_or(windows::core::Error::empty())?;
        ctx.CopyResource(&staging, &tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

        let table = table_for(scale);
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut out = vec![255u8; w * h * 4];
        for y in 0..h {
            let row = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize) as *const u16, w * 4);
            let dst = &mut out[y * w * 4..(y + 1) * w * 4];
            for (px, d) in row.as_chunks::<4>().0.iter().zip(dst.as_chunks_mut::<4>().0.iter_mut()) {
                d[0] = table[px[0] as usize];
                d[1] = table[px[1] as usize];
                d[2] = table[px[2] as usize];
            }
        }
        ctx.Unmap(&staging, 0);
        let _ = session.Close();
        let _ = pool.Close();
        RgbaImage::from_raw(w as u32, h as u32, out).ok_or(windows::core::Error::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0x4200), 3.0);
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        assert_eq!(f16_to_f32(0), 0.0);
    }

    #[test]
    fn boost_is_undone_for_srgb_values() {
        // sRGB 128 の線形値を 3 倍に持ち上げて、3 で割ると 128 に戻る
        let lin = (128.0f32 / 255.0 + 0.055) / 1.055;
        let lin = lin.powf(2.4);
        assert_eq!(srgb8(lin * 3.0 / 3.0), 128);
        assert_eq!(srgb8(f32::NAN), 0);
        assert_eq!(srgb8(5.0), 255);
    }
}
