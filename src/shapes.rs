use crate::text;
use egui::{Color32, Pos2, Rect, Vec2, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Line,
    Arrow,
    DoubleArrow,
    CurveArrow,
    Rect,
    Ellipse,
    Bracket,
    Text,
    Bubble,
    Number,
    Mosaic,
    Crop,
}

#[derive(Clone, Debug)]
pub enum Geometry {
    Pen(Vec<Pos2>),
    Highlight(Vec<Pos2>),
    Line(Pos2, Pos2),
    Arrow(Pos2, Pos2),
    /// 両端に矢じりのある矢印
    DoubleArrow(Pos2, Pos2),
    /// 曲線の矢印。`ctrl` が曲がりを決める制御点(2次ベジェ)
    CurveArrow {
        a: Pos2,
        b: Pos2,
        ctrl: Pos2,
    },
    Rect(Pos2, Pos2),
    Ellipse(Pos2, Pos2),
    /// 対象を左右から挟む括弧。2点が成す矩形の左右の辺に沿って描く
    Bracket {
        a: Pos2,
        b: Pos2,
        style: BracketStyle,
    },
    /// `pos` が左上
    Text {
        pos: Pos2,
        text: String,
    },
    /// 吹き出し。`size` が `None` なら文字に合わせた最小の大きさ。ハンドルで広げると `Some`
    Bubble {
        center: Pos2,
        text: String,
        tail: Pos2,
        style: BubbleStyle,
        size: Option<Vec2>,
    },
    Number {
        center: Pos2,
        n: u32,
        style: NumberStyle,
    },
    Mosaic(Pos2, Pos2),
}

/// 画像ピクセル座標で保持する注釈図形。
#[derive(Clone, Debug)]
pub struct Shape {
    pub geometry: Geometry,
    pub color: Color32,
    /// 線の太さ(モザイクでは粗さ)
    pub width: f32,
    /// 文字・番号の大きさ(px)
    pub size: f32,
}

/// 括弧の種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BracketStyle {
    /// ( )
    #[default]
    Round,
    /// [ ]
    Square,
    /// { }
    Curly,
}

impl BracketStyle {
    pub const ALL: [(BracketStyle, &'static str); 3] = [
        (BracketStyle::Round, "丸括弧 ( )"),
        (BracketStyle::Square, "角括弧 [ ]"),
        (BracketStyle::Curly, "波括弧 { }"),
    ];
}

/// 番号マーカーの見た目。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NumberStyle {
    /// 塗りつぶした丸に、抜き色の数字
    #[default]
    Filled,
    /// 白地に色の枠と色の数字(塗りの逆)
    Outline,
    /// 塗りつぶした四角
    FilledSquare,
    /// 白地に色の枠の四角
    OutlineSquare,
}

impl NumberStyle {
    pub const ALL: [(NumberStyle, &'static str); 4] = [
        (NumberStyle::Filled, "● 丸(塗り)"),
        (NumberStyle::Outline, "○ 丸(逆)"),
        (NumberStyle::FilledSquare, "■ 四角(塗り)"),
        (NumberStyle::OutlineSquare, "□ 四角(逆)"),
    ];

    pub fn is_square(self) -> bool {
        matches!(self, NumberStyle::FilledSquare | NumberStyle::OutlineSquare)
    }

    /// 白地に色の枠(塗りの逆)かどうか。
    pub fn is_outline(self) -> bool {
        matches!(self, NumberStyle::Outline | NumberStyle::OutlineSquare)
    }
}

/// 吹き出しの輪郭の種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BubbleStyle {
    /// 角丸の四角
    #[default]
    Rounded,
    /// 楕円
    Ellipse,
    /// 角のある四角
    Square,
    /// もくもく(考えごと): 楕円+小さな丸が連なるしっぽ
    Thought,
}

impl BubbleStyle {
    pub const ALL: [(BubbleStyle, &'static str); 4] = [
        (BubbleStyle::Rounded, "角丸"),
        (BubbleStyle::Ellipse, "楕円"),
        (BubbleStyle::Square, "四角"),
        (BubbleStyle::Thought, "もくもく"),
    ];

    /// 本体が楕円かどうか(楕円は四角より文字が収まりにくいので、余白を多めにとる)
    pub fn is_oval(self) -> bool {
        matches!(self, BubbleStyle::Ellipse | BubbleStyle::Thought)
    }
}

/// 吹き出しの形: 本体の矩形・角丸半径・しっぽ(根元2点+先端)。
pub struct BubbleParts {
    pub rect: Rect,
    pub radius: f32,
    pub style: BubbleStyle,
    pub tail: Option<[Pos2; 3]>,
}

fn rect_of(a: Pos2, b: Pos2) -> Rect {
    Rect::from_two_pos(a, b)
}

fn bounds_of(points: &[Pos2]) -> Rect {
    points.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)))
}

impl Shape {
    /// 線として描く図形の実効の太さ。蛍光ペンは太く描く。
    pub fn stroke_width(&self) -> f32 {
        match self.geometry {
            Geometry::Highlight(_) => (self.width * 3.5).max(14.0),
            _ => self.width,
        }
    }

    pub fn number_radius(&self) -> f32 {
        self.size * 0.75
    }

    /// 線分の集まりとして描く図形を分解する。それ以外は空。
    pub fn segments(&self) -> Vec<(Pos2, Pos2)> {
        match &self.geometry {
            Geometry::Pen(points) | Geometry::Highlight(points) => match points.as_slice() {
                [] => vec![],
                [p] => vec![(*p, *p)],
                _ => points.windows(2).map(|w| (w[0], w[1])).collect(),
            },
            Geometry::Line(a, b) => vec![(*a, *b)],
            Geometry::Rect(a, b) => {
                let r = rect_of(*a, *b);
                let (tl, br) = (r.min, r.max);
                let (tr, bl) = (pos2(br.x, tl.y), pos2(tl.x, br.y));
                vec![(tl, tr), (tr, br), (br, bl), (bl, tl)]
            }
            Geometry::Ellipse(a, b) => {
                let r = rect_of(*a, *b);
                let (c, rx, ry) = (r.center(), r.width() / 2.0, r.height() / 2.0);
                const N: usize = 96;
                let pt = |i: usize| {
                    let t = i as f32 / N as f32 * std::f32::consts::TAU;
                    pos2(c.x + rx * t.cos(), c.y + ry * t.sin())
                };
                (0..N).map(|i| (pt(i), pt(i + 1))).collect()
            }
            Geometry::Arrow(a, b) => {
                let mut segs = vec![(*a, *b)];
                segs.extend(self.arrow_head(*b, *a));
                segs
            }
            Geometry::DoubleArrow(a, b) => {
                let mut segs = vec![(*a, *b)];
                segs.extend(self.arrow_head(*b, *a));
                segs.extend(self.arrow_head(*a, *b));
                segs
            }
            Geometry::CurveArrow { a, b, ctrl } => {
                let pts = quad_points(*a, *ctrl, *b, 40);
                let mut segs: Vec<(Pos2, Pos2)> = pts.windows(2).map(|w| (w[0], w[1])).collect();
                // 矢じりは終点での接線の向き(制御点 → 終点)に合わせる
                segs.extend(self.arrow_head(*b, if (*b - *ctrl).length() > 1.0 { *ctrl } else { *a }));
                segs
            }
            Geometry::Bracket { a, b, style } => bracket_segments(rect_of(*a, *b), *style, self.width),
            _ => vec![],
        }
    }

    /// 矢じり(開いたV字)の2本の線分。`tip` が先端、`from` は矢の根元側の点(向きの基準)。
    fn arrow_head(&self, tip: Pos2, from: Pos2) -> Vec<(Pos2, Pos2)> {
        let dir = tip - from;
        if dir.length() <= 1.0 {
            return vec![];
        }
        let dir = dir.normalized();
        let head = (self.width * 4.0).max(14.0);
        [-1.0f32, 1.0]
            .iter()
            .map(|sign| {
                let (s, c) = (sign * 25f32.to_radians()).sin_cos();
                let rot = vec2(dir.x * c - dir.y * s, dir.x * s + dir.y * c);
                (tip, tip - rot * head)
            })
            .collect()
    }

    /// 曲線矢印の、曲線上の中点(ハンドルの位置)。
    pub fn curve_mid(&self) -> Option<Pos2> {
        let Geometry::CurveArrow { a, b, ctrl } = &self.geometry else { return None };
        Some(quad_points(*a, *ctrl, *b, 2)[1])
    }

    /// 吹き出し本体の最小の大きさ(文字が収まる大きさ)。
    pub fn bubble_min_size(&self) -> Vec2 {
        let Geometry::Bubble { text, style, .. } = &self.geometry else { return Vec2::ZERO };
        let sample = if text.is_empty() { "あああ" } else { text.as_str() };
        let pad = self.size * 0.6;
        let min = text::measure(sample, self.size) + vec2(pad * 2.0, pad * 1.4);
        if style.is_oval() { min * 1.4 } else { min }
    }

    pub fn bubble_parts(&self) -> Option<BubbleParts> {
        let Geometry::Bubble { center, tail, style, size, .. } = &self.geometry else { return None };
        let min = self.bubble_min_size();
        let dims = size.map_or(min, |s| vec2(s.x.max(min.x), s.y.max(min.y)));
        let rect = Rect::from_center_size(*center, dims);
        let radius = match style {
            BubbleStyle::Square => 0.0,
            BubbleStyle::Rounded => (rect.height() / 2.0).min(self.size * 0.9),
            BubbleStyle::Ellipse | BubbleStyle::Thought => rect.width().min(rect.height()) / 2.0,
        };
        let style = *style;
        if rect.expand(1.0).contains(*tail) {
            return Some(BubbleParts { rect, radius, style, tail: None });
        }
        // しっぽは、先端に最も近い辺から出す。根元は、角(楕円なら端の近く)を避けて辺の中ほどに置く
        let margin = |len: f32| if style.is_oval() { len * 0.25 } else { radius.min(len / 2.0) };
        let d = *tail - rect.center();
        let horizontal = d.x.abs() / (rect.width() / 2.0) > d.y.abs() / (rect.height() / 2.0);
        let half = (self.size * 0.5).min(if horizontal { rect.height() } else { rect.width() } / 4.0);
        let tail_pts = if horizontal {
            let x = if d.x > 0.0 { rect.max.x } else { rect.min.x };
            let m = margin(rect.height());
            let (lo, hi) = (rect.min.y + m + half, rect.max.y - m - half);
            let y = if lo < hi { tail.y.clamp(lo, hi) } else { rect.center().y };
            [pos2(x, y - half), pos2(x, y + half), *tail]
        } else {
            let y = if d.y > 0.0 { rect.max.y } else { rect.min.y };
            let m = margin(rect.width());
            let (lo, hi) = (rect.min.x + m + half, rect.max.x - m - half);
            let x = if lo < hi { tail.x.clamp(lo, hi) } else { rect.center().x };
            [pos2(x - half, y), pos2(x + half, y), *tail]
        };
        // 楕円は外接矩形の辺より内側に縁があるので、根元を縁の上に載せる(段差を作らない)
        let tail_pts = if style.is_oval() {
            let (c, r) = (rect.center(), rect.size() / 2.0);
            let on_edge = |p: Pos2| {
                if horizontal {
                    let k = ((p.y - c.y) / r.y).clamp(-0.98, 0.98);
                    let dx = r.x * (1.0 - k * k).sqrt() - 1.0;
                    pos2(c.x + dx * (p.x - c.x).signum(), p.y)
                } else {
                    let k = ((p.x - c.x) / r.x).clamp(-0.98, 0.98);
                    let dy = r.y * (1.0 - k * k).sqrt() - 1.0;
                    pos2(p.x, c.y + dy * (p.y - c.y).signum())
                }
            };
            [on_edge(tail_pts[0]), on_edge(tail_pts[1]), tail_pts[2]]
        } else {
            tail_pts
        };
        Some(BubbleParts { rect, radius, style, tail: Some(tail_pts) })
    }
    /// 画像座標での外接矩形(線の太さ込み)。
    pub fn bounds(&self) -> Rect {
        match &self.geometry {
            Geometry::Pen(p) | Geometry::Highlight(p) => bounds_of(p).expand(self.stroke_width() / 2.0 + 1.0),
            Geometry::Line(a, b) | Geometry::Rect(a, b) | Geometry::Ellipse(a, b) => {
                rect_of(*a, *b).expand(self.width / 2.0 + 1.0)
            }
            Geometry::Arrow(a, b) | Geometry::DoubleArrow(a, b) => {
                let head = (self.width * 4.0).max(14.0);
                rect_of(*a, *b).expand(self.width / 2.0 + head + 1.0)
            }
            Geometry::CurveArrow { .. } | Geometry::Bracket { .. } => {
                let pts: Vec<Pos2> = self.segments().iter().flat_map(|(a, b)| [*a, *b]).collect();
                bounds_of(&pts).expand(self.width / 2.0 + 1.0)
            }
            Geometry::Text { pos, text } => {
                let sample = if text.is_empty() { "あ" } else { text.as_str() };
                Rect::from_min_size(*pos, text::measure(sample, self.size))
            }
            Geometry::Bubble { center, .. } => match self.bubble_parts() {
                Some(parts) => {
                    let mut r = parts.rect;
                    if let Some(t) = parts.tail {
                        r = r.union(bounds_of(&t));
                    }
                    r.expand(self.width / 2.0 + 2.0)
                }
                None => Rect::from_center_size(*center, Vec2::ZERO),
            },
            Geometry::Number { center, style, .. } => {
                let ring = if style.is_outline() { self.width } else { 0.0 };
                Rect::from_center_size(*center, Vec2::splat(self.number_radius() * 2.0 + 2.0 + ring))
            }
            Geometry::Mosaic(a, b) => rect_of(*a, *b),
        }
    }

    /// 点 `p`(画像座標)が図形に当たっているか。`tol` は許容距離。
    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        let segs = self.segments();
        if !segs.is_empty() {
            let r = self.stroke_width() / 2.0 + tol;
            return segs.iter().any(|(a, b)| dist_to_segment(p, *a, *b) <= r);
        }
        self.bounds().expand(tol).contains(p)
    }

    pub fn translate(&mut self, d: Vec2) {
        let mv = |p: &mut Pos2| *p += d;
        match &mut self.geometry {
            Geometry::Pen(pts) | Geometry::Highlight(pts) => pts.iter_mut().for_each(mv),
            Geometry::Line(a, b)
            | Geometry::Arrow(a, b)
            | Geometry::DoubleArrow(a, b)
            | Geometry::Rect(a, b)
            | Geometry::Ellipse(a, b)
            | Geometry::Mosaic(a, b)
            | Geometry::Bracket { a, b, .. } => {
                mv(a);
                mv(b);
            }
            Geometry::CurveArrow { a, b, ctrl } => {
                mv(a);
                mv(b);
                mv(ctrl);
            }
            Geometry::Text { pos, .. } => mv(pos),
            Geometry::Bubble { center, tail, .. } => {
                mv(center);
                mv(tail);
            }
            Geometry::Number { center, .. } => mv(center),
        }
    }

    pub fn translated(&self, d: Vec2) -> Shape {
        let mut s = self.clone();
        s.translate(d);
        s
    }

    pub fn is_text_like(&self) -> bool {
        matches!(self.geometry, Geometry::Text { .. } | Geometry::Bubble { .. })
    }
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 <= f32::EPSILON { 0.0 } else { (((p - a).x * ab.x + (p - a).y * ab.y) / len2).clamp(0.0, 1.0) };
    (p - (a + ab * t)).length()
}

/// 2次ベジェ曲線 `a`→`b`(制御点 `c`)を `n` 分割した点列(両端を含む `n + 1` 点)。
pub fn quad_points(a: Pos2, c: Pos2, b: Pos2, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            pos2(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
        })
        .collect()
}

/// 3次ベジェ曲線の点列(両端を含む `n + 1` 点)。
fn cubic_points(p: [Pos2; 4], n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            pos2(
                w0 * p[0].x + w1 * p[1].x + w2 * p[2].x + w3 * p[3].x,
                w0 * p[0].y + w1 * p[1].y + w2 * p[2].y + w3 * p[3].y,
            )
        })
        .collect()
}

/// 括弧の線分。`r` の左右の辺に沿って、内側へ向けて開く括弧を1組描く。
fn bracket_segments(r: Rect, style: BracketStyle, width: f32) -> Vec<(Pos2, Pos2)> {
    if r.width() < 1.0 || r.height() < 1.0 {
        return vec![];
    }
    // 括弧の張り出し(奥行き)。細長い矩形でも、極端に大きくも小さくもならないようにする
    let depth = (r.height() * 0.12).clamp(width * 2.0 + 6.0, 40.0).min(r.width() / 2.0);
    let (x0, x1, y0, y1) = (r.min.x, r.max.x, r.min.y, r.max.y);
    let h = y1 - y0;

    // 左側の括弧を、外側 x = x0・内側 x = x0 + depth の座標で作り、右側は左右反転して作る
    let left: Vec<Pos2> = match style {
        BracketStyle::Square => {
            vec![pos2(x0 + depth, y0), pos2(x0, y0), pos2(x0, y1), pos2(x0 + depth, y1)]
        }
        BracketStyle::Round => (0..=24)
            .map(|i| {
                let s = i as f32 / 24.0;
                // 両端で内側、中央で外側へ膨らむ放物線
                pos2(x0 + depth * (2.0 * s - 1.0).powi(2), y0 + h * s)
            })
            .collect(),
        BracketStyle::Curly => {
            // 上半分(端 → 中央の尖り)を2本の3次ベジェでつなぎ、下半分は上下反転して作る
            let at = |u: f32, v: f32| pos2(x0 + depth * (1.0 - u), y0 + h * v);
            let mut top = cubic_points([at(0.0, 0.0), at(0.5, 0.0), at(0.5, 0.04), at(0.5, 0.14)], 8);
            top.extend(
                cubic_points([at(0.5, 0.14), at(0.5, 0.36), at(0.5, 0.44), at(1.0, 0.5)], 10).into_iter().skip(1),
            );
            let mut pts = top.clone();
            pts.extend(top.iter().rev().skip(1).map(|p| pos2(p.x, y0 + y1 - p.y)));
            pts
        }
    };
    let mirror = |p: &Pos2| pos2(x0 + x1 - p.x, p.y);
    let right: Vec<Pos2> = left.iter().map(mirror).collect();
    let mut segs: Vec<(Pos2, Pos2)> = left.windows(2).map(|w| (w[0], w[1])).collect();
    segs.extend(right.windows(2).map(|w| (w[0], w[1])));
    segs
}
