//! スクロール撮影: 範囲をホイールで送りながら何枚も撮り、重なりを探して縦に1枚へつなぐ。

use crate::capture;
use image::RgbaImage;
use std::time::{Duration, Instant};

/// つないだ画像の高さの上限(これを超えたら打ち切る)
const MAX_HEIGHT: usize = 30_000;
/// 送りの最大回数
const MAX_STEPS: usize = 80;

/// 1行ぶんの特徴: (ハッシュ, 全画素が同じ色か)。
/// 右端はスクロールバー(つまみが動く)なので、比べる対象から外す。
fn row_info(img: &RgbaImage) -> Vec<(u64, bool)> {
    let w = img.width() as usize;
    let (x0, x1) = (w / 50, w - (w / 25).clamp(1, 24).min(w / 3));
    img.as_raw()
        .chunks_exact(w * 4)
        .map(|row| {
            let row = &row[x0 * 4..x1 * 4];
            let mut h: u64 = 0xcbf29ce484222325;
            for c in row.as_chunks::<8>().0 {
                h = (h ^ u64::from_le_bytes(*c)).wrapping_mul(0x100000001b3);
            }
            let uniform = row.as_chunks::<4>().0.iter().all(|p| p[..] == row[..4]);
            (h, uniform)
        })
        .collect()
}

pub enum Step {
    /// 新しい内容を、この行数だけ足した
    Added(u32),
    /// 前の画像と同じ(これ以上スクロールしない)
    End,
    /// 重なりが見つからず、つなげなかった
    Lost,
}

/// 撮った画像を、上から順に1枚へつなぐ。
pub struct Stitcher {
    w: u32,
    h: u32,
    /// つないだ結果(下端の固定部分は、最後にまとめて足すので含まない)
    raw: Vec<u8>,
    prev: RgbaImage,
    /// 画面の上端・下端の、スクロールしても動かない部分(ヘッダーやステータスバー)の行数。最初の送りで決める
    bands: Option<(usize, usize)>,
}

impl Stitcher {
    pub fn new(first: RgbaImage) -> Self {
        Stitcher { w: first.width(), h: first.height(), raw: first.as_raw().clone(), prev: first, bands: None }
    }

    pub fn height(&self) -> usize {
        self.raw.len() / (self.w as usize * 4)
    }

    pub fn push(&mut self, next: RgbaImage) -> Step {
        if next.width() != self.w || next.height() != self.h {
            return Step::Lost;
        }
        let h = self.h as usize;
        let row = self.w as usize * 4;
        let (ha, hb) = (row_info(&self.prev), row_info(&next));
        let same = (0..h).filter(|&i| ha[i].0 == hb[i].0).count();
        if same * 1000 >= h * 995 {
            return Step::End;
        }
        let (ts, bs) = match self.bands {
            Some(b) => b,
            None => {
                let cap = h / 3;
                let ts = (0..h).take_while(|&i| ha[i].0 == hb[i].0).count().min(cap);
                let bs = (0..h).take_while(|&i| ha[h - 1 - i].0 == hb[h - 1 - i].0).count().min(cap);
                // 最初の1枚の下端の固定部分は、最後に足し直す
                self.raw.truncate((h - bs) * row);
                self.bands = Some((ts, bs));
                (ts, bs)
            }
        };
        let end = h - bs;
        let len = end - ts;
        // B の行 i(ts..)が A の行 i+d と同じになる d を探す。単色の行は決め手にならないので数えない
        let mut best: Option<(usize, usize)> = None; // (一致数, d)
        for d in 1..len {
            let (mut matched, mut total) = (0usize, 0usize);
            for i in ts..end - d {
                if hb[i].1 {
                    continue;
                }
                total += 1;
                if ha[i + d].0 == hb[i].0 {
                    matched += 1;
                }
            }
            if matched >= 6 && matched * 100 >= total * 85 && best.is_none_or(|(m, _)| matched > m) {
                best = Some((matched, d));
            }
        }
        let Some((_, d)) = best else { return Step::Lost };
        self.raw.extend_from_slice(&next.as_raw()[(end - d) * row..end * row]);
        self.prev = next;
        Step::Added(d as u32)
    }

    pub fn finish(mut self) -> RgbaImage {
        let h = self.h as usize;
        let row = self.w as usize * 4;
        if let Some((_, bs)) = self.bands {
            // 下端の固定部分は、最後の画像のものを1回だけ足す
            self.raw.extend_from_slice(&self.prev.as_raw()[(h - bs) * row..]);
        }
        let rows = self.raw.len() / row;
        RgbaImage::from_raw(self.w, rows as u32, self.raw).expect("rows * width * 4")
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn esc_pressed() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    // SAFETY: 引数は有効な仮想キーコード
    unsafe { GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000 != 0 }
}

fn wheel(notches: i32) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_WHEEL, mouse_event};
    // SAFETY: 引数は値渡しのみ
    unsafe { mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -120 * notches, 0) };
}

fn set_cursor(x: i32, y: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
    // SAFETY: 引数は値渡しのみ
    unsafe { SetCursorPos(x, y) };
}

/// 範囲 `rect`(物理座標 left, top, right, bottom)を撮る。
fn grab(rect: (i32, i32, i32, i32)) -> Option<RgbaImage> {
    let shot = capture::capture_monitor_at((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2).ok()?;
    let (iw, ih) = (shot.image.width() as i32, shot.image.height() as i32);
    let x0 = (rect.0 - shot.origin.0).clamp(0, iw - 1);
    let y0 = (rect.1 - shot.origin.1).clamp(0, ih - 1);
    let x1 = (rect.2 - shot.origin.0).clamp(x0 + 1, iw);
    let y1 = (rect.3 - shot.origin.1).clamp(y0 + 1, ih);
    Some(image::imageops::crop_imm(&shot.image, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image())
}

/// スクロールが止まる(連続した2枚が同じになる)まで待って撮る。
fn grab_settled(rect: (i32, i32, i32, i32)) -> Option<RgbaImage> {
    let mut last = grab(rect)?;
    for _ in 0..10 {
        pause(120);
        let cur = grab(rect)?;
        if cur.as_raw() == last.as_raw() {
            return Some(cur);
        }
        last = cur;
    }
    Some(last)
}

/// `rect` の中央にカーソルを置いてホイールで送りながら撮り、1枚につなぐ。`Esc` で中止(`None`)。
pub fn capture_scrolling(rect: (i32, i32, i32, i32)) -> Option<RgbaImage> {
    let (cx, cy) = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
    let height = (rect.3 - rect.1).max(1);
    set_cursor(cx, cy);
    pause(150);
    let mut st = Stitcher::new(grab_settled(rect)?);
    // 1回の送りが範囲の半分くらいになるように、最初の結果を見て刻みを合わせる
    let mut notches = if height < 400 { 1 } else { 3 };
    let mut tuned = false;
    let started = Instant::now();
    for _ in 0..MAX_STEPS {
        if esc_pressed() {
            return None;
        }
        set_cursor(cx, cy);
        wheel(notches);
        pause(250);
        let Some(next) = grab_settled(rect) else { break };
        match st.push(next) {
            Step::Added(d) => {
                if !tuned {
                    tuned = true;
                    notches = ((notches as f32 * height as f32 * 0.5 / d as f32).round() as i32).clamp(1, 30);
                }
            }
            Step::End => break,
            // 一度に送りすぎて前の画像と重ならなかった。元の位置へ戻し、送る量を半分にしてやり直す
            Step::Lost if notches > 1 => {
                wheel(-notches);
                pause(250);
                notches = (notches / 2).max(1);
                tuned = true;
                let _ = grab_settled(rect);
            }
            Step::Lost => break,
        }
        if st.height() >= MAX_HEIGHT || started.elapsed() > Duration::from_secs(120) {
            break;
        }
    }
    Some(st.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// 縦に長い「ページ」。行ごとに内容が違う
    fn page(total: u32, w: u32) -> RgbaImage {
        RgbaImage::from_fn(w, total, |x, y| Rgba([(y * 7 % 251) as u8, (y / 3 % 253) as u8, (x * 31 % 255) as u8, 255]))
    }

    fn view(page: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(page, 0, top, page.width(), h).to_image()
    }

    #[test]
    fn stitches_scrolled_views_back_into_the_page() {
        let p = page(700, 40);
        let mut st = Stitcher::new(view(&p, 0, 200));
        assert!(matches!(st.push(view(&p, 120, 200)), Step::Added(120)));
        assert!(matches!(st.push(view(&p, 240, 200)), Step::Added(120)));
        // 末尾付近は送り量が小さくなり、その次は同じ画像になる
        assert!(matches!(st.push(view(&p, 500, 200)), Step::Lost));
        assert!(matches!(st.push(view(&p, 240, 200)), Step::End));
        let out = st.finish();
        assert_eq!(out.dimensions(), (40, 440));
        assert_eq!(out.as_raw(), view(&p, 0, 440).as_raw());
    }

    #[test]
    fn keeps_fixed_header_and_footer_once() {
        let p = page(900, 30);
        let header = |top: u32| {
            let mut v = view(&p, top, 200);
            for y in 0..20 {
                for x in 0..30 {
                    v.put_pixel(x, y, Rgba([9, 9, 200 + (x as u8 % 3), 255]));
                }
            }
            for y in 180..200 {
                for x in 0..30 {
                    v.put_pixel(x, y, Rgba([200, (x as u8 * 3) % 100, 9, 255]));
                }
            }
            v
        };
        let mut st = Stitcher::new(header(0));
        assert!(matches!(st.push(header(100)), Step::Added(100)));
        assert!(matches!(st.push(header(200)), Step::Added(100)));
        let out = st.finish();
        // 先頭の1枚(200行) + 100 + 100。ヘッダーとフッターは1回ずつ
        assert_eq!(out.height(), 400);
        assert_eq!(out.get_pixel(0, 0), &Rgba([9, 9, 200, 255]));
        assert_eq!(out.get_pixel(0, 399), &Rgba([200, 0, 9, 255]));
        assert_eq!(out.get_pixel(0, 250), p.get_pixel(0, 250));
    }
}
