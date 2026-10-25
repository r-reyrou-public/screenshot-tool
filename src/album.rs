//! アルバム: 保存先フォルダの画像をサムネイルで一覧し、見る・コピー・再編集・削除ができる。

use crate::output;
use egui::{Color32, ColorImage, Context, Key, Modifiers, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::time::SystemTime;

const THUMB_PX: u32 = 240;
const CELL: egui::Vec2 = vec2(190.0, 160.0);
const GAP: f32 = 10.0;
/// 一度にサムネイルを読み込む枚数(「さらに表示」で増やす)
const BATCH: usize = 120;
/// 大きく見るときの、画像の長辺の上限(GPU のテクスチャ上限を超えないように)
const VIEW_MAX: u32 = 4096;

pub enum Outcome {
    None,
    Close,
    /// この画像を編集画面で開く(保存済みの元ファイルは変えず、完了すると新しいファイルとして保存される)
    Edit(PathBuf),
}

struct Entry {
    path: PathBuf,
    name: String,
    modified: SystemTime,
    tex: Option<TextureHandle>,
    /// 元画像の大きさ(読み込めたら)
    dims: Option<(u32, u32)>,
}

struct Thumb {
    path: PathBuf,
    image: Option<(ColorImage, (u32, u32))>,
}

pub struct Album {
    dir: PathBuf,
    entries: Vec<Entry>,
    newest_first: bool,
    /// サムネイルを読み込む(表示する)枚数
    shown: usize,
    rx: Receiver<Thumb>,
    tx: Sender<Thumb>,
    selected: Option<usize>,
    /// 大きく表示中の画像
    viewer: Option<(usize, TextureHandle)>,
    confirm_delete: bool,
    status: String,
    /// 閉じたときに、サムネイルの読み込みスレッドへ中止を知らせる
    cancel: Arc<AtomicBool>,
}

impl Drop for Album {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

impl Album {
    pub fn open(ctx: &Context, dir: &Path, newest_first: bool) -> Self {
        let mut entries: Vec<Entry> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_image(&e.path()))
            .map(|e| Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                modified: e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH),
                path: e.path(),
                tex: None,
                dims: None,
            })
            .collect();
        entries.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| a.name.cmp(&b.name)));
        if newest_first {
            entries.reverse();
        }
        let (tx, rx) = channel();
        let mut album = Self {
            dir: dir.to_path_buf(),
            entries,
            newest_first,
            shown: 0,
            rx,
            tx,
            selected: None,
            viewer: None,
            confirm_delete: false,
            status: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        album.show_more(ctx);
        album
    }

    pub fn set_status(&mut self, s: String) {
        self.status = s;
    }

    /// サムネイルの読み込みをバックグラウンドで進める(表示枚数を `BATCH` 枚ずつ増やす)。
    fn show_more(&mut self, ctx: &Context) {
        let from = self.shown;
        self.shown = (self.shown + BATCH).min(self.entries.len());
        let paths: Vec<PathBuf> = self.entries[from..self.shown].iter().map(|e| e.path.clone()).collect();
        let (tx, ctx, cancel) = (self.tx.clone(), ctx.clone(), self.cancel.clone());
        std::thread::spawn(move || {
            for path in paths {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let image = image::open(&path).ok().map(|img| {
                    let dims = (img.width(), img.height());
                    let t = img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
                    (ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw()), dims)
                });
                if tx.send(Thumb { path, image }).is_err() {
                    return; // アルバムが閉じられた
                }
                ctx.request_repaint();
            }
        });
    }

    fn receive_thumbs(&mut self, ctx: &Context) {
        while let Ok(t) = self.rx.try_recv() {
            let Some(e) = self.entries.iter_mut().find(|e| e.path == t.path) else { continue };
            if let Some((img, dims)) = t.image {
                e.tex = Some(ctx.load_texture(format!("album-{}", e.name), img, TextureOptions::LINEAR));
                e.dims = Some(dims);
            }
        }
    }

    fn load_full(path: &Path) -> Option<image::RgbaImage> {
        let img = image::open(path).ok()?;
        Some(img.to_rgba8())
    }

    fn open_viewer(&mut self, ctx: &Context, i: usize) {
        let Some(e) = self.entries.get(i) else { return };
        self.selected = Some(i);
        self.confirm_delete = false;
        match Self::load_full(&e.path) {
            Some(img) => {
                let img = if img.width().max(img.height()) > VIEW_MAX {
                    image::DynamicImage::ImageRgba8(img).thumbnail(VIEW_MAX, VIEW_MAX).to_rgba8()
                } else {
                    img
                };
                let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.viewer = Some((i, ctx.load_texture("album-view", ci, TextureOptions::LINEAR)));
            }
            None => self.status = format!("画像を読み込めません: {}", e.name),
        }
    }

    fn copy_selected(&mut self) {
        let Some(e) = self.selected.and_then(|i| self.entries.get(i)) else { return };
        self.status = match Self::load_full(&e.path).map(|img| output::copy_to_clipboard(&img)) {
            Some(Ok(())) => "クリップボードにコピーしました".into(),
            Some(Err(err)) => format!("コピー失敗: {err:#}"),
            None => "画像を読み込めません".into(),
        };
    }

    fn delete_selected(&mut self) {
        let Some(i) = self.selected.filter(|&i| i < self.entries.len()) else { return };
        let path = self.entries[i].path.clone();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.entries.remove(i);
                self.shown = self.shown.saturating_sub(1);
                self.viewer = None;
                self.selected = (!self.entries.is_empty()).then(|| i.min(self.entries.len() - 1));
                self.status = "削除しました".into();
            }
            Err(e) => self.status = format!("削除できません: {e}"),
        }
        self.confirm_delete = false;
    }

    pub fn ui(&mut self, root: &mut egui::Ui) -> (Outcome, Option<bool>) {
        let ctx = root.ctx().clone();
        self.receive_thumbs(&ctx);
        let mut out = Outcome::None;
        // 並び順を切り替えたいときの新しい値
        let mut reorder = None;

        // キー操作
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.confirm_delete {
                self.confirm_delete = false;
            } else if self.viewer.is_some() {
                self.viewer = None;
            } else {
                out = Outcome::Close;
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::C)) {
            self.copy_selected();
        }
        if self.selected.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete)) {
            self.confirm_delete = true;
        }
        if let Some((vi, _)) = &self.viewer {
            let vi = *vi;
            let step = ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    1
                } else if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                    -1
                } else {
                    0
                }
            });
            let next = (vi as i64 + step).clamp(0, self.entries.len() as i64 - 1) as usize;
            if step != 0 && next != vi {
                self.open_viewer(&ctx, next);
            }
        } else if let Some(i) = self.selected
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
        {
            self.open_viewer(&ctx, i);
        }

        egui::Panel::top("album-top").resizable(false).show(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(format!("{} 枚", self.entries.len()));
                ui.separator();
                let label = if self.newest_first { "新しい順" } else { "古い順" };
                if ui.button(format!("並び: {label}")).clicked() {
                    reorder = Some(!self.newest_first);
                }
                if ui.button("再読み込み").clicked() {
                    reorder = Some(self.newest_first);
                }
                if ui.button("フォルダを開く").clicked() {
                    crate::winutil::open_in_explorer(&self.dir);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("閉じる").clicked() {
                        out = Outcome::Close;
                    }
                });
            });
            ui.add_space(4.0);
        });

        let has_sel = self.selected.is_some_and(|i| i < self.entries.len());
        egui::Panel::bottom("album-bottom").exact_size(34.0).show(root, |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(e) = self.selected.and_then(|i| self.entries.get(i)) {
                    let dims = e.dims.map(|(w, h)| format!("  {w}×{h}")).unwrap_or_default();
                    ui.label(format!("{}{dims}", e.name));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.confirm_delete {
                        if ui.button("やめる").clicked() {
                            self.confirm_delete = false;
                        }
                        if ui.button(egui::RichText::new("本当に削除する").color(Color32::from_rgb(235, 60, 60))).clicked() {
                            self.delete_selected();
                        }
                        ui.label("元に戻せません。");
                    } else {
                        ui.add_enabled_ui(has_sel, |ui| {
                            if ui.button("削除").clicked() {
                                self.confirm_delete = true;
                            }
                            if ui.button("編集").clicked()
                                && let Some(e) = self.selected.and_then(|i| self.entries.get(i))
                            {
                                out = Outcome::Edit(e.path.clone());
                            }
                            if ui.button("コピー").clicked() {
                                self.copy_selected();
                            }
                        });
                        if !self.status.is_empty() {
                            ui.weak(&self.status);
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default().show(root, |ui| {
            if let Some((_, tex)) = &self.viewer {
                let avail = ui.available_rect_before_wrap();
                let size = tex.size_vec2();
                let k = (avail.width() / size.x).min(avail.height() / size.y).clamp(0.01, 1.0);
                let rect = Rect::from_center_size(avail.center(), size * k);
                ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                let resp = ui.allocate_rect(avail, Sense::click());
                if resp.double_clicked() {
                    self.viewer = None;
                }
                ui.painter().text(
                    avail.left_top() + vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    "← → で前後  / ダブルクリックか Esc で一覧へ",
                    egui::FontId::proportional(12.0),
                    Color32::from_gray(150),
                );
                return;
            }
            if self.entries.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(format!("{} に画像がありません", self.dir.display()));
                });
                return;
            }
            let mut want_more = false;
            let mut clicked = None;
            let mut opened = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let cols = (((ui.available_width() + GAP) / (CELL.x + GAP)) as usize).max(1);
                ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                for row in (0..self.shown).step_by(cols) {
                    ui.horizontal(|ui| {
                        for i in row..(row + cols).min(self.shown) {
                            let e = &self.entries[i];
                            let (rect, resp) = ui.allocate_exact_size(CELL, Sense::click());
                            let sel = self.selected == Some(i);
                            let vis = ui.visuals();
                            let bg = if sel { vis.selection.bg_fill } else { vis.faint_bg_color };
                            let p = ui.painter_at(rect);
                            p.rect_filled(rect, 6.0, bg);
                            if sel {
                                p.rect_stroke(rect, 6.0, Stroke::new(2.0, vis.selection.stroke.color), egui::StrokeKind::Inside);
                            } else if resp.hovered() {
                                p.rect_stroke(rect, 6.0, Stroke::new(1.0, vis.widgets.hovered.bg_stroke.color), egui::StrokeKind::Inside);
                            }
                            let img_area = Rect::from_min_max(rect.min + vec2(6.0, 6.0), rect.max - vec2(6.0, 40.0));
                            match &e.tex {
                                Some(t) => {
                                    let s = t.size_vec2();
                                    let k = (img_area.width() / s.x).min(img_area.height() / s.y);
                                    let r = Rect::from_center_size(img_area.center(), s * k);
                                    p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                }
                                None => {
                                    p.text(
                                        img_area.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "…",
                                        egui::FontId::proportional(18.0),
                                        Color32::from_gray(140),
                                    );
                                }
                            }
                            let when: chrono::DateTime<chrono::Local> = e.modified.into();
                            let name = egui::FontId::proportional(12.0);
                            p.text(
                                pos2(rect.min.x + 8.0, rect.max.y - 34.0),
                                egui::Align2::LEFT_TOP,
                                elide(&e.name, 24),
                                name.clone(),
                                vis.text_color(),
                            );
                            p.text(
                                pos2(rect.min.x + 8.0, rect.max.y - 18.0),
                                egui::Align2::LEFT_TOP,
                                when.format("%Y-%m-%d %H:%M").to_string(),
                                egui::FontId::proportional(11.0),
                                Color32::from_gray(140),
                            );
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            if resp.double_clicked() {
                                opened = Some(i);
                            }
                        }
                    });
                }
                if self.shown < self.entries.len() {
                    ui.horizontal(|ui| {
                        if ui.button(format!("さらに表示({} 枚残り)", self.entries.len() - self.shown)).clicked() {
                            want_more = true;
                        }
                    });
                }
            });
            if let Some(i) = clicked {
                self.selected = Some(i);
                self.confirm_delete = false;
                self.status.clear();
            }
            if let Some(i) = opened {
                self.open_viewer(&ctx, i);
            }
            if want_more {
                self.show_more(&ctx);
            }
        });

        (out, reorder)
    }
}

/// 長い名前を、真ん中を省略して `max` 文字ほどにする(拡張子が見えるように)。
fn elide(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail = 8.min(max / 3);
    let head = max - tail - 1;
    format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_names() {
        assert_eq!(elide("a.png", 24), "a.png");
        let e = elide("Screenshot_20260101_123456_12.png", 24);
        assert_eq!(e.chars().count(), 24);
        assert!(e.ends_with("56_12.png") || e.ends_with(".png"));
    }

    #[test]
    fn image_extensions() {
        assert!(is_image(Path::new("a.PNG")));
        assert!(is_image(Path::new("a.jpeg")));
        assert!(!is_image(Path::new("a.txt")));
        assert!(!is_image(Path::new("png")));
    }
}
