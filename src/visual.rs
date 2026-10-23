//! 画面の見た目から「要素のかたまり」を切り出す。
//!
//! UI Automation がページ内の要素(Webページの見出し・画像・ボタンなど)を公開しないアプリでも、
//! 撮影済みの画像だけを使って、カーソル位置の文字・画像・ボタンのまとまりを矩形で返す。

use image::RgbaImage;
use std::collections::VecDeque;

/// 辺とみなす輝度差(隣り合う画素の差の和)
const EDGE: i32 = 28;
/// 調べる範囲(カーソルから上下左右にこの画素数まで)
const HALF: i32 = 450;

/// `img` 上の画素 (px, py) にあるかたまりの外接矩形 `(x0, y0, x1, y1)`(画像座標)を、
/// `gaps` の各値(隙間がこの画素数以下なら同じかたまりとみなす)ごとに求める。
/// 大きい `gap` ほど大きなかたまりになる。
///
/// - `bounds`: 調べる範囲の上限 `(x0, y0, x1, y1)`。ペインの枠などを渡す
/// - 戻り値は `(gap, 矩形)` の並びで、求められなかったもの(カーソルの近くに何も無い、
///   かたまりが範囲全体に及ぶ、など)は含まない
///
/// 輝度と辺の計算は `gaps` で共通なので、複数の大きさを一度に求めても重くならない。
pub fn blocks_at(
    img: &RgbaImage,
    px: i32,
    py: i32,
    bounds: (i32, i32, i32, i32),
    gaps: &[u32],
) -> Vec<(u32, (i32, i32, i32, i32))> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = (px - HALF).max(bounds.0).max(0);
    let y0 = (py - HALF).max(bounds.1).max(0);
    let x1 = (px + HALF).min(bounds.2).min(w);
    let y1 = (py + HALF).min(bounds.3).min(h);
    if x1 - x0 < 8 || y1 - y0 < 8 || px < x0 || px >= x1 || py < y0 || py >= y1 {
        return vec![];
    }
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);

    // 輝度
    let mut luma = vec![0u8; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = img.get_pixel((x0 as usize + x) as u32, (y0 as usize + y) as u32);
            luma[y * rw + x] = ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8;
        }
    }

    // 辺
    let mut edge = vec![false; rw * rh];
    for y in 1..rh - 1 {
        for x in 1..rw - 1 {
            let i = y * rw + x;
            let gx = (luma[i + 1] as i32 - luma[i - 1] as i32).abs();
            let gy = (luma[i + rw] as i32 - luma[i - rw] as i32).abs();
            edge[i] = gx + gy >= EDGE;
        }
    }

    let (cx, cy) = ((px - x0), (py - y0));
    let area = |r: (i32, i32, i32, i32)| (r.2 - r.0).max(0) as i64 * (r.3 - r.1).max(0) as i64;
    let mut out = Vec::new();
    for &gap in gaps {
        // 隙間 `gap` 以下の辺同士をつなぐため、半径 r だけ膨らませる(横→縦の2段階。累積和で O(N))
        let r = (gap / 2).max(1) as usize;
        let dil = dilate(&dilate_h(&edge, rw, rh, r), rw, rh, r, true);

        // 種: カーソルに最も近い膨らませた画素(近くに無ければ余白とみなして除外)
        let reach = (gap * 2).max(8) as i32;
        let Some(seed) = (0..=reach)
            .find_map(|d| ring(cx, cy, d, rw as i32, rh as i32).find(|&(x, y)| dil[y as usize * rw + x as usize]))
        else {
            continue;
        };

        // 種から広がる連結成分のうち、もとの辺の画素の外接矩形を求める
        let mut seen = vec![false; rw * rh];
        let mut queue = VecDeque::from([seed]);
        seen[seed.1 as usize * rw + seed.0 as usize] = true;
        let (mut minx, mut miny, mut maxx, mut maxy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        while let Some((x, y)) = queue.pop_front() {
            if edge[y as usize * rw + x as usize] {
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if nx >= 0 && ny >= 0 && (nx as usize) < rw && (ny as usize) < rh {
                    let i = ny as usize * rw + nx as usize;
                    if dil[i] && !seen[i] {
                        seen[i] = true;
                        queue.push_back((nx, ny));
                    }
                }
            }
        }
        if minx > maxx {
            continue;
        }
        // 少し余白を付ける
        let pad = 3;
        let rect = (
            (x0 + minx - pad).max(bounds.0).max(0),
            (y0 + miny - pad).max(bounds.1).max(0),
            (x0 + maxx + 1 + pad).min(bounds.2).min(w),
            (y0 + maxy + 1 + pad).min(bounds.3).min(h),
        );
        // 範囲のほぼ全体に及ぶ(つまり絞れていない)ものは使わない
        if area(rect) * 100 < area(bounds) * 90 && rect.2 - rect.0 >= 6 && rect.3 - rect.1 >= 6 {
            out.push((gap, rect));
        }
    }
    out
}
/// 中心 (cx, cy) からチェビシェフ距離 d の正方形の輪にある、範囲内の座標。
fn ring(cx: i32, cy: i32, d: i32, w: i32, h: i32) -> impl Iterator<Item = (i32, i32)> {
    let pts: Vec<(i32, i32)> = if d == 0 {
        vec![(cx, cy)]
    } else {
        let mut v = Vec::new();
        for i in -d..=d {
            v.push((cx + i, cy - d));
            v.push((cx + i, cy + d));
            v.push((cx - d, cy + i));
            v.push((cx + d, cy + i));
        }
        v
    };
    pts.into_iter().filter(move |&(x, y)| x >= 0 && y >= 0 && x < w && y < h)
}

/// 横方向に半径 r だけ膨らませる。
fn dilate_h(src: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        // 累積和
        let mut acc = vec![0u32; w + 1];
        for x in 0..w {
            acc[x + 1] = acc[x] + row[x] as u32;
        }
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r + 1).min(w));
            out[y * w + x] = acc[hi] - acc[lo] > 0;
        }
    }
    out
}

/// 縦方向に半径 r だけ膨らませる(`vertical` が真のとき)。
fn dilate(src: &[bool], w: usize, h: usize, r: usize, vertical: bool) -> Vec<bool> {
    if !vertical {
        return dilate_h(src, w, h, r);
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut acc = vec![0u32; h + 1];
        for y in 0..h {
            acc[y + 1] = acc[y] + src[y * w + x] as u32;
        }
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = acc[hi] - acc[lo] > 0;
        }
    }
    out
}
