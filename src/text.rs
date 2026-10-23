//! 注釈テキストの計測と描画。画面表示と書き出しで同じ処理を使い、見た目を一致させる。

use ab_glyph::{Font, FontArc, FontVec, PxScale, ScaleFont, point};
use egui::{Vec2, vec2};
use image::RgbaImage;
use std::sync::OnceLock;

fn font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| {
        // 太めの日本語フォントを優先する
        let dir = crate::winutil::fonts_dir()?;
        let candidates = ["YuGothB.ttc", "meiryob.ttc", "YuGothM.ttc", "meiryo.ttc", "msgothic.ttc", "arialbd.ttf"];
        candidates.iter().find_map(|name| {
            let bytes = std::fs::read(dir.join(name)).ok()?;
            FontVec::try_from_vec_and_index(bytes, 0).ok().map(FontArc::new)
        })
    })
    .as_ref()
}

fn line_height(size: f32) -> f32 {
    font().map_or(size * 1.3, |f| {
        let s = f.as_scaled(PxScale::from(size));
        s.height() + s.line_gap()
    })
}

fn line_width(text: &str, size: f32) -> f32 {
    let Some(f) = font() else { return text.chars().count() as f32 * size * 0.6 };
    let s = f.as_scaled(PxScale::from(size));
    text.chars().map(|c| s.h_advance(f.glyph_id(c))).sum()
}

/// 複数行テキストの大きさ(画像ピクセル)。
pub fn measure(text: &str, size: f32) -> Vec2 {
    let lines: Vec<&str> = text.split('\n').collect();
    let w = lines.iter().map(|l| line_width(l, size)).fold(0.0, f32::max);
    vec2(w, lines.len() as f32 * line_height(size))
}

/// 1行のテキストを `draw` で描いたときの、実際に塗られる範囲(`pos` からの相対)。
/// 行の高さや送り幅ではなく字形そのものの範囲なので、数字を円の中心に置くときなどに使う。
pub fn ink_bounds(text: &str, size: f32) -> Option<egui::Rect> {
    let f = font()?;
    let scale = PxScale::from(size);
    let s = f.as_scaled(scale);
    let baseline = s.ascent();
    let mut x = 0.0;
    let mut bounds: Option<egui::Rect> = None;
    for ch in text.lines().next().unwrap_or("").chars() {
        let id = f.glyph_id(ch);
        if let Some(og) = f.outline_glyph(id.with_scale_and_position(scale, point(x, baseline))) {
            let b = og.px_bounds();
            let r = egui::Rect::from_min_max(egui::pos2(b.min.x, b.min.y), egui::pos2(b.max.x, b.max.y));
            bounds = Some(bounds.map_or(r, |acc| acc.union(r)));
        }
        x += s.h_advance(id);
    }
    bounds
}

/// `pos` を左上として文字を描く。
pub fn draw(img: &mut RgbaImage, pos: egui::Pos2, text: &str, size: f32, color: [u8; 4]) {
    let Some(f) = font() else { return };
    let scale = PxScale::from(size);
    let s = f.as_scaled(scale);
    let (w, h) = (img.width() as i32, img.height() as i32);
    for (li, line) in text.split('\n').enumerate() {
        let baseline = pos.y + li as f32 * line_height(size) + s.ascent();
        let mut x = pos.x;
        for ch in line.chars() {
            let id = f.glyph_id(ch);
            let glyph = id.with_scale_and_position(scale, point(x, baseline));
            if let Some(og) = f.outline_glyph(glyph) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cover| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 && px < w && py < h {
                        crate::raster::blend(img, px as u32, py as u32, color, cover);
                    }
                });
            }
            x += s.h_advance(id);
        }
    }
}
