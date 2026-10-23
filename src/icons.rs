//! ツールパレット用のアイコン。画像ファイルは使わず、egui の図形でその場で描く。

use crate::shapes::{Tool, quad_points};
use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

/// `r` の中に、`tool` のアイコンを `col` 色で描く。アイコンは 24×24 の座標系で設計してある。
pub fn paint(painter: &Painter, r: Rect, tool: Tool, col: Color32) {
    let s = r.width().min(r.height()) / 24.0;
    let origin = r.center() - Vec2::splat(12.0 * s);
    let p = |x: f32, y: f32| -> Pos2 { origin + vec2(x, y) * s };
    let stroke = Stroke::new(2.0 * s, col);
    let line = |pts: &[(f32, f32)]| {
        painter.add(egui::Shape::line(pts.iter().map(|&(x, y)| p(x, y)).collect(), stroke));
    };
    // 先端 (tx, ty) に向かう矢じり。(fx, fy) は矢の根元側の点
    let head = |tx: f32, ty: f32, fx: f32, fy: f32| {
        let d = vec2(tx - fx, ty - fy);
        let d = if d.length() > 0.0 { d.normalized() } else { vec2(1.0, 0.0) };
        for sign in [-1.0f32, 1.0] {
            let (sn, cs) = (sign * 28f32.to_radians()).sin_cos();
            let rot = vec2(d.x * cs - d.y * sn, d.x * sn + d.y * cs);
            let end = vec2(tx, ty) - rot * 6.5;
            line(&[(tx, ty), (end.x, end.y)]);
        }
    };

    match tool {
        Tool::Select => {
            // マウスカーソル(矢印)
            let pts = [(6.0, 3.0), (6.0, 19.0), (10.0, 15.0), (13.0, 21.5), (15.5, 20.3), (12.5, 14.0), (18.0, 14.0)];
            painter.add(egui::Shape::convex_polygon(
                pts.iter().take(3).map(|&(x, y)| p(x, y)).collect(),
                col,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                [pts[2], pts[3], pts[4], pts[5]].iter().map(|&(x, y)| p(x, y)).collect(),
                col,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                [pts[0], pts[2], pts[5], pts[6]].iter().map(|&(x, y)| p(x, y)).collect(),
                col,
                Stroke::NONE,
            ));
        }
        Tool::Pen => {
            // 鉛筆: 斜めの軸と芯先
            line(&[(18.0, 4.0), (6.0, 16.0)]);
            painter.add(egui::Shape::convex_polygon(vec![p(6.0, 16.0), p(4.0, 20.5), p(8.5, 18.5)], col, Stroke::NONE));
        }
        Tool::Highlighter => {
            // 蛍光ペン: 太い半透明の線とペン先
            let hi = Color32::from_rgba_unmultiplied(255, 200, 0, 190);
            painter.add(egui::Shape::line(vec![p(6.0, 17.0), p(17.0, 8.0)], Stroke::new(8.0 * s, hi)));
            line(&[(5.0, 21.0), (19.0, 21.0)]);
        }
        Tool::Line => line(&[(4.0, 19.0), (20.0, 5.0)]),
        Tool::Arrow => {
            line(&[(4.0, 20.0), (19.0, 5.0)]);
            head(19.0, 5.0, 4.0, 20.0);
        }
        Tool::DoubleArrow => {
            line(&[(5.0, 19.0), (19.0, 5.0)]);
            head(19.0, 5.0, 5.0, 19.0);
            head(5.0, 19.0, 19.0, 5.0);
        }
        Tool::CurveArrow => {
            let pts: Vec<(f32, f32)> =
                quad_points(pos2(4.0, 19.0), pos2(5.0, 6.0), pos2(19.0, 6.0), 16).iter().map(|q| (q.x, q.y)).collect();
            line(&pts);
            head(19.0, 6.0, 8.0, 6.0);
        }
        Tool::Rect => {
            painter.rect_stroke(
                Rect::from_min_max(p(4.0, 6.0), p(20.0, 18.0)),
                1.5 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Tool::Ellipse => {
            let pts: Vec<Pos2> = (0..=40)
                .map(|i| {
                    let t = i as f32 / 40.0 * std::f32::consts::TAU;
                    p(12.0 + 9.0 * t.cos(), 12.0 + 6.5 * t.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        Tool::Bracket => {
            line(&[(8.5, 4.0), (5.0, 4.0), (5.0, 20.0), (8.5, 20.0)]);
            line(&[(15.5, 4.0), (19.0, 4.0), (19.0, 20.0), (15.5, 20.0)]);
        }
        Tool::Text => {
            // 「T」
            line(&[(5.0, 5.0), (19.0, 5.0)]);
            line(&[(12.0, 5.0), (12.0, 20.0)]);
        }
        Tool::Bubble => {
            painter.rect_stroke(
                Rect::from_min_max(p(3.5, 4.5), p(20.5, 15.5)),
                4.0 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(8.0, 15.5), (6.0, 20.5), (12.5, 15.5)]);
        }
        Tool::Number => {
            painter.circle_filled(r.center(), 10.0 * s, col);
            // 「1」は背景色(円の中を抜く)で描く
            let inv = painter.ctx().global_style().visuals.panel_fill;
            let thin = Stroke::new(2.0 * s, inv);
            painter.add(egui::Shape::line(vec![p(9.5, 9.5), p(12.5, 7.0), p(12.5, 17.0)], thin));
        }
        Tool::Mosaic => {
            // 市松模様
            for (i, (x, y)) in [(0, 0), (1, 1), (2, 0), (0, 2), (2, 2)].into_iter().enumerate() {
                let a = if i % 2 == 0 { 255 } else { 150 };
                let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a);
                let min = p(4.0 + x as f32 * 5.5, 4.0 + y as f32 * 5.5);
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(5.0 * s)), 0.0, c);
            }
            for (x, y) in [(1, 0), (0, 1), (2, 1), (1, 2)] {
                let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 90);
                let min = p(4.0 + x as f32 * 5.5, 4.0 + y as f32 * 5.5);
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(5.0 * s)), 0.0, c);
            }
        }
        Tool::Crop => {
            // トリミングのマーク(2つのL字)
            line(&[(7.0, 2.5), (7.0, 17.0), (21.5, 17.0)]);
            line(&[(2.5, 7.0), (17.0, 7.0), (17.0, 21.5)]);
        }
    }
}
