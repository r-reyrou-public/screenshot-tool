//! 注釈を画像へ焼き込む。画面のプレビューと書き出しの両方でこの描画を使う。

use crate::shapes::{BubbleStyle, Geometry, Shape};
use crate::text;
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use image::RgbaImage;

/// 蛍光ペンの不透明度
const HIGHLIGHT_ALPHA: f32 = 0.4;

/// 注釈をすべて重ねた画像を返す。
pub fn render(base: &RgbaImage, shapes: &[Shape]) -> RgbaImage {
    let mut img = base.clone();
    for shape in shapes {
        draw_shape(&mut img, shape);
    }
    img
}

/// 1つの図形を `img` に重ねる。モザイクは `img` の現在の内容を元にする。
pub fn draw_shape(img: &mut RgbaImage, shape: &Shape) {
    let c = shape.color.to_srgba_unmultiplied();
    match &shape.geometry {
        Geometry::Highlight(_) => draw_highlight(img, shape, c),
        Geometry::Pen(_)
        | Geometry::Line(..)
        | Geometry::Arrow(..)
        | Geometry::DoubleArrow(..)
        | Geometry::CurveArrow { .. }
        | Geometry::Bracket { .. }
        | Geometry::Rect(..)
        | Geometry::Ellipse(..) => {
            let (w, h) = (img.width() as i32, img.height() as i32);
            let r = shape.stroke_width() / 2.0;
            for (a, b) in shape.segments() {
                for_each_cover((w, h), a, b, r, |x, y, cover| blend(img, x, y, c, cover));
            }
        }
        Geometry::Text { pos, text } => text::draw(img, *pos, text, shape.size, c),
        Geometry::Bubble { center, text, .. } => {
            let Some(parts) = shape.bubble_parts() else { return };
            let (rect, radius, tail, style) = (parts.rect, parts.radius, parts.tail, parts.style);
            let unit = shape.size;
            let sdf = move |p: Pos2| {
                let d = if style.is_oval() { sd_ellipse(p, rect) } else { sd_round_rect(p, rect, radius) };
                match (tail, style) {
                    // もくもく: 根元から先端へ、だんだん小さくなる丸を並べる
                    (Some([a, b, t]), BubbleStyle::Thought) => {
                        let base = a + (b - a) * 0.5;
                        [(0.28, 0.30), (0.58, 0.20), (0.86, 0.13)]
                            .iter()
                            .fold(d, |acc, &(pos, r)| acc.min((p - (base + (t - base) * pos)).length() - unit * r))
                    }
                    (Some([a, b, t]), _) => d.min(sd_triangle(p, a, b, t)),
                    (None, _) => d,
                }
            };
            draw_sdf(img, shape.bounds(), sdf, Some([255, 255, 255, 255]), Some((shape.width, c)));
            if !text.is_empty() {
                let size = text::measure(text, shape.size);
                text::draw(img, *center - size / 2.0, text, shape.size, c);
            }
        }
        Geometry::Number { center, n, style } => {
            let (center, r) = (*center, shape.number_radius());
            let (square, outline) = (style.is_square(), style.is_outline());
            // 四角は、丸と同じくらいの見た目の大きさになるよう少し小さめにとる
            let body = Rect::from_center_size(center, Vec2::splat(r * 1.88));
            let corner = r * 0.28;
            let sdf = move |p: Pos2| if square { sd_round_rect(p, body, corner) } else { (p - center).length() - r };
            if outline {
                // 塗りの逆: 白地に色の枠
                draw_sdf(img, shape.bounds(), sdf, Some([255, 255, 255, 255]), Some((shape.width, c)));
            } else {
                draw_sdf(img, shape.bounds(), sdf, Some(c), None);
            }
            let label = n.to_string();
            let ink = if outline {
                c
            } else {
                let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
                if lum > 170.0 { [20, 20, 20, 255] } else { [255, 255, 255, 255] }
            };
            // 行の高さではなく、実際に塗られる字形の範囲を中心に合わせる(数字が上に寄らない)
            let origin = match text::ink_bounds(&label, shape.size) {
                Some(b) => center - b.center().to_vec2(),
                None => center - text::measure(&label, shape.size) / 2.0,
            };
            text::draw(img, origin, &label, shape.size, ink);
        }
        // モザイクの強度(ブロックの1辺の画素数)は `width` に持つ
        Geometry::Mosaic(a, b) => mosaic(img, Rect::from_two_pos(*a, *b), shape.width.max(2.0) as u32),
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    crate::shapes::dist_to_segment(p, a, b)
}

/// 丸い端点を持つ太線が覆う各画素の被覆率(0..1)を通知する。
fn for_each_cover(dims: (i32, i32), a: Pos2, b: Pos2, radius: f32, mut f: impl FnMut(u32, u32, f32)) {
    let (w, h) = dims;
    let pad = radius.ceil() as i32 + 1;
    let x0 = (a.x.min(b.x).floor() as i32 - pad).max(0);
    let y0 = (a.y.min(b.y).floor() as i32 - pad).max(0);
    let x1 = (a.x.max(b.x).ceil() as i32 + pad).min(w - 1);
    let y1 = (a.y.max(b.y).ceil() as i32 + pad).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = dist_to_segment(pos2(x as f32 + 0.5, y as f32 + 0.5), a, b);
            let cover = (radius + 0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                f(x as u32, y as u32, cover);
            }
        }
    }
}

/// 蛍光ペン: 線が重なっても濃くならないよう、被覆率の最大値を取ってから一度だけ塗る。
fn draw_highlight(img: &mut RgbaImage, shape: &Shape, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let b = shape.bounds();
    let x0 = (b.min.x.floor() as i32).clamp(0, w);
    let y0 = (b.min.y.floor() as i32).clamp(0, h);
    let x1 = (b.max.x.ceil() as i32).clamp(0, w);
    let y1 = (b.max.y.ceil() as i32).clamp(0, h);
    let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    if bw == 0 || bh == 0 {
        return;
    }
    let mut mask = vec![0.0f32; bw * bh];
    let r = shape.stroke_width() / 2.0;
    for (a, b) in shape.segments() {
        for_each_cover((w, h), a, b, r, |x, y, cover| {
            let (mx, my) = (x as i32 - x0, y as i32 - y0);
            if mx >= 0 && my >= 0 && (mx as usize) < bw && (my as usize) < bh {
                let m = &mut mask[my as usize * bw + mx as usize];
                *m = m.max(cover);
            }
        });
    }
    for my in 0..bh {
        for mx in 0..bw {
            let cover = mask[my * bw + mx];
            if cover > 0.0 {
                blend(img, (x0 as usize + mx) as u32, (y0 as usize + my) as u32, color, cover * HIGHLIGHT_ALPHA);
            }
        }
    }
}

/// 符号付き距離関数で塗りと縁取りを描く(縁は距離0の線を中心にした帯)。
fn draw_sdf(
    img: &mut RgbaImage,
    bbox: Rect,
    sdf: impl Fn(Pos2) -> f32,
    fill: Option<[u8; 4]>,
    stroke: Option<(f32, [u8; 4])>,
) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = (bbox.min.x.floor() as i32).clamp(0, w);
    let y0 = (bbox.min.y.floor() as i32).clamp(0, h);
    let x1 = (bbox.max.x.ceil() as i32).clamp(0, w);
    let y1 = (bbox.max.y.ceil() as i32).clamp(0, h);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = sdf(pos2(x as f32 + 0.5, y as f32 + 0.5));
            if let Some(fc) = fill {
                let cover = (0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    blend(img, x as u32, y as u32, fc, cover);
                }
            }
            if let Some((sw, sc)) = stroke {
                let cover = (sw / 2.0 + 0.5 - d.abs()).clamp(0.0, 1.0);
                if cover > 0.0 {
                    blend(img, x as u32, y as u32, sc, cover);
                }
            }
        }
    }
}

fn sd_round_rect(p: Pos2, rect: Rect, radius: f32) -> f32 {
    let half = rect.size() / 2.0 - Vec2::splat(radius);
    let d = (p - rect.center()).abs() - half;
    vec2(d.x.max(0.0), d.y.max(0.0)).length() + d.x.max(d.y).min(0.0) - radius
}

/// 楕円の符号付き距離の近似(縁の近くでは十分正確)。
fn sd_ellipse(p: Pos2, rect: Rect) -> f32 {
    let half = rect.size() / 2.0;
    let q = (p - rect.center()) / half;
    (q.length() - 1.0) * half.x.min(half.y)
}

fn sd_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> f32 {
    let (e0, e1, e2) = (b - a, c - b, a - c);
    let (v0, v1, v2) = (p - a, p - b, p - c);
    let proj = |v: Vec2, e: Vec2| v - e * (v.dot(e) / e.dot(e)).clamp(0.0, 1.0);
    let (q0, q1, q2) = (proj(v0, e0), proj(v1, e1), proj(v2, e2));
    let s = (e0.x * e2.y - e0.y * e2.x).signum();
    let cross = |v: Vec2, e: Vec2| s * (v.x * e.y - v.y * e.x);
    let cands = [(q0.dot(q0), cross(v0, e0)), (q1.dot(q1), cross(v1, e1)), (q2.dot(q2), cross(v2, e2))];
    let dx = cands.iter().map(|c| c.0).fold(f32::MAX, f32::min);
    let dy = cands.iter().map(|c| c.1).fold(f32::MAX, f32::min);
    -dx.sqrt() * dy.signum()
}

/// 矩形を `block` 四方のセルに分け、各セルを平均色で塗りつぶす。
fn mosaic(img: &mut RgbaImage, r: Rect, block: u32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = (r.min.x.floor() as i32).clamp(0, w) as u32;
    let y0 = (r.min.y.floor() as i32).clamp(0, h) as u32;
    let x1 = (r.max.x.ceil() as i32).clamp(0, w) as u32;
    let y1 = (r.max.y.ceil() as i32).clamp(0, h) as u32;
    let block = block.max(2);
    let mut y = y0;
    while y < y1 {
        let ye = (y + block).min(y1);
        let mut x = x0;
        while x < x1 {
            let xe = (x + block).min(x1);
            let (mut sum, n) = ([0u32; 3], (xe - x) * (ye - y));
            for yy in y..ye {
                for xx in x..xe {
                    let px = img.get_pixel(xx, yy);
                    (0..3).for_each(|i| sum[i] += px[i] as u32);
                }
            }
            let avg = [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8];
            for yy in y..ye {
                for xx in x..xe {
                    let px = img.get_pixel_mut(xx, yy);
                    px[0] = avg[0];
                    px[1] = avg[1];
                    px[2] = avg[2];
                }
            }
            x = xe;
        }
        y = ye;
    }
}

pub fn blend(img: &mut RgbaImage, x: u32, y: u32, color: [u8; 4], cover: f32) {
    let px = img.get_pixel_mut(x, y);
    let a = cover * color[3] as f32 / 255.0;
    for i in 0..3 {
        px[i] = (color[i] as f32 * a + px[i] as f32 * (1.0 - a)).round() as u8;
    }
    px[3] = (255.0 * a + px[3] as f32 * (1.0 - a)).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::BubbleStyle;
    use egui::Color32;

    fn bubble(style: BubbleStyle, x: f32, size: Option<Vec2>) -> Shape {
        Shape {
            geometry: Geometry::Bubble {
                center: pos2(x, 90.0),
                text: "Hello".into(),
                tail: pos2(x - 40.0, 190.0),
                style,
                size,
            },
            color: Color32::from_rgb(220, 40, 40),
            width: 4.0,
            size: 28.0,
        }
    }

    /// 4種類の吹き出しが描画でき、背景が白以外になる(輪郭と文字が塗られる)ことを確かめる。
    /// 環境変数 `SS_TEST_DIR` にフォルダを渡すと、目視用に PNG も書き出す。
    #[test]
    fn bubble_styles_render() {
        let base = RgbaImage::from_pixel(1000, 260, image::Rgba([240, 240, 240, 255]));
        let shapes: Vec<Shape> = BubbleStyle::ALL
            .iter()
            .enumerate()
            .map(|(i, (style, _))| bubble(*style, 130.0 + i as f32 * 240.0, (i == 2).then(|| vec2(200.0, 120.0))))
            .collect();
        let out = render(&base, &shapes);
        let painted = out.pixels().filter(|p| p.0 != [240, 240, 240, 255]).count();
        assert!(painted > 5000, "bubbles were not drawn: {painted}");
        save_for_review(&out, "bubbles.png");
    }

    fn save_for_review(img: &RgbaImage, name: &str) {
        if let Some(dir) = std::env::var_os("SS_TEST_DIR") {
            img.save(std::path::Path::new(&dir).join(name)).unwrap();
        }
    }

    fn plain(geometry: Geometry, width: f32) -> Shape {
        Shape { geometry, color: Color32::from_rgb(220, 40, 40), width, size: 28.0 }
    }

    /// 括弧・両矢印・曲線矢印・番号の各表現・モザイクの強度が描画できることを確かめる。
    #[test]
    fn new_shapes_render() {
        use crate::shapes::{BracketStyle, NumberStyle};
        // モザイクの強度の違いが見えるよう、細かい模様(3px)と粗い模様(24px)を重ねた背景にする
        let base = RgbaImage::from_fn(1000, 420, |x, y| {
            let fine = (x / 3 + y / 3) % 2 == 0;
            let coarse = (x / 24 + y / 24) % 2 == 0;
            let v = match (fine, coarse) {
                (true, true) => 30,
                (false, true) => 120,
                (true, false) => 160,
                (false, false) => 235,
            };
            image::Rgba([v, v, v, 255])
        });
        let mut shapes = Vec::new();
        for (i, style) in [BracketStyle::Round, BracketStyle::Square, BracketStyle::Curly].into_iter().enumerate() {
            let x = 40.0 + i as f32 * 150.0;
            shapes.push(plain(Geometry::Bracket { a: pos2(x, 30.0), b: pos2(x + 110.0, 150.0), style }, 4.0));
        }
        shapes.push(plain(Geometry::DoubleArrow(pos2(500.0, 60.0), pos2(700.0, 60.0)), 4.0));
        shapes.push(plain(
            Geometry::CurveArrow { a: pos2(500.0, 150.0), b: pos2(720.0, 150.0), ctrl: pos2(610.0, 60.0) },
            4.0,
        ));
        for (i, (style, _)) in NumberStyle::ALL.iter().enumerate() {
            shapes.push(plain(
                Geometry::Number { center: pos2(60.0 + i as f32 * 90.0, 250.0), n: i as u32 + 1, style: *style },
                4.0,
            ));
        }
        // 同じ範囲の幅で、強度だけ変えたモザイク
        for (i, block) in [6.0f32, 14.0, 30.0].into_iter().enumerate() {
            let x = 460.0 + i as f32 * 170.0;
            shapes.push(plain(Geometry::Mosaic(pos2(x, 220.0), pos2(x + 150.0, 400.0)), block));
        }
        let out = render(&base, &shapes);
        save_for_review(&out, "shapes.png");
        let painted = out.pixels().zip(base.pixels()).filter(|(a, b)| a != b).count();
        assert!(painted > 20_000, "shapes were not drawn: {painted}");
    }
}
