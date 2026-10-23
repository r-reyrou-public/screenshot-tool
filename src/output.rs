use crate::config::{self, Config};
use anyhow::{Context, Result};
use image::RgbaImage;
use std::path::PathBuf;

/// 命名規則・形式に従って保存先パスを決める(重複時は連番を付ける)。
fn next_path(cfg: &Config) -> PathBuf {
    let ext = if cfg.is_jpeg() { "jpg" } else { "png" };
    let stem = config::format_name(&cfg.name_pattern)
        .or_else(|| config::format_name(&config::Config::default().name_pattern))
        .unwrap_or_else(|| "Screenshot".into());
    let mut path = cfg.save_dir.join(format!("{stem}.{ext}"));
    let mut n = 1;
    while path.exists() {
        path = cfg.save_dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    path
}

/// 設定の倍率・長辺上限に従って出力解像度を変える。
pub fn apply_resolution(img: &RgbaImage, cfg: &Config) -> RgbaImage {
    let mut k = cfg.scale_percent.max(1) as f32 / 100.0;
    let long = img.width().max(img.height()) as f32;
    if cfg.max_long_edge > 0 && long * k > cfg.max_long_edge as f32 {
        k = cfg.max_long_edge as f32 / long;
    }
    if (k - 1.0).abs() < 1e-3 {
        return img.clone();
    }
    let w = ((img.width() as f32 * k).round() as u32).max(1);
    let h = ((img.height() as f32 * k).round() as u32).max(1);
    image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3)
}

pub fn save(img: &RgbaImage, cfg: &Config) -> Result<PathBuf> {
    std::fs::create_dir_all(&cfg.save_dir)
        .with_context(|| format!("保存先を作成できません: {}", cfg.save_dir.display()))?;
    let path = next_path(cfg);
    if cfg.is_jpeg() {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let file = std::fs::File::create(&path)?;
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::BufWriter::new(file),
            cfg.jpeg_quality.clamp(1, 100),
        );
        rgb.write_with_encoder(enc)?;
    } else {
        img.save_with_format(&path, image::ImageFormat::Png)?;
    }
    Ok(path)
}

pub fn copy_to_clipboard(img: &RgbaImage) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_image(arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: std::borrow::Cow::Borrowed(img.as_raw()),
    })?;
    Ok(())
}
