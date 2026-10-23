//! 編集画面: 図形・文字・吹き出し・番号・モザイク・トリミングなどの注釈編集。

use crate::raster;
use crate::shapes::{BubbleStyle, Geometry, Shape, Tool};
use egui::{
    Color32, ColorImage, Context, CursorIcon, Key, Modifiers, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions,
    Vec2, pos2, vec2,
};
use image::RgbaImage;
use std::sync::Arc;

pub enum Outcome {
    None,
    Finish,
    Copy,
    Discard,
}

const TOOLS: [(Tool, &str); 12] = [
    (Tool::Select, "選択"),
    (Tool::Pen, "ペン"),
    (Tool::Highlighter, "蛍光ペン"),
    (Tool::Line, "直線"),
    (Tool::Arrow, "矢印"),
    (Tool::Rect, "四角"),
    (Tool::Ellipse, "楕円"),
    (Tool::Text, "文字"),
    (Tool::Bubble, "吹き出し"),
    (Tool::Number, "番号"),
    (Tool::Mosaic, "モザイク"),
    (Tool::Crop, "切り抜き"),
];

const SWATCHES: [[u8; 3]; 7] =
    [[230, 40, 40], [255, 200, 0], [40, 170, 70], [30, 110, 230], [160, 60, 200], [20, 20, 20], [255, 255, 255]];

const MAX_UNDO: usize = 50;

pub fn hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "クリックで選択 / ドラッグで移動 / ダブルクリックで文字を編集 / Delete で削除",
        Tool::Pen => "ドラッグで自由に描く",
        Tool::Highlighter => "ドラッグで蛍光ペン(重ねても濃くなりません)",
        Tool::Line => "ドラッグで直線 / Shift で45度刻み",
        Tool::Arrow => "ドラッグで矢印 / Shift で45度刻み",
        Tool::Rect => "ドラッグで四角 / Shift で正方形",
        Tool::Ellipse => "ドラッグで楕円 / Shift で正円",
        Tool::Text => "クリックした位置に文字を入れる",
        Tool::Bubble => "しっぽの先から、吹き出しを置きたい位置へドラッグ",
        Tool::Number => "クリックで連番マークを置く",
        Tool::Mosaic => "ドラッグした範囲にモザイク(太さ=粗さ)",
        Tool::Crop => "ドラッグで範囲を決め、Enter で切り抜き",
    }
}

#[derive(Clone)]
struct Snap {
    base: Arc<RgbaImage>,
    shapes: Vec<Shape>,
}

/// 図形の変形用ハンドル。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    /// 線・矢印の始点と終点
    Start,
    End,
    /// 矩形系の8方向
    TL,
    T,
    TR,
    R,
    BR,
    B,
    BL,
    L,
    /// 吹き出しのしっぽの先
    Tail,
    /// 文字・番号・吹き出しの大きさ
    Size,
}

const HANDLE_PX: f32 = 9.0;

fn rect_handles(r: Rect) -> [(Handle, Pos2); 8] {
    let c = r.center();
    [
        (Handle::TL, r.min),
        (Handle::T, pos2(c.x, r.min.y)),
        (Handle::TR, pos2(r.max.x, r.min.y)),
        (Handle::R, pos2(r.max.x, c.y)),
        (Handle::BR, r.max),
        (Handle::B, pos2(c.x, r.max.y)),
        (Handle::BL, pos2(r.min.x, r.max.y)),
        (Handle::L, pos2(r.min.x, c.y)),
    ]
}

fn points_rect(pts: &[Pos2]) -> Rect {
    pts.iter().fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)))
}

/// 図形ごとの変形ハンドルの位置(画像座標)。
fn shape_handles(s: &Shape) -> Vec<(Handle, Pos2)> {
    match &s.geometry {
        Geometry::Line(a, b) | Geometry::Arrow(a, b) => vec![(Handle::Start, *a), (Handle::End, *b)],
        Geometry::Rect(a, b) | Geometry::Ellipse(a, b) | Geometry::Mosaic(a, b) => {
            rect_handles(Rect::from_two_pos(*a, *b)).to_vec()
        }
        Geometry::Pen(p) | Geometry::Highlight(p) => rect_handles(points_rect(p)).to_vec(),
        Geometry::Text { .. } => vec![(Handle::Size, s.bounds().max)],
        Geometry::Bubble { .. } => {
            // 本体の8方向(広げる/縮める)、しっぽの先、上の菱形(文字の大きさ)
            let Some(parts) = s.bubble_parts() else { return vec![] };
            let mut v = rect_handles(parts.rect).to_vec();
            v.push((Handle::Size, bubble_size_handle(parts.rect)));
            if let Some(t) = parts.tail {
                v.push((Handle::Tail, t[2]));
            }
            v
        }
        Geometry::Number { center, .. } => vec![(Handle::Size, *center + vec2(s.number_radius(), 0.0))],
    }
}

/// 吹き出しの「文字の大きさ」ハンドルの位置(本体の上端の少し上)。
fn bubble_size_handle(rect: Rect) -> Pos2 {
    pos2(rect.center().x, rect.min.y - 18.0)
}

fn resize_rect(r: Rect, h: Handle, p: Pos2) -> Rect {
    let (mut l, mut t, mut rt, mut b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    match h {
        Handle::TL => (l, t) = (p.x, p.y),
        Handle::T => t = p.y,
        Handle::TR => (rt, t) = (p.x, p.y),
        Handle::R => rt = p.x,
        Handle::BR => (rt, b) = (p.x, p.y),
        Handle::B => b = p.y,
        Handle::BL => (l, b) = (p.x, p.y),
        Handle::L => l = p.x,
        _ => {}
    }
    Rect::from_two_pos(pos2(l, t), pos2(rt, b))
}

/// ハンドル `h` を `p` までドラッグしたときの図形(`orig` は掴んだ時点の図形)。
fn apply_handle(orig: &Shape, h: Handle, p: Pos2, shift: bool) -> Shape {
    let mut s = orig.clone();
    match (&mut s.geometry, h) {
        (Geometry::Line(a, b) | Geometry::Arrow(a, b), Handle::Start) => *a = if shift { snap_angle(*b, p) } else { p },
        (Geometry::Line(a, b) | Geometry::Arrow(a, b), Handle::End) => *b = if shift { snap_angle(*a, p) } else { p },
        (Geometry::Rect(a, b) | Geometry::Ellipse(a, b) | Geometry::Mosaic(a, b), _) => {
            let old = Rect::from_two_pos(*a, *b);
            let new = resize_rect(old, h, p);
            (*a, *b) = (new.min, new.max);
        }
        (Geometry::Pen(pts) | Geometry::Highlight(pts), _) => {
            let old = points_rect(pts);
            let new = resize_rect(old, h, p);
            let fx = if old.width() > 0.5 { new.width() / old.width() } else { 1.0 };
            let fy = if old.height() > 0.5 { new.height() / old.height() } else { 1.0 };
            for q in pts.iter_mut() {
                *q = pos2(new.min.x + (q.x - old.min.x) * fx, new.min.y + (q.y - old.min.y) * fy);
            }
        }
        (Geometry::Bubble { tail, .. }, Handle::Tail) => *tail = p,
        (Geometry::Text { pos, .. }, Handle::Size) => {
            let old_h = orig.bounds().height().max(1.0);
            s.size = (orig.size * (p.y - pos.y) / old_h).clamp(8.0, 300.0);
        }
        (Geometry::Bubble { center, .. }, Handle::Size) => {
            let grip = orig.bubble_parts().map_or(*center, |b| bubble_size_handle(b.rect));
            let base = (grip - *center).length().max(1.0);
            s.size = (orig.size * (p - *center).length() / base).clamp(8.0, 300.0);
        }
        (Geometry::Bubble { center, size, .. }, _) => {
            // 本体の矩形をドラッグで広げ縮めする(文字が収まる最小の大きさより小さくはならない)
            let old = orig.bubble_parts().map_or(Rect::from_center_size(*center, Vec2::ZERO), |b| b.rect);
            let new = resize_rect(old, h, p);
            let min = orig.bubble_min_size();
            let dims = vec2(new.width().max(min.x), new.height().max(min.y));
            *size = Some(dims);
            // 動かしていない側の辺を固定するため、中心を取り直す
            let anchor_x = match h {
                Handle::TL | Handle::L | Handle::BL => old.max.x - dims.x / 2.0,
                Handle::TR | Handle::R | Handle::BR => old.min.x + dims.x / 2.0,
                _ => old.center().x,
            };
            let anchor_y = match h {
                Handle::TL | Handle::T | Handle::TR => old.max.y - dims.y / 2.0,
                Handle::BL | Handle::B | Handle::BR => old.min.y + dims.y / 2.0,
                _ => old.center().y,
            };
            *center = pos2(anchor_x, anchor_y);
        }
        (Geometry::Number { center, .. }, Handle::Size) => {
            s.size = ((p.x - center.x).abs() / 0.75).clamp(8.0, 300.0);
        }
        _ => {}
    }
    s
}

struct Moving {
    index: usize,
    orig: Shape,
    shape: Shape,
    grab: Pos2,
    /// `None` なら図形全体の移動
    handle: Option<Handle>,
}

struct Pending {
    shape: Shape,
    /// 既存の図形を編集中なら、その元の位置
    original: Option<usize>,
}

pub struct Editor {
    base: Arc<RgbaImage>,
    shapes: Vec<Shape>,
    /// 元画像に確定済みの注釈をすべて重ねた画像(表示・書き出しの元)
    comp: RgbaImage,
    tex: TextureHandle,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    tool: Tool,
    color: [u8; 3],
    width: f32,
    font_size: f32,
    bubble_style: BubbleStyle,
    zoom: f32,
    pub status: String,
    current: Option<Shape>,
    moving: Option<Moving>,
    selected: Option<usize>,
    pending: Option<Pending>,
    focus_text: bool,
    crop: Option<Rect>,
    crop_anchor: Option<Pos2>,
    /// 切り抜き枠のハンドルを掴んでいる間: (掴んだハンドル, 掴んだ時点の枠)
    crop_handle: Option<(Handle, Rect)>,
    preview_tex: Option<TextureHandle>,
    origin: Pos2,
    avail: Vec2,
}

impl Editor {
    pub fn new(ctx: &Context, base: RgbaImage, zoom: f32) -> Self {
        let tex = ctx.load_texture("edit", to_color_image(&base), TextureOptions::LINEAR);
        Self {
            comp: base.clone(),
            base: Arc::new(base),
            shapes: vec![],
            tex,
            undo: vec![],
            redo: vec![],
            tool: Tool::Pen,
            color: SWATCHES[0],
            width: 4.0,
            font_size: 28.0,
            bubble_style: BubbleStyle::default(),
            zoom,
            status: format!("Enter: 保存してコピー  /  Ctrl+C: コピーのみ  /  Esc: 破棄  |  {}", hint(Tool::Pen)),
            current: None,
            moving: None,
            selected: None,
            pending: None,
            focus_text: false,
            crop: None,
            crop_anchor: None,
            crop_handle: None,
            preview_tex: None,
            origin: Pos2::ZERO,
            avail: Vec2::ZERO,
        }
    }

    /// 注釈を焼き込んだ書き出し用の画像。
    pub fn final_image(&self) -> RgbaImage {
        self.comp.clone()
    }

    fn color32(&self) -> Color32 {
        Color32::from_rgb(self.color[0], self.color[1], self.color[2])
    }

    fn snap(&self) -> Snap {
        Snap { base: self.base.clone(), shapes: self.shapes.clone() }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.snap());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn apply_snap(&mut self, s: Snap) {
        self.base = s.base;
        self.shapes = s.shapes;
        self.selected = None;
        self.rebuild();
    }

    fn undo_step(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snap());
            self.apply_snap(s);
        }
    }

    fn redo_step(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snap());
            self.apply_snap(s);
        }
    }

    fn rebuild(&mut self) {
        self.comp = raster::render(&self.base, &self.shapes);
        self.upload();
    }

    fn upload(&mut self) {
        self.tex.set(to_color_image(&self.comp), TextureOptions::LINEAR);
    }

    /// 図形を確定する(元に戻せる)。
    fn commit(&mut self, shape: Shape) {
        self.push_undo();
        raster::draw_shape(&mut self.comp, &shape);
        self.shapes.push(shape);
        // 描いた直後の図形にハンドルを出し、そのまま形を直せるようにする
        self.selected = Some(self.shapes.len() - 1);
        self.upload();
    }

    fn new_shape(&self, geometry: Geometry) -> Shape {
        Shape { geometry, color: self.color32(), width: self.width, size: self.font_size }
    }

    fn start_geometry(&self, p: Pos2) -> Option<Geometry> {
        Some(match self.tool {
            Tool::Pen => Geometry::Pen(vec![p]),
            Tool::Highlighter => Geometry::Highlight(vec![p]),
            Tool::Line => Geometry::Line(p, p),
            Tool::Arrow => Geometry::Arrow(p, p),
            Tool::Rect => Geometry::Rect(p, p),
            Tool::Ellipse => Geometry::Ellipse(p, p),
            Tool::Mosaic => Geometry::Mosaic(p, p),
            Tool::Bubble => {
                Geometry::Bubble { center: p, text: String::new(), tail: p, style: self.bubble_style, size: None }
            }
            _ => return None,
        })
    }

    fn to_img(&self, p: Pos2) -> Pos2 {
        let v = (p - self.origin) / self.zoom;
        pos2(v.x.clamp(0.0, self.base.width() as f32), v.y.clamp(0.0, self.base.height() as f32))
    }

    fn hit_test(&self, p: Pos2) -> Option<usize> {
        let tol = 6.0 / self.zoom;
        self.shapes.iter().rposition(|s| s.hit(p, tol))
    }

    // ---- 文字入力 -------------------------------------------------------

    fn begin_edit(&mut self, idx: usize) {
        self.push_undo();
        let shape = self.shapes.remove(idx);
        self.rebuild();
        self.pending = Some(Pending { shape, original: Some(idx) });
        self.focus_text = true;
        self.selected = None;
    }

    fn confirm_pending(&mut self) {
        let Some(p) = self.pending.take() else { return };
        let empty = matches!(&p.shape.geometry,
            Geometry::Text { text, .. } | Geometry::Bubble { text, .. } if text.trim().is_empty());
        match p.original {
            // 既存の図形の編集: 空にしたら削除(元に戻せる)
            Some(i) if !empty => {
                self.shapes.insert(i.min(self.shapes.len()), p.shape);
                self.rebuild();
            }
            Some(_) => {}
            None if !empty => self.commit(p.shape),
            None => {}
        }
    }

    fn cancel_pending(&mut self) {
        let Some(p) = self.pending.take() else { return };
        if p.original.is_some()
            && let Some(s) = self.undo.pop()
        {
            self.apply_snap(s);
        }
    }

    // ---- 切り抜き -------------------------------------------------------

    fn apply_crop(&mut self) {
        let Some(r) = self.crop.take() else { return };
        let (bw, bh) = (self.base.width() as f32, self.base.height() as f32);
        let x = r.min.x.round().clamp(0.0, bw - 1.0) as u32;
        let y = r.min.y.round().clamp(0.0, bh - 1.0) as u32;
        let w = (r.width().round() as u32).clamp(1, self.base.width() - x);
        let h = (r.height().round() as u32).clamp(1, self.base.height() - y);
        if w < 2 || h < 2 {
            return;
        }
        self.push_undo();
        self.base = Arc::new(image::imageops::crop_imm(&*self.base, x, y, w, h).to_image());
        let d = vec2(-(x as f32), -(y as f32));
        self.shapes.iter_mut().for_each(|s| s.translate(d));
        self.selected = None;
        self.rebuild();
        self.status = format!("{w} x {h} に切り抜きました(Ctrl+Z で戻せます)");
    }

    // ---- UI ------------------------------------------------------------

    pub fn ui(&mut self, root: &mut egui::Ui) -> Outcome {
        let ctx = root.ctx().clone();
        let mut out = Outcome::None;
        let typing = self.pending.is_some();

        if !typing {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                if self.crop.is_some() {
                    self.apply_crop();
                } else {
                    out = Outcome::Finish;
                }
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                if self.crop.is_some() {
                    self.crop = None;
                } else if self.selected.is_some() {
                    self.selected = None;
                } else {
                    out = Outcome::Discard;
                }
            }
            if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                out = Outcome::Copy;
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                || ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Y))
            {
                self.redo_step();
            } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
                self.undo_step();
            }
            let del = ctx.input_mut(|i| {
                i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)
            });
            if del
                && let Some(i) = self.selected.take()
                && i < self.shapes.len()
            {
                self.push_undo();
                self.shapes.remove(i);
                self.rebuild();
            }
        }

        self.toolbar(root, &ctx, &mut out);

        egui::Panel::bottom("status").exact_size(28.0).show(root, |ui| {
            ui.horizontal_centered(|ui| ui.label(&self.status));
        });

        egui::CentralPanel::default().show(root, |ui| self.canvas(ui, &ctx));

        self.text_popup(&ctx);
        out
    }

    fn toolbar(&mut self, root: &mut egui::Ui, ctx: &Context, out: &mut Outcome) {
        egui::Panel::top("toolbar").resizable(false).show(root, |ui| {
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                for (tool, label) in TOOLS {
                    if ui.selectable_label(self.tool == tool, label).clicked() && self.tool != tool {
                        self.tool = tool;
                        self.current = None;
                        self.crop = None;
                        self.status = hint(tool).to_string();
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                let before = self.color;
                for c in SWATCHES {
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                    ui.painter().rect_stroke(
                        r,
                        3.0,
                        Stroke::new(1.0, Color32::from_gray(120)),
                        egui::StrokeKind::Inside,
                    );
                    if self.color == c {
                        ui.painter().rect_stroke(
                            r.expand(2.0),
                            3.0,
                            Stroke::new(2.0, Color32::from_rgb(0, 160, 255)),
                            egui::StrokeKind::Outside,
                        );
                    }
                    if resp.clicked() {
                        self.color = c;
                    }
                }
                ui.color_edit_button_srgb(&mut self.color);
                // 色の変更が既存の図形に及ぶのは選択ツールのときだけ(描画ツールでは次の図形の色)
                if self.color != before && self.tool == Tool::Select {
                    self.recolor_selected();
                }
                ui.separator();
                let width_label = if self.tool == Tool::Mosaic { "粗さ" } else { "太さ" };
                ui.add(egui::Slider::new(&mut self.width, 1.0..=24.0).text(width_label));
                if matches!(self.tool, Tool::Text | Tool::Bubble | Tool::Number) {
                    ui.add(egui::Slider::new(&mut self.font_size, 10.0..=120.0).text("文字"));
                }
                self.bubble_style_ui(ui);
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("↶ 戻す")).clicked() {
                    self.undo_step();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↷ やり直す")).clicked() {
                    self.redo_step();
                }
                ui.separator();
                if ui
                    .button(format!("{:.0}%", self.zoom * ctx.pixels_per_point() * 100.0))
                    .on_hover_text("等倍に戻す")
                    .clicked()
                {
                    self.zoom = 1.0 / ctx.pixels_per_point();
                }
                if ui.button("全体表示").clicked() && self.avail.x > 0.0 {
                    let (w, h) = (self.base.width() as f32, self.base.height() as f32);
                    self.zoom = (self.avail.x / w).min(self.avail.y / h).clamp(0.1, 8.0);
                }
                if self.crop.is_some() && ui.button("切り抜く (Enter)").clicked() {
                    self.apply_crop();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("完了").clicked() {
                        self.confirm_pending();
                        *out = Outcome::Finish;
                    }
                    if ui.button("コピー").clicked() {
                        self.confirm_pending();
                        *out = Outcome::Copy;
                    }
                    if ui.button("破棄").clicked() {
                        *out = Outcome::Discard;
                    }
                });
            });
            ui.add_space(3.0);
        });
    }

    /// 吹き出しの輪郭(角丸・楕円・四角・もくもく)を選ぶ。吹き出しツール中か、吹き出しを選択中に出す。
    fn bubble_style_ui(&mut self, ui: &mut egui::Ui) {
        let selected = self
            .selected
            .filter(|&i| i < self.shapes.len() && matches!(self.shapes[i].geometry, Geometry::Bubble { .. }));
        if self.tool != Tool::Bubble && selected.is_none() {
            return;
        }
        let current = match selected.map(|i| &self.shapes[i].geometry) {
            Some(Geometry::Bubble { style, .. }) => *style,
            _ => self.bubble_style,
        };
        let mut style = current;
        let label = BubbleStyle::ALL.iter().find(|(s, _)| *s == style).map_or("", |(_, l)| *l);
        egui::ComboBox::from_id_salt("bubble_style").selected_text(format!("吹き出し: {label}")).show_ui(ui, |ui| {
            for (s, l) in BubbleStyle::ALL {
                ui.selectable_value(&mut style, s, l);
            }
        });
        if style != current {
            self.bubble_style = style;
            if let Some(i) = selected {
                self.push_undo();
                if let Geometry::Bubble { style: st, .. } = &mut self.shapes[i].geometry {
                    *st = style;
                }
                self.rebuild();
            }
        }
    }

    /// 選択中の図形の色を、いま選んでいる色に変える。
    fn recolor_selected(&mut self) {
        let Some(i) = self.selected else { return };
        if i >= self.shapes.len() {
            return;
        }
        let color = self.color32();
        if self.shapes[i].color != color {
            self.push_undo();
            self.shapes[i].color = color;
            self.rebuild();
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui, ctx: &Context) {
        let zd = ui.input(|i| i.zoom_delta());
        if zd != 1.0 {
            self.zoom = (self.zoom * zd).clamp(0.1, 8.0);
        }
        self.avail = ui.available_size();
        egui::ScrollArea::both().show(ui, |ui| {
            let size = vec2(self.base.width() as f32, self.base.height() as f32);
            let (resp, painter) = ui.allocate_painter(size * self.zoom, Sense::click_and_drag());
            self.origin = resp.rect.min;
            painter.image(self.tex.id(), resp.rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            if self.pending.is_none() {
                self.handle_input(&resp, ctx);
            }
            self.draw_preview(&painter, ctx);
            self.draw_overlays(&painter);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(match self.tool {
                    Tool::Select => CursorIcon::Default,
                    Tool::Text => CursorIcon::Text,
                    _ => CursorIcon::Crosshair,
                });
            }
        });
    }

    fn handle_input(&mut self, resp: &egui::Response, ctx: &Context) {
        let pos = resp.interact_pointer_pos().or(resp.hover_pos()).map(|p| self.to_img(p));
        let shift = ctx.input(|i| i.modifiers.shift);
        let press = ctx.input(|i| i.pointer.press_origin()).map(|p| self.to_img(p));

        // 選択中の図形のハンドルは、どのツールでも掴んで変形できる
        if self.moving.is_none()
            && resp.drag_started()
            && let Some(p) = press
            && let Some((i, h)) = self.handle_at(p)
        {
            self.begin_move(i, Some(h), p);
        }
        if self.drive_moving(resp, pos, shift) {
            return;
        }

        match self.tool {
            Tool::Select => self.input_select(resp, pos, press),
            Tool::Text => {
                if let (true, Some(p)) = (resp.clicked(), pos) {
                    let shape = self.new_shape(Geometry::Text { pos: p, text: String::new() });
                    self.pending = Some(Pending { shape, original: None });
                    self.focus_text = true;
                }
            }
            Tool::Number => {
                if let (true, Some(p)) = (resp.clicked(), pos) {
                    let n = self
                        .shapes
                        .iter()
                        .filter_map(|s| if let Geometry::Number { n, .. } = s.geometry { Some(n) } else { None })
                        .max()
                        .unwrap_or(0)
                        + 1;
                    let shape = self.new_shape(Geometry::Number { center: p, n });
                    self.commit(shape);
                }
            }
            Tool::Crop => {
                if resp.drag_started() {
                    // 既存の切り抜き枠のハンドルを掴んだら、枠を変形する
                    let tol = HANDLE_PX * 1.3 / self.zoom;
                    self.crop_handle = self.crop.zip(press).and_then(|(c, p)| {
                        rect_handles(c).into_iter().find(|(_, hp)| (*hp - p).length() <= tol).map(|(h, _)| (h, c))
                    });
                    if self.crop_handle.is_none() {
                        self.crop_anchor = press;
                        self.crop = None;
                    }
                }
                if resp.dragged() || resp.drag_stopped() {
                    if let (Some((h, orig)), Some(p)) = (self.crop_handle, pos) {
                        self.crop = Some(resize_rect(orig, h, p));
                    } else if let (Some(a), Some(p)) = (self.crop_anchor, pos) {
                        self.crop = Some(Rect::from_two_pos(a, p));
                    }
                }
                if resp.drag_stopped() {
                    self.crop_anchor = None;
                    self.crop_handle = None;
                    if self.crop.is_some_and(|r| r.width() < 4.0 || r.height() < 4.0) {
                        self.crop = None;
                    }
                }
            }
            _ => self.input_draw(resp, pos, shift),
        }
    }

    fn input_draw(&mut self, resp: &egui::Response, pos: Option<Pos2>, shift: bool) {
        if resp.drag_started()
            && let Some(p) = pos
            && let Some(g) = self.start_geometry(p)
        {
            self.current = Some(self.new_shape(g));
        }
        if (resp.dragged() || resp.drag_stopped())
            && let (Some(cur), Some(p)) = (self.current.as_mut(), pos)
        {
            update_geometry(&mut cur.geometry, p, shift);
        }
        if resp.drag_stopped() {
            let Some(shape) = self.current.take() else { return };
            match &shape.geometry {
                Geometry::Bubble { .. } => {
                    self.pending = Some(Pending { shape, original: None });
                    self.focus_text = true;
                }
                Geometry::Line(a, b)
                | Geometry::Arrow(a, b)
                | Geometry::Rect(a, b)
                | Geometry::Ellipse(a, b)
                | Geometry::Mosaic(a, b) => {
                    let r = Rect::from_two_pos(*a, *b);
                    if r.width() >= 3.0 || r.height() >= 3.0 {
                        self.commit(shape);
                    }
                }
                _ => self.commit(shape),
            }
        }
    }

    /// 選択中の図形のハンドルのうち、`p` に最も近いものを返す。
    fn handle_at(&self, p: Pos2) -> Option<(usize, Handle)> {
        let i = self.selected.filter(|&i| i < self.shapes.len())?;
        let tol = HANDLE_PX * 1.3 / self.zoom;
        shape_handles(&self.shapes[i])
            .into_iter()
            .map(|(h, hp)| (h, (hp - p).length()))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| (i, h))
    }

    /// 図形を掴んで移動・変形を始める。掴んでいる間は一覧から外して描画から除く。
    fn begin_move(&mut self, i: usize, handle: Option<Handle>, grab: Pos2) {
        self.push_undo();
        let orig = self.shapes.remove(i);
        self.rebuild();
        self.moving = Some(Moving { index: i, shape: orig.clone(), orig, grab, handle });
        self.selected = Some(i);
    }

    /// 移動・変形中の図形を更新し、離したら確定する。処理中なら true。
    fn drive_moving(&mut self, resp: &egui::Response, pos: Option<Pos2>, shift: bool) -> bool {
        if self.moving.is_none() {
            return false;
        }
        // 離した瞬間のフレームでは dragged() が偽になるので、最終位置もここで反映する
        if (resp.dragged() || resp.drag_stopped())
            && let (Some(m), Some(p)) = (self.moving.as_mut(), pos)
        {
            m.shape = match m.handle {
                Some(h) => apply_handle(&m.orig, h, p, shift),
                None => m.orig.translated(p - m.grab),
            };
        }
        if resp.drag_stopped()
            && let Some(m) = self.moving.take()
        {
            let i = m.index.min(self.shapes.len());
            self.shapes.insert(i, m.shape);
            self.rebuild();
            self.selected = Some(i);
        }
        true
    }

    fn input_select(&mut self, resp: &egui::Response, pos: Option<Pos2>, press: Option<Pos2>) {
        if resp.double_clicked() {
            if let Some(i) = pos.and_then(|p| self.hit_test(p))
                && self.shapes[i].is_text_like()
            {
                self.begin_edit(i);
            }
            return;
        }
        if resp.clicked() {
            self.selected = pos.and_then(|p| self.hit_test(p));
        }
        if resp.drag_started() {
            let Some(p) = press else { return };
            match self.hit_test(p) {
                Some(i) => self.begin_move(i, None, p),
                None => self.selected = None,
            }
        }
    }
    /// 作図中・移動中・入力中の図形を、確定済みの画像の上に重ねて見せる。
    fn draw_preview(&mut self, painter: &egui::Painter, ctx: &Context) {
        let shape = self
            .moving
            .as_ref()
            .map(|m| &m.shape)
            .or(self.pending.as_ref().map(|p| &p.shape))
            .or(self.current.as_ref());
        let Some(shape) = shape else {
            self.preview_tex = None;
            return;
        };
        let (iw, ih) = (self.comp.width() as i32, self.comp.height() as i32);
        let b = shape.bounds();
        let x0 = (b.min.x.floor() as i32).clamp(0, iw);
        let y0 = (b.min.y.floor() as i32).clamp(0, ih);
        let x1 = (b.max.x.ceil() as i32).clamp(0, iw);
        let y1 = (b.max.y.ceil() as i32).clamp(0, ih);
        if x1 - x0 < 1 || y1 - y0 < 1 {
            return;
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut crop = image::imageops::crop_imm(&self.comp, x0 as u32, y0 as u32, w, h).to_image();
        raster::draw_shape(&mut crop, &shape.translated(vec2(-(x0 as f32), -(y0 as f32))));
        let tex = ctx.load_texture("preview", to_color_image(&crop), TextureOptions::LINEAR);
        let rect = Rect::from_min_size(
            self.origin + vec2(x0 as f32, y0 as f32) * self.zoom,
            vec2(w as f32, h as f32) * self.zoom,
        );
        painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        self.preview_tex = Some(tex);
    }

    fn screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    /// ハンドルの印。しっぽは丸、大きさは菱形、それ以外は四角。
    fn draw_handle(&self, painter: &egui::Painter, h: Handle, p: Pos2) {
        let blue = Color32::from_rgb(0, 160, 255);
        let c = self.screen(p);
        let r = HANDLE_PX / 2.0;
        match h {
            Handle::Tail => {
                painter.circle_filled(c, r + 1.0, blue);
                painter.circle_stroke(c, r + 1.0, Stroke::new(2.0, Color32::WHITE));
            }
            Handle::Size => {
                let pts = vec![
                    c + vec2(0.0, -r - 2.0),
                    c + vec2(r + 2.0, 0.0),
                    c + vec2(0.0, r + 2.0),
                    c + vec2(-r - 2.0, 0.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    Color32::from_rgb(255, 200, 0),
                    Stroke::new(1.5, Color32::BLACK),
                ));
            }
            _ => {
                let rect = Rect::from_center_size(c, Vec2::splat(HANDLE_PX));
                painter.rect_filled(rect, 1.0, Color32::WHITE);
                painter.rect_stroke(rect, 1.0, Stroke::new(1.5, blue), egui::StrokeKind::Outside);
            }
        }
    }

    fn draw_overlays(&self, painter: &egui::Painter) {
        let blue = Color32::from_rgb(0, 160, 255);
        // 選択中の図形の枠
        let selected =
            self.moving.as_ref().map(|m| &m.shape).or_else(|| self.selected.and_then(|i| self.shapes.get(i)));
        if let Some(s) = selected {
            let b = s.bounds();
            let r = Rect::from_min_max(self.screen(b.min), self.screen(b.max));
            painter.rect_stroke(
                r.expand(2.0),
                0.0,
                Stroke::new(3.0, Color32::from_black_alpha(140)),
                egui::StrokeKind::Outside,
            );
            painter.rect_stroke(r.expand(2.0), 0.0, Stroke::new(1.5, blue), egui::StrokeKind::Outside);
            for (h, p) in shape_handles(s) {
                self.draw_handle(painter, h, p);
            }
        }
        // 切り抜き範囲(外側を暗くする)
        if let Some(c) = self.crop {
            let full = Rect::from_min_max(
                self.screen(Pos2::ZERO),
                self.screen(pos2(self.base.width() as f32, self.base.height() as f32)),
            );
            let r = Rect::from_min_max(self.screen(c.min), self.screen(c.max));
            let dim = Color32::from_black_alpha(150);
            painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, r.min.y)), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.max.y), full.max), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.min.y), pos2(r.min.x, r.max.y)), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(full.max.x, r.max.y)), 0.0, dim);
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, blue), egui::StrokeKind::Outside);
            for (h, p) in rect_handles(c) {
                self.draw_handle(painter, h, p);
            }
            painter.text(
                r.min + vec2(0.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{} x {}", c.width().round(), c.height().round()),
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }

    /// 文字入力ポップアップ(文字・吹き出し)。
    fn text_popup(&mut self, ctx: &Context) {
        let Some(p) = &self.pending else { return };
        let b = p.shape.bounds();
        let at = self.screen(pos2(b.min.x, b.max.y)) + vec2(0.0, 8.0);
        let enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let (mut ok, mut cancel) = (enter, esc);
        let focus = std::mem::take(&mut self.focus_text);
        if let Some(p) = &mut self.pending {
            egui::Area::new(egui::Id::NULL.with("text_popup")).order(egui::Order::Foreground).fixed_pos(at).show(
                ctx,
                |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        if let Geometry::Text { text, .. } | Geometry::Bubble { text, .. } = &mut p.shape.geometry {
                            let r = ui.add(
                                egui::TextEdit::multiline(text)
                                    .desired_rows(2)
                                    .desired_width(280.0)
                                    .hint_text("文字を入力  (Enter: 確定 / Shift+Enter: 改行)"),
                            );
                            if focus {
                                r.request_focus();
                            }
                        }
                        ui.horizontal(|ui| {
                            ok |= ui.button("確定").clicked();
                            cancel |= ui.button("取消").clicked();
                        });
                    });
                },
            );
        }
        if cancel {
            self.cancel_pending();
        } else if ok {
            self.confirm_pending();
        }
    }
}

fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// ドラッグ中の点 `p` に合わせて図形を更新する。
fn update_geometry(g: &mut Geometry, p: Pos2, shift: bool) {
    match g {
        Geometry::Pen(pts) | Geometry::Highlight(pts) => {
            if pts.last().is_none_or(|l| (*l - p).length() > 0.5) {
                pts.push(p);
            }
        }
        Geometry::Line(a, b) | Geometry::Arrow(a, b) => *b = if shift { snap_angle(*a, p) } else { p },
        Geometry::Rect(a, b) | Geometry::Ellipse(a, b) => {
            *b = if shift {
                let d = p - *a;
                let m = d.x.abs().max(d.y.abs());
                pos2(a.x + m * d.x.signum(), a.y + m * d.y.signum())
            } else {
                p
            }
        }
        Geometry::Mosaic(_, b) => *b = p,
        Geometry::Bubble { center, .. } => *center = p,
        _ => {}
    }
}

/// `from` から見た `to` の向きを45度刻みに丸める。
fn snap_angle(from: Pos2, to: Pos2) -> Pos2 {
    let d = to - from;
    let len = d.length();
    if len < 1.0 {
        return to;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (d.y.atan2(d.x) / step).round() * step;
    from + vec2(ang.cos(), ang.sin()) * len
}
