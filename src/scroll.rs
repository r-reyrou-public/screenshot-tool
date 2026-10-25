//! スクロール撮影: 範囲をホイールで送りながら何枚も撮り、重なりを探して縦に1枚へつなぐ。

use crate::capture;
use image::RgbaImage;
use std::time::{Duration, Instant};

/// つないだ画像の高さの上限(これを超えたら打ち切る)
const MAX_HEIGHT: usize = 30_000;
/// 送りの最大回数
const MAX_STEPS: usize = 80;

/// 2枚の間で、見た目が変わった列の範囲 (左, 右)(右は含まない)。
/// 動いた行が1割に満たない列(点滅するカーソルなど)は、変わったとは数えない。
fn changed_cols(a: &RgbaImage, b: &RgbaImage) -> Option<(usize, usize)> {
    let (w, h) = (a.width() as usize, a.height() as usize);
    let mut count = vec![0usize; w];
    for (ra, rb) in a.as_raw().chunks_exact(w * 4).zip(b.as_raw().chunks_exact(w * 4)) {
        for (x, (pa, pb)) in ra.as_chunks::<4>().0.iter().zip(rb.as_chunks::<4>().0).enumerate() {
            count[x] += usize::from(pa != pb);
        }
    }
    let min = (h / 10).max(2);
    // 動いた列を、近いもの同士(隙間 GAP 列まで)で塊にし、いちばん幅の広い塊を返す。
    // 端に写り込んだ別のウィンドウなどで、離れた列まで動いたことになるのを防ぐ
    const GAP: usize = 40;
    let mut best: Option<(usize, usize)> = None;
    let mut run: Option<(usize, usize)> = None;
    for x in (0..w).filter(|&x| count[x] >= min) {
        run = match run {
            Some((f, l)) if x - l <= GAP => Some((f, x)),
            _ => {
                best = widest(best, run);
                Some((x, x))
            }
        };
    }
    best = widest(best, run);
    best.map(|(f, l)| (f, l + 1))
}

fn widest(a: Option<(usize, usize)>, b: Option<(usize, usize)>) -> Option<(usize, usize)> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.1 - y.0 > x.1 - x.0 { y } else { x }),
        (x, None) => x,
        (None, y) => y,
    }
}

/// 列 `x` が、上から下まで同じ色か
fn column_is_uniform(img: &RgbaImage, x: usize) -> bool {
    let w = img.width() as usize;
    let raw = img.as_raw();
    let first = &raw[x * 4..x * 4 + 4];
    raw.chunks_exact(w * 4).all(|row| &row[x * 4..x * 4 + 4] == first)
}

/// 1行ぶんの特徴: (ハッシュ, 全画素が同じ色か)。比べるのは列 `x0..x1` だけ。
fn row_info(img: &RgbaImage, (x0, x1): (usize, usize)) -> Vec<(u64, bool)> {
    let w = img.width() as usize;
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
    /// 比べる列の範囲(動いた列から、右端のスクロールバーを除いたもの)。最初の送りで決める
    cols: (usize, usize),
    /// 動かない列に内容(サイドバーなど)があるときは、動いた列の範囲だけを最後に切り出す
    crop: Option<(usize, usize)>,
}

impl Stitcher {
    pub fn new(first: RgbaImage) -> Self {
        Stitcher { w: first.width(), h: first.height(), raw: first.as_raw().clone(), prev: first, bands: None, cols: (0, 0), crop: None }
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
        if self.bands.is_none() {
            // 最初の送りで、動いた列を調べる。何も動いていなければ、これ以上スクロールしない
            let Some((l, r)) = changed_cols(&self.prev, &next) else { return Step::End };
            // 右端はスクロールバー(つまみだけが動く)なので、比べる対象から外す
            let bar = ((r - l) / 25).clamp(1, 24).min((r - l) / 3);
            self.cols = (l, (r - bar).max(l + 1));
            let w = self.w as usize;
            if (0..l).chain(r..w).any(|x| !column_is_uniform(&self.prev, x)) {
                self.crop = Some((l, r));
            }
        }
        let (ha, hb) = (row_info(&self.prev, self.cols), row_info(&next, self.cols));
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
        let img = RgbaImage::from_raw(self.w, rows as u32, self.raw).expect("rows * width * 4");
        match self.crop {
            Some((l, r)) => image::imageops::crop_imm(&img, l as u32, 0, (r - l) as u32, rows as u32).to_image(),
            None => img,
        }
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

/// ホイールで内容が動く場所を探す。中央から始め、動かなければ範囲内のほかの場所を順に試す
/// (サイドバーと並んだ画面などで、中央がスクロールしない部分のことがある)。
fn find_scroll_point(rect: (i32, i32, i32, i32), first: &RgbaImage) -> (i32, i32) {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    let at = |fx: f32, fy: f32| (rect.0 + (w as f32 * fx) as i32, rect.1 + (h as f32 * fy) as i32);
    let mut spots = vec![at(0.5, 0.5)];
    for fy in [0.5, 0.3, 0.7] {
        for fx in [0.3, 0.7, 0.15, 0.85, 0.5] {
            spots.push(at(fx, fy));
        }
    }
    for (x, y) in spots {
        set_cursor(x, y);
        pause(60);
        wheel(1);
        pause(250);
        let Some(now) = grab_settled(rect) else { break };
        if now.as_raw() != first.as_raw() {
            // 動いた。元の位置へ戻す
            wheel(-1);
            pause(250);
            let _ = grab_settled(rect);
            return (x, y);
        }
    }
    at(0.5, 0.5)
}

/// 範囲 `rect` の中でホイールを送りながら撮り、1枚につなぐ。`Esc` で中止(`None`)。
pub fn capture_scrolling(rect: (i32, i32, i32, i32)) -> Option<RgbaImage> {
    let height = (rect.3 - rect.1).max(1);
    set_cursor((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
    pause(150);
    let first = grab_settled(rect)?;
    let (cx, cy) = find_scroll_point(rect, &first);
    // 途中までスクロールされていても、上端から撮るために先頭まで戻す
    let mut top = grab_settled(rect).unwrap_or(first);
    for _ in 0..40 {
        if esc_pressed() {
            return None;
        }
        set_cursor(cx, cy);
        wheel(-10);
        pause(250);
        let Some(now) = grab_settled(rect) else { break };
        if now.as_raw() == top.as_raw() {
            break;
        }
        top = now;
    }
    let mut st = Stitcher::new(top);
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

    #[test]
    fn ignores_noise_strip_at_the_edge() {
        let p = page(700, 40);
        // 左端の 6 列は、スクロールとは無関係に毎回変わる(別のウィンドウが写り込んだ縁)。次に 50 列のサイドバー
        let frame = |top: u32, noise: u32| {
            RgbaImage::from_fn(96, 200, |x, y| {
                if x < 6 {
                    Rgba([(y * 5 + noise * 37) as u8, noise as u8, 1, 255])
                } else if x < 56 {
                    Rgba([(y * 13 % 251) as u8, 90, (x * 40) as u8, 255])
                } else {
                    *p.get_pixel(x - 56, y + top)
                }
            })
        };
        let mut st = Stitcher::new(frame(0, 0));
        assert!(matches!(st.push(frame(120, 1)), Step::Added(120)));
        assert!(matches!(st.push(frame(240, 2)), Step::Added(120)));
        let out = st.finish();
        assert_eq!(out.dimensions(), (40, 440));
        assert_eq!(out.as_raw(), view(&p, 0, 440).as_raw());
    }

    #[test]
    fn ignores_fixed_sidebar_and_crops_to_scrolling_pane() {
        let p = page(700, 40);
        let side = |v: RgbaImage| {
            // 左 30 列は、スクロールしない縞模様のサイドバー
            RgbaImage::from_fn(70, 200, |x, y| {
                if x < 30 { Rgba([(y * 13 % 251) as u8, 90, (x * 40) as u8, 255]) } else { *v.get_pixel(x - 30, y) }
            })
        };
        let mut st = Stitcher::new(side(view(&p, 0, 200)));
        assert!(matches!(st.push(side(view(&p, 120, 200))), Step::Added(120)));
        assert!(matches!(st.push(side(view(&p, 240, 200))), Step::Added(120)));
        assert!(matches!(st.push(side(view(&p, 240, 200))), Step::End));
        let out = st.finish();
        assert_eq!(out.dimensions(), (40, 440));
        assert_eq!(out.as_raw(), view(&p, 0, 440).as_raw());
    }

    /// 実際の画面をスクロール撮影する手元確認用。`SM_RECT="left,top,right,bottom"`(物理座標)と `SM_OUT`(保存先 PNG)を指定する。
    #[test]
    #[ignore]
    fn live_scroll() {
        let v: Vec<i32> = std::env::var("SM_RECT").unwrap().split(',').map(|n| n.trim().parse().unwrap()).collect();
        let img = capture_scrolling((v[0], v[1], v[2], v[3])).expect("cancelled");
        img.save(std::env::var("SM_OUT").unwrap()).unwrap();
        println!("{}x{}", img.width(), img.height());
    }
}
