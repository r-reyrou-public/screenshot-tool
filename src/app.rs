use crate::album::{self, Album};
use crate::capture::{self, Shot};
use crate::config::Config;
use crate::editor::{Editor, Outcome};
use crate::output;
use crate::uia::{Element, Uia};
use crate::winlist::WinInfo;
use egui::{
    Color32, ColorImage, Context, CursorIcon, Key, Modifiers, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions,
    ViewportCommand, WindowLevel, pos2, vec2,
};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

pub enum Cmd {
    Capture,
    Settings,
    Album,
    OpenFolder,
    /// 更新確認の結果。最新の版が分かれば `Some`
    UpdateChecked(Option<String>),
    /// 新しい版のリリースページを開く
    OpenUpdatePage,
    Quit,
}

/// タスクトレイのアイコンとメニュー項目。更新があるときにバッジと案内を出すために、アプリが持つ。
pub struct Tray {
    pub icon: tray_icon::TrayIcon,
    pub update_item: tray_icon::menu::MenuItem,
    pub capture_item: tray_icon::menu::MenuItem,
    pub hotkeys: global_hotkey::GlobalHotKeyManager,
    /// いま登録されているホットキー
    pub hotkey: Option<global_hotkey::hotkey::HotKey>,
}

impl Tray {
    /// ホットキーを `text` に切り替える。新しいものを先に登録し、成功してから古いものを外すので、
    /// 失敗しても元のホットキーはそのまま使える。
    pub fn set_hotkey(&mut self, text: &str) -> Result<(), String> {
        use global_hotkey::hotkey::HotKey;
        let new: HotKey = text.parse().map_err(|e| format!("ホットキーの書式が不正です ({text}): {e}"))?;
        if self.hotkey == Some(new) {
            return Ok(());
        }
        self.hotkeys
            .register(new)
            .map_err(|e| format!("ホットキー {text} を登録できません。他のアプリが使っている可能性があります ({e})"))?;
        if let Some(old) = self.hotkey.replace(new) {
            let _ = self.hotkeys.unregister(old);
        }
        self.capture_item.set_text(format!("スクショを撮る ({text})"));
        Ok(())
    }

    fn show_update(&self, version: Option<&str>) {
        let tip = match version {
            Some(v) => format!("SnapMark - 新しい版 v{v} があります"),
            None => "SnapMark".to_string(),
        };
        let _ = self.icon.set_tooltip(Some(tip));
        let _ = self.icon.set_icon(crate::tray_icon_image(version.is_some()));
        match version {
            Some(v) => {
                self.update_item.set_text(format!("● 新しい版 v{v} があります(ダウンロード...)"));
                self.update_item.set_enabled(true);
            }
            None => {
                self.update_item.set_text(format!("SnapMark v{}(最新)", crate::update::current_version()));
                self.update_item.set_enabled(false);
            }
        }
    }
}

/// 待機中ウィンドウを退避させる画面外座標(物理ピクセル)
const PARKED: f32 = -32000.0;
/// 編集画面のツールバー(2段)の高さの目安。ウィンドウの大きさの計算に使う
const TOOLBAR_H: f32 = 70.0;
const STATUS_H: f32 = 28.0;
const MARGIN: f32 = 8.0;

struct Selecting {
    shot: Shot,
    tex: TextureHandle,
    drag_start: Option<Pos2>,
    frames: u32,
    /// ルーペの、選んだ隅から内側へのずらし量(ポイント)
    loupe_off: egui::Vec2,
    /// Shift+ドラッグでルーペを掴んでいる間の、掴んだ位置(ルーペ左上からの差)
    loupe_grab: Option<egui::Vec2>,
    /// 撮影時点のウィンドウ一覧(手前にあるものから)
    windows: Vec<WinInfo>,
    /// カーソル位置のUI要素の一覧(内側から)と、いま選んでいる段
    elem_chain: Vec<Element>,
    /// 詳細キー押下中の候補(小さい順)。UI Automation の要素と、見た目から切り出したかたまりを合わせたもの
    cands: Vec<Element>,
    /// 候補のうち、ホイールを回さないときに選ばれる位置
    cand_default: usize,
    /// ホイールで動かした量(正で大きい方へ)
    cand_offset: i32,
    /// 候補を求めたときの (物理x, 物理y, 時刻)
    vprobe: (i32, i32, Instant),
    /// 要素を調べたときの (物理x, 物理y, 時刻, 対象ウィンドウ)。調べ過ぎないための間引きに使う
    probe: (i32, i32, Instant, isize),
}

struct SettingsDraft {
    cfg: Config,
    dir: String,
    msg: String,
    frames: u32,
    /// ホットキーを、実際にキーを押して入力している最中
    capturing_key: bool,
}

/// エクスプローラーの「パスのコピー」が付ける引用符や前後の空白を除く。
fn clean_path(s: &str) -> String {
    s.trim().trim_matches(['"', '\'']).trim().to_string()
}

const SETTINGS_SIZE: egui::Vec2 = vec2(600.0, 800.0);
const ALBUM_SIZE: egui::Vec2 = vec2(1040.0, 720.0);

/// ウィンドウを、カーソルのあるモニタの中央に置く。
fn center_window(ctx: &Context, size: egui::Vec2) {
    let ppp = ctx.pixels_per_point();
    let pos = match capture::cursor_monitor_rect() {
        Some((x, y, w, h)) => pos2(
            (x as f32 + (w as f32 - size.x * ppp) / 2.0) / ppp,
            (y as f32 + (h as f32 - size.y * ppp) / 2.0) / ppp,
        ),
        None => pos2(200.0, 120.0),
    };
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
}

fn center_settings(ctx: &Context) {
    center_window(ctx, SETTINGS_SIZE);
}

/// 画面の状態。値は mem::replace で受け渡すだけの短命な値なので、大きさの差は許容する。
#[allow(clippy::large_enum_variant)]
enum Mode {
    Idle,
    Settings(SettingsDraft),
    Selecting(Selecting),
    Editing(Editor),
    Album(Album),
}

/// ウィンドウ撮影の予約: (撮る時刻, 範囲(物理座標), 角丸の半径)
type PendingWindow = (Instant, (i32, i32, i32, i32), u32);

pub struct App {
    cfg: Config,
    rx: Receiver<Cmd>,
    mode: Mode,
    pending_capture: Option<Instant>,
    /// ウィンドウ撮影の待機中: (撮る時刻, 対象ウィンドウの物理座標の枠)
    pending_window: Option<PendingWindow>,
    /// UI Automation(要素検出)。最初に必要になったときに作る
    uia: Option<Uia>,
    quitting: bool,
    tray: Tray,
    /// 新しい版があるときの版番号。あればトレイと設定画面でバッジ表示する
    update: Option<String>,
    /// 更新確認の通信中
    checking: bool,
    tx: Sender<Cmd>,
    /// 待機に戻った後、使い終わったメモリを OS へ返す時刻
    trim_at: Option<Instant>,
    /// 起動時にホットキーを登録できなかったときの案内。最初に開く設定画面に出す
    startup_msg: String,
}

impl App {
    pub fn new(cfg: Config, rx: Receiver<Cmd>, tx: Sender<Cmd>, tray: Tray, startup_msg: String) -> Self {
        let mut app = Self {
            cfg,
            rx,
            mode: Mode::Idle,
            pending_capture: None,
            pending_window: None,
            uia: None,
            quitting: false,
            tray,
            update: None,
            checking: false,
            tx,
            trim_at: Some(Instant::now() + Duration::from_secs(3)),
            startup_msg,
        };
        app.refresh_update_badge();
        app
    }

    /// 記録している最新版と、いま動いている版を比べて、バッジ表示を合わせる。
    fn refresh_update_badge(&mut self) {
        self.update = crate::update::newer_than_current(&self.cfg.latest_version);
        self.tray.show_update(self.update.as_deref());
    }

    /// 更新確認の時期が来ていれば、バックグラウンドで GitHub に問い合わせる。
    fn maybe_check_update(&mut self, ctx: &Context) {
        if !self.cfg.update_check || self.checking || !crate::update::is_due(self.cfg.last_update_check) {
            return;
        }
        self.checking = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Cmd::UpdateChecked(crate::update::check_latest()));
            ctx.request_repaint();
        });
    }

    /// 常駐中のウィンドウは画面外の 1x1 に退避させる(非表示にすると再描画が止まるため)。
    fn go_idle(&mut self, ctx: &Context) {
        // 撮影画像・編集内容・アルバムのサムネイルは、ここで手放す(次に使うときに作り直す)
        self.mode = Mode::Idle;
        self.trim_at = Some(Instant::now() + Duration::from_millis(1500));
        let ppp = ctx.pixels_per_point();
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(1.0, 1.0)));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(PARKED / ppp, PARKED / ppp)));
    }

    fn open_settings(&mut self, ctx: &Context) {
        self.maybe_check_update(ctx);
        self.pending_capture = None;
        self.mode = Mode::Settings(SettingsDraft {
            dir: self.cfg.save_dir.display().to_string(),
            cfg: self.cfg.clone(),
            msg: std::mem::take(&mut self.startup_msg),
            frames: 0,
            capturing_key: false,
        });
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::Title("設定".into()));
        center_settings(ctx);
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn open_album(&mut self, ctx: &Context) {
        // 撮影中・編集中は、作業を壊さないよう、そのウィンドウを前面に出すだけにする
        if matches!(self.mode, Mode::Selecting(_) | Mode::Editing(_)) {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            return;
        }
        self.pending_capture = None;
        let newest_first = match &self.mode {
            Mode::Album(a) => a.newest_first(),
            _ => true,
        };
        self.mode = Mode::Album(Album::open(ctx, &self.cfg.save_dir, newest_first));
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::Title("SnapMark - アルバム".into()));
        center_window(ctx, ALBUM_SIZE);
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn album_ui(&mut self, root: &mut egui::Ui, mut a: Album) -> Mode {
        let ctx = &root.ctx().clone();
        let (outcome, reorder) = a.ui(root);
        match outcome {
            album::Outcome::Close => {
                self.go_idle(ctx);
                return Mode::Idle;
            }
            album::Outcome::Edit(path) => match image::open(&path) {
                Ok(img) => {
                    let monitor = capture::cursor_monitor_rect();
                    return self.open_editor(ctx, img.to_rgba8(), monitor);
                }
                Err(e) => a.set_status(format!("画像を開けません: {e}")),
            },
            album::Outcome::None => {}
        }
        if let Some(newest_first) = reorder {
            return Mode::Album(Album::open(ctx, &self.cfg.save_dir, newest_first));
        }
        Mode::Album(a)
    }

    fn settings_ui(&mut self, root: &mut egui::Ui, mut d: SettingsDraft) -> Mode {
        let ctx = &root.ctx().clone();
        // DPI の異なるモニタへ移った直後はずれるので、数フレームは中央に合わせ直す
        if d.frames < 6 {
            center_settings(ctx);
            d.frames += 1;
            ctx.request_repaint();
        }
        let mut cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut save = false;
        egui::Panel::bottom("about").resizable(false).show(root, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong("SnapMark");
                ui.label(format!("Ver.{}", crate::update::current_version()));
                ui.separator();
                ui.label(format!("© {}", crate::update::AUTHOR));
                ui.separator();
                ui.label("Apache-2.0");
            });
            ui.horizontal(|ui| {
                let url = crate::update::repo_url();
                ui.label(&url);
                if ui.small_button("開く").clicked() {
                    crate::winutil::open_repo_page();
                }
            });
            ui.add_space(6.0);
        });
        // 保存・キャンセルは、設定項目が長くても常に見えるよう、フッターの上に固定する
        egui::Panel::bottom("buttons").resizable(false).show(root, |ui| {
            ui.add_space(6.0);
            if !d.msg.is_empty() {
                ui.colored_label(Color32::RED, &d.msg);
            }
            ui.horizontal(|ui| {
                save = ui.button("保存").clicked();
                cancel |= ui.button("キャンセル").clicked();
            });
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(root, |root_ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(root_ui, |ui| {
            egui::Grid::new("settings").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("ホットキー");
                ui.horizontal(|ui| {
                    if d.capturing_key {
                        // 押されたキーの組み合わせをそのまま読み取る(Esc で中止)
                        let pressed = ui.input(|i| {
                            i.events.iter().find_map(|e| match e {
                                egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => {
                                    Some((*key, *modifiers))
                                }
                                _ => None,
                            })
                        });
                        ui.add_enabled(false, egui::TextEdit::singleline(&mut d.cfg.hotkey.clone()).desired_width(180.0));
                        if ui.button("中止").clicked() {
                            d.capturing_key = false;
                        }
                        ui.weak("使いたいキーの組み合わせを押してください");
                        if let Some((key, m)) = pressed {
                            if key != Key::Escape {
                                match hotkey_text(key, m) {
                                    Some(t) => {
                                        d.cfg.hotkey = t;
                                        d.msg.clear();
                                    }
                                    None => d.msg = "そのキーはホットキーに使えません(Ctrl や Alt を併用してください)".into(),
                                }
                            }
                            d.capturing_key = false;
                        }
                    } else {
                        ui.add(egui::TextEdit::singleline(&mut d.cfg.hotkey).desired_width(180.0));
                        if ui.button("キーを押して設定").clicked() {
                            d.capturing_key = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("起動");
                ui.checkbox(&mut d.cfg.autostart, "Windows の起動時に自動で起動する");
                ui.end_row();
                ui.label("更新");
                ui.vertical(|ui| {
                    ui.checkbox(&mut d.cfg.update_check, "数日に1回、新しい版を確認する");
                    match &self.update {
                        Some(v) => {
                            ui.horizontal(|ui| {
                                ui.colored_label(Color32::from_rgb(235, 60, 60), format!("● 新しい版 v{v} があります"));
                                if ui.button("ダウンロードページを開く").clicked() {
                                    crate::winutil::open_release_page();
                                }
                            });
                        }
                        None => {
                            ui.weak(format!("現在の版: v{}", crate::update::current_version()));
                        }
                    }
                });
                ui.end_row();
                ui.label("保存先フォルダ");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.dir).desired_width(250.0));
                    if ui.button("貼り付け").clicked()
                        && let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            d.dir = text;
                        }
                    if ui.button("参照...").clicked() {
                        let mut dlg = rfd::FileDialog::new().set_title("保存先フォルダを選択");
                        let cur = std::path::PathBuf::from(clean_path(&d.dir));
                        if cur.is_dir() {
                            dlg = dlg.set_directory(cur);
                        }
                        if let Some(dir) = dlg.pick_folder() {
                            d.dir = dir.display().to_string();
                        }
                    }
                });
                ui.end_row();
                ui.label("ファイル形式");
                egui::ComboBox::from_id_salt("fmt")
                    .selected_text(if d.cfg.is_jpeg() { "JPEG (jpg)" } else { "PNG" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.cfg.format, "png".into(), "PNG");
                        ui.selectable_value(&mut d.cfg.format, "jpg".into(), "JPEG (jpg)");
                    });
                ui.end_row();
                ui.label("JPEG品質");
                ui.add_enabled(d.cfg.is_jpeg(), egui::Slider::new(&mut d.cfg.jpeg_quality, 1..=100));
                ui.end_row();
                ui.label("命名規則");
                ui.text_edit_singleline(&mut d.cfg.name_pattern);
                ui.end_row();
                ui.label("");
                let ext = if d.cfg.is_jpeg() { "jpg" } else { "png" };
                match crate::config::format_name(&d.cfg.name_pattern) {
                    Some(n) => ui.weak(format!("例: {n}.{ext}")),
                    None => ui.colored_label(Color32::RED, "命名規則の書式が不正です"),
                };
                ui.end_row();
                ui.label("ウィンドウ自動検出");
                ui.vertical(|ui| {
                    ui.checkbox(&mut d.cfg.window_snap, "ON(範囲選択中、クリックでウィンドウを撮影)");
                    ui.horizontal(|ui| {
                        ui.label("押している間 ON/OFF を反転するキー");
                        ui.add(egui::TextEdit::singleline(&mut d.cfg.window_snap_key).desired_width(70.0));
                    });
                    ui.weak("例: W / Tab / Space / Ctrl / Alt(空欄か off で無効)");
                    ui.checkbox(&mut d.cfg.element_snap, "UI要素(ボタン・パネルなど)単位まで細かく検出");
                    ui.horizontal(|ui| {
                        ui.label("押している間 要素検出を反転するキー");
                        ui.add(egui::TextEdit::singleline(&mut d.cfg.element_snap_key).desired_width(70.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("押している間 最も細かい要素を選ぶキー");
                        ui.add(egui::TextEdit::singleline(&mut d.cfg.element_detail_key).desired_width(70.0));
                    });
                    ui.add(egui::Slider::new(&mut d.cfg.visual_gap, 4..=60).text("かたまりの大きさ(隙間 px)"));
                    ui.weak("詳細キー中、Webページなど要素が公開されない画面では、見た目から文字・画像のかたまりを切り出します");
                });
                ui.end_row();
                ui.label("ルーペの位置");
                let corners = [
                    ("off", "表示しない"),
                    ("top_left", "左上"),
                    ("top_right", "右上"),
                    ("bottom_left", "左下"),
                    ("bottom_right", "右下"),
                ];
                let shown = corners
                    .iter()
                    .find(|(k, _)| *k == d.cfg.loupe_corner)
                    .map_or("右下", |(_, l)| *l);
                egui::ComboBox::from_id_salt("loupe").selected_text(shown).show_ui(ui, |ui| {
                    for (k, l) in corners {
                        ui.selectable_value(&mut d.cfg.loupe_corner, k.into(), l);
                    }
                });
                ui.end_row();
                ui.label("ルーペの倍率");
                ui.add(egui::Slider::new(&mut d.cfg.loupe_zoom, 2..=32).suffix(" 倍"));
                ui.end_row();
                ui.label("ルーペの大きさ");
                ui.add(egui::Slider::new(&mut d.cfg.loupe_size, 80..=400));
                ui.end_row();
                ui.label("ルーペの位置調整");
                ui.vertical(|ui| {
                    ui.add(egui::Slider::new(&mut d.cfg.loupe_offset_x, 0..=1200).text("横"));
                    ui.add(egui::Slider::new(&mut d.cfg.loupe_offset_y, 0..=800).text("縦"));
                    ui.weak("隅から内側へのずらし量。範囲選択中に Shift+ドラッグでも動かせます");
                    if ui.button("位置をリセット").clicked() {
                        let def = Config::default();
                        d.cfg.loupe_offset_x = def.loupe_offset_x;
                        d.cfg.loupe_offset_y = def.loupe_offset_y;
                    }
                });
                ui.end_row();
                ui.label("出力倍率");
                ui.add(egui::Slider::new(&mut d.cfg.scale_percent, 10..=200).suffix(" %"));
                ui.end_row();
                ui.label("長辺の上限");
                ui.add(
                    egui::DragValue::new(&mut d.cfg.max_long_edge)
                        .range(0..=20000)
                        .suffix(" px (0=無制限)"),
                );
                ui.end_row();
            });
            ui.add_space(10.0);
            ui.weak(format!("設定ファイル: {}", crate::config::ini_path().display()));
            });
        });
        if save {
            d.cfg.hotkey = d.cfg.hotkey.trim().to_string();
            if d.cfg.hotkey.parse::<global_hotkey::hotkey::HotKey>().is_err() {
                d.msg = "ホットキーの書式が不正です (例: ctrl+shift+s)".into();
            } else if crate::config::format_name(&d.cfg.name_pattern).is_none() {
                d.msg = "命名規則の書式が不正です".into();
            } else if !valid_key_name(&d.cfg.window_snap_key)
                || !valid_key_name(&d.cfg.element_snap_key)
                || !valid_key_name(&d.cfg.element_detail_key)
            {
                d.msg = "キー名が不正です (例: W, Tab, Space, Ctrl)".into();
            } else if clean_path(&d.dir).is_empty() {
                d.msg = "保存先を指定してください".into();
            } else {
                d.cfg.save_dir = clean_path(&d.dir).into();
                // 更新確認の記録は、設定画面を開いている間に更新されていることがあるので引き継ぐ
                d.cfg.last_update_check = self.cfg.last_update_check;
                d.cfg.latest_version = self.cfg.latest_version.clone();
                // ホットキーは実際に登録してみる。失敗したら、設定は保存せずに理由を見せる
                // 自動起動の登録も先に反映する。失敗したら同様に保存しない
                let applied = self.tray.set_hotkey(&d.cfg.hotkey).and_then(|()| {
                    crate::autostart::sync(d.cfg.autostart)
                        .and_then(|()| crate::config::save(&d.cfg).map_err(|e| format!("INIを保存できません: {e}")))
                });
                match applied {
                    Ok(()) => {
                        self.cfg = d.cfg;
                        self.refresh_update_badge();
                        self.go_idle(ctx);
                        return Mode::Idle;
                    }
                    Err(e) => {
                        d.msg = e;
                        // 保存に至らなかったので、ホットキーも元に戻す(失敗しても元の登録が残る)
                        let _ = self.tray.set_hotkey(&self.cfg.hotkey);
                    }
                }
            }
        }
        if cancel {
            self.go_idle(ctx);
            return Mode::Idle;
        }
        Mode::Settings(d)
    }

    /// 撮影要求。待機中なら即座に撮る。編集・設定ウィンドウが開いている場合だけ、
    /// 写り込まないよう退避してから少し待つ。
    fn request_capture(&mut self, ctx: &Context) {
        // 起動中に勝手に通信はせず、ユーザーが撮影・設定を操作したときに、時期が来ていれば確認する
        self.maybe_check_update(ctx);
        if matches!(self.mode, Mode::Idle) {
            self.begin_selection(ctx);
        } else {
            self.go_idle(ctx);
            self.pending_capture = Some(Instant::now() + Duration::from_millis(150));
        }
    }

    fn begin_selection(&mut self, ctx: &Context) {
        match capture::capture_cursor_monitor() {
            Ok(shot) => {
                let size = [shot.image.width() as usize, shot.image.height() as usize];
                let tex = ctx.load_texture(
                    "shot",
                    ColorImage::from_rgba_unmultiplied(size, shot.image.as_raw()),
                    TextureOptions::NEAREST,
                );
                let loupe_off = vec2(self.cfg.loupe_offset_x as f32, self.cfg.loupe_offset_y as f32);
                // ウィンドウ自動検出に使う一覧。キーで一時的にONにできるので、設定がOFFでも集めておく
                let windows = crate::winlist::list_windows();
                self.mode = Mode::Selecting(Selecting {
                    shot,
                    tex,
                    drag_start: None,
                    frames: 0,
                    loupe_off,
                    loupe_grab: None,
                    windows,
                    elem_chain: Vec::new(),
                    cands: Vec::new(),
                    cand_default: 0,
                    cand_offset: 0,
                    vprobe: (-100_000, -100_000, Instant::now()),
                    probe: (i32::MIN, i32::MIN, Instant::now(), 0),
                });
                if self.uia.is_none() {
                    self.uia = Uia::new();
                }
            }
            Err(e) => eprintln!("capture failed: {e:#}"),
        }
    }

    fn fit_overlay(ctx: &Context, shot: &Shot, first: bool) {
        let ppp = ctx.pixels_per_point();
        if first {
            // 装飾やレベルの変更はウィンドウスタイルの再構築を伴うので最初の1回だけ
            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
        }
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(
            shot.origin.0 as f32 / ppp,
            shot.origin.1 as f32 / ppp,
        )));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
            shot.image.width() as f32 / ppp,
            // モニタを1pxだけ覆わない高さにして、Windows に「全画面ウィンドウ」と判定させない
            // (判定されると表示モードの切り替えで映像が一瞬途切れることがある)
            (shot.image.height() as f32 - 1.0) / ppp,
        )));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn selecting_ui(&mut self, root: &mut egui::Ui, mut s: Selecting) -> Mode {
        let ctx = &root.ctx().clone();
        // DPI の異なるモニタへ移動した直後はサイズがずれるので、数フレームは合わせ直す。
        if s.frames < 6 {
            Self::fit_overlay(ctx, &s.shot, s.frames == 0);
            s.frames += 1;
            ctx.request_repaint();
        }

        let cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        // ルーペの配置 (右か, 下か)。"off" や不明な値なら表示しない
        let loupe = match self.cfg.loupe_corner.as_str() {
            "top_left" => Some((false, false)),
            "top_right" => Some((true, false)),
            "bottom_left" => Some((false, true)),
            "bottom_right" => Some((true, true)),
            _ => None,
        };
        let (loupe_zoom, loupe_size) = (self.cfg.loupe_zoom as f32, self.cfg.loupe_size as f32);
        let mut chosen: Option<Rect> = None; // 画面(ポイント)座標での選択範囲
        let mut screen = Rect::NOTHING;
        let mut loupe_moved = false;
        let mut chosen_window: Option<usize> = None;
        // 設定のON/OFFを、指定キーを押している間だけ反転させる
        let snap_on = self.cfg.window_snap != key_held(ctx, &self.cfg.window_snap_key);
        // UI要素単位の検出も同様に、設定を指定キーで一時的に反転できる
        // 詳細キーを押している間は、要素検出の設定に関わらず最も細かい要素を選ぶ
        let detail = snap_on && key_held(ctx, &self.cfg.element_detail_key);
        let elem_mode = snap_on && (detail || self.cfg.element_snap != key_held(ctx, &self.cfg.element_snap_key));
        let mut chosen_elem: Option<(isize, (i32, i32, i32, i32))> = None;
        if elem_mode && s.drag_start.is_none() && s.loupe_grab.is_none() && !ctx.input(|i| i.modifiers.shift) {
            self.probe_elements(&mut s);
            if detail {
                self.probe_visual(&mut s);
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            let rect = ui.max_rect();
            screen = rect;
            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            // OS のカーソルは隠し、画面いっぱいに伸びる細い十字線を代わりに描く
            ui.ctx().set_cursor_icon(CursorIcon::None);
            let painter = ui.painter();
            painter.image(
                s.tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if let Some(c) = ui.input(|i| i.pointer.latest_pos()).filter(|c| rect.contains(*c)) {
                // 明るい背景でも暗い背景でも見えるよう、暗い線の上に細い白線を重ねる
                let ppp = ui.ctx().pixels_per_point();
                let c = pos2((c.x * ppp).floor() / ppp + 0.5 / ppp, (c.y * ppp).floor() / ppp + 0.5 / ppp);
                for (w, color) in [(3.0 / ppp, Color32::from_black_alpha(150)), (1.0 / ppp, Color32::WHITE)] {
                    let stroke = Stroke::new(w, color);
                    painter.hline(rect.x_range(), c.y, stroke);
                    painter.vline(c.x, rect.y_range(), stroke);
                }
            }

            // ルーペの位置。Shift を押している間は逃げず、ドラッグで動かせる。
            let cur = ui.input(|i| i.pointer.latest_pos());
            let shift = ui.input(|i| i.modifiers.shift);
            let geo = loupe.map(|(right, bottom)| {
                let avoid = if shift || s.loupe_grab.is_some() { None } else { cur };
                loupe_geometry(rect, right, bottom, loupe_zoom, loupe_size, s.loupe_off, avoid)
            });

            if resp.drag_started() {
                let press = resp.interact_pointer_pos();
                match (shift, geo, press) {
                    (true, Some((outer, _)), Some(p)) if outer.contains(p) => {
                        s.loupe_grab = Some(p - outer.min);
                    }
                    _ => s.drag_start = press,
                }
            }
            if let (Some(grab), Some((outer, _)), Some(p), Some((right, bottom))) =
                (s.loupe_grab, geo, cur, loupe)
            {
                let min = p - grab;
                let (w, h) = (outer.width(), outer.height());
                s.loupe_off = vec2(
                    (if right { rect.max.x - w - min.x } else { min.x - rect.min.x })
                        .clamp(0.0, (rect.width() - w).max(0.0)),
                    (if bottom { rect.max.y - h - min.y } else { min.y - rect.min.y })
                        .clamp(0.0, (rect.height() - h).max(0.0)),
                );
            }
            let sel = match (s.drag_start, ui.input(|i| i.pointer.latest_pos())) {
                (Some(a), Some(b)) => Some(Rect::from_two_pos(a, b).intersect(rect)),
                _ => None,
            };

            // 画面は暗くせず、撮影した瞬間の見た目のまま、枠と寸法だけを重ねる。
            let label = |painter: &egui::Painter, anchor: Pos2, align: egui::Align2, text: String| {
                let galley = painter.layout_no_wrap(
                    text,
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                );
                let r = align.anchor_size(anchor, galley.size()).expand2(vec2(6.0, 3.0));
                painter.rect_filled(r, 4.0, Color32::from_black_alpha(190));
                painter.galley(r.min + vec2(6.0, 3.0), galley, Color32::WHITE);
            };
            // ウィンドウ自動検出: カーソル下のウィンドウを枠で示す。重なっていればホイールで切り替える。
            let img_w = s.shot.image.width() as f32;
            let k = img_w / rect.width();
            let to_pt = |x: i32, y: i32| {
                pos2(
                    rect.min.x + (x - s.shot.origin.0) as f32 / k,
                    rect.min.y + (y - s.shot.origin.1) as f32 / k,
                )
            };

            // UI要素の検出: カーソル下のウィンドウの「ペイン」(ウィンドウ直下の大きな区画)を選ぶ
            let mut elem_hover: Option<Element> = None;
            if elem_mode
                && !shift
                && s.drag_start.is_none()
                && s.loupe_grab.is_none()
                && cur.is_some_and(|c| rect.contains(c))
            {
                let win_rect = s.windows.iter().find(|w| w.hwnd == s.probe.3).map(|w| w.rect);
                elem_hover = if detail {
                    // 詳細キー押下中だけ、ホイールで候補を1つずつ大きく/小さく切り替える
                    let wheel: f32 = ui.input(|i| {
                        i.events
                            .iter()
                            .map(|e| match e {
                                egui::Event::MouseWheel { delta, .. } => delta.y,
                                _ => 0.0,
                            })
                            .sum()
                    });
                    if wheel > 0.0 {
                        s.cand_offset += 1;
                    } else if wheel < 0.0 {
                        s.cand_offset -= 1;
                    }
                    let last = s.cands.len().saturating_sub(1) as i32;
                    let idx = (s.cand_default as i32 + s.cand_offset).clamp(0, last);
                    s.cand_offset = idx - s.cand_default as i32; // 端で止まったら、それ以上溜めない
                    s.cands.get(idx as usize).cloned()
                } else {
                    win_rect.and_then(|w| pick_pane(&s.elem_chain, w))
                };
            }
            if let Some(el) = &elem_hover {
                let r = Rect::from_min_max(to_pt(el.rect.0, el.rect.1), to_pt(el.rect.2, el.rect.3)).intersect(rect);
                let green = Color32::from_rgb(0, 200, 120);
                painter.rect_stroke(r, 0.0, Stroke::new(5.0, Color32::from_black_alpha(150)), egui::StrokeKind::Inside);
                painter.rect_stroke(r, 0.0, Stroke::new(3.0, green), egui::StrokeKind::Inside);
                let name: String = el.name.chars().take(40).collect();
                let size = format!("{} x {}", el.rect.2 - el.rect.0, el.rect.3 - el.rect.1);
                let mut text = if name.is_empty() { size } else { format!("{name}  ({size})") };
                if detail && !s.cands.is_empty() {
                    let idx = (s.cand_default as i32 + s.cand_offset).clamp(0, s.cands.len() as i32 - 1);
                    text += &format!("   [ホイール: 大きく/小さく {}/{}]", idx + 1, s.cands.len());
                }
                label(
                    painter,
                    r.min + vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    text,
                );
                if resp.clicked() {
                    chosen_elem = Some((s.probe.3, el.rect));
                }
            }

            let mut hover_win: Option<usize> = None;
            if snap_on
                && elem_hover.is_none()
                && s.drag_start.is_none()
                && s.loupe_grab.is_none()
                && !shift
                && let Some(c) = cur.filter(|c| rect.contains(*c)) {
                    let px = s.shot.origin.0 + ((c.x - rect.min.x) * k) as i32;
                    let py = s.shot.origin.1 + ((c.y - rect.min.y) * k) as i32;
                    let stack: Vec<usize> =
                        (0..s.windows.len()).filter(|&i| s.windows[i].contains(px, py)).collect();
                    // 重なっているときは最前面のウィンドウ
                    hover_win = stack.first().copied();
                }
            if let Some(i) = hover_win {
                let w = &s.windows[i];
                let r = Rect::from_min_max(to_pt(w.rect.0, w.rect.1), to_pt(w.rect.2, w.rect.3)).intersect(rect);
                painter.rect_stroke(r, 0.0, Stroke::new(5.0, Color32::from_black_alpha(150)), egui::StrokeKind::Inside);
                painter.rect_stroke(r, 0.0, Stroke::new(3.0, Color32::from_rgb(0, 160, 255)), egui::StrokeKind::Inside);
                let title: String = w.title.chars().take(40).collect();
                label(
                    painter,
                    r.min + vec2(12.0, 12.0),
                    egui::Align2::LEFT_TOP,
                    format!("{title}  ({} x {})", w.rect.2 - w.rect.0, w.rect.3 - w.rect.1),
                );
                if resp.clicked() {
                    chosen_window = Some(i);
                }
            }

            match sel {
                Some(sel) => {
                    painter.rect_stroke(
                        sel,
                        0.0,
                        Stroke::new(3.0, Color32::from_black_alpha(160)),
                        egui::StrokeKind::Outside,
                    );
                    painter.rect_stroke(
                        sel,
                        0.0,
                        Stroke::new(1.5, Color32::from_rgb(0, 160, 255)),
                        egui::StrokeKind::Outside,
                    );
                    let k = s.shot.image.width() as f32 / rect.width();
                    let at = pos2(sel.min.x, (sel.min.y - 10.0).max(rect.min.y + 24.0));
                    label(
                        painter,
                        at,
                        egui::Align2::LEFT_BOTTOM,
                        format!("{} x {}", (sel.width() * k).round(), (sel.height() * k).round()),
                    );
                }
                None => {
                    let hint = if detail {
                        "クリック: 最も細かい要素を撮影  /  ドラッグ: 範囲選択  /  Enter: 全画面  /  右クリック・Esc: キャンセル"
                    } else if elem_mode {
                        "クリック: ペインを撮影(Eを押している間はウィンドウ全体 / Dを押している間は最も細かい要素)  /  ドラッグ: 範囲選択  /  Enter: 全画面  /  右クリック・Esc: キャンセル"
                    } else if snap_on {
                        "クリック: ウィンドウを撮影  /  ドラッグ: 範囲選択  /  Enter: 全画面  /  右クリック・Esc: キャンセル"
                    } else {
                        "ドラッグ: 範囲選択  /  Enter: 全画面  /  Shift+ドラッグ: ルーペ移動  /  右クリック・Esc: キャンセル"
                    };
                    label(
                        painter,
                        pos2(rect.center().x, rect.min.y + 40.0),
                        egui::Align2::CENTER_CENTER,
                        hint.into(),
                    );
                }
            }

            if let (Some((outer, n)), Some(cur)) = (geo, cur)
                && rect.contains(cur) {
                    draw_loupe(painter, &s, rect, cur, outer, n, loupe_zoom);
                }

            if resp.drag_stopped() && s.loupe_grab.take().is_some() {
                loupe_moved = true;
            } else if resp.drag_stopped() {
                match sel {
                    Some(sel) if sel.width() >= 4.0 && sel.height() >= 4.0 => chosen = Some(sel),
                    _ => s.drag_start = None, // 小さすぎる選択はやり直し
                }
            }
        });

        if loupe_moved {
            // ドラッグで決めた位置を設定に保存する
            self.cfg.loupe_offset_x = s.loupe_off.x.round() as u32;
            self.cfg.loupe_offset_y = s.loupe_off.y.round() as u32;
            if let Err(e) = crate::config::save(&self.cfg) {
                eprintln!("ini write failed: {e}");
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            chosen = Some(screen);
        }
        if cancel {
            self.go_idle(ctx);
            return Mode::Idle;
        }
        if let Some(i) = chosen_window {
            let win = s.windows[i].clone();
            let radius = crate::winlist::corner_radius(win.hwnd);
            self.begin_window_capture(ctx, win.hwnd, win.rect, radius);
            return Mode::Idle;
        }
        if let Some((hwnd, rect)) = chosen_elem {
            self.begin_window_capture(ctx, hwnd, rect, 0);
            return Mode::Idle;
        }
        match chosen {
            Some(sel) => self.start_editing(ctx, &s, sel, screen),
            None => Mode::Selecting(s),
        }
    }

    /// ウィンドウ `hwnd` を前面に出し、オーバーレイを消してから少し待って、`rect`(物理座標)を撮り直す。
    /// `radius` は角丸の半径(物理px)。0 でなければ四隅を透過にする。
    fn begin_window_capture(&mut self, ctx: &Context, hwnd: isize, rect: (i32, i32, i32, i32), radius: u32) {
        if hwnd != 0 {
            crate::winlist::focus_window(hwnd);
        }
        self.go_idle(ctx);
        self.pending_window = Some((Instant::now() + Duration::from_millis(350), rect, radius));
    }

    /// 詳細キー押下中の候補を作る。UI Automation の要素(ウィンドウ自身まで)と、撮影済みの画像から
    /// 切り出した複数の大きさの「文字・画像・ボタンのかたまり」を、小さい順に並べる。
    /// ホイールを回さないときの選択位置(`cand_default`)は、UI Automation が十分細かい要素を返すならそれ、
    /// そうでなければ設定の大きさのかたまり。
    fn probe_visual(&self, s: &mut Selecting) {
        let Some((cx, cy)) = capture::cursor_pos() else { return };
        if cx.abs_diff(s.vprobe.0) < 3 && cy.abs_diff(s.vprobe.1) < 3
            || s.vprobe.2.elapsed() < Duration::from_millis(60)
        {
            return;
        }
        s.vprobe = (cx, cy, Instant::now());
        type R = (i32, i32, i32, i32);
        let area = |r: R| (r.2 - r.0).max(0) as i64 * (r.3 - r.1).max(0) as i64;
        let win = s.windows.iter().find(|w| w.hwnd == s.probe.3).map(|w| w.rect);
        let leaf = s.elem_chain.first().filter(|_| s.elem_chain.len() > 1).map(|e| e.rect);
        let Some(base) = win.and_then(|w| pick_pane(&s.elem_chain, w)).map(|p| p.rect).or(win) else {
            s.cands.clear();
            return;
        };

        // 見た目のかたまり(ペインの範囲内で)。設定の大きさも候補に含める
        let mut gaps = vec![5, 9, 14, 22, 34, 50, self.cfg.visual_gap];
        gaps.sort_unstable();
        gaps.dedup();
        let o = s.shot.origin;
        let bounds = (base.0 - o.0, base.1 - o.1, base.2 - o.0, base.3 - o.1);
        let blocks = crate::visual::blocks_at(&s.shot.image, cx - o.0, cy - o.1, bounds, &gaps);
        let own_block = blocks.iter().find(|(g, _)| *g == self.cfg.visual_gap).map(|(_, r)| *r);
        let shift = |r: R| (r.0 + o.0, r.1 + o.1, r.2 + o.0, r.3 + o.1);

        // 候補: UI Automation の要素 + 見た目のかたまり
        let mut cands: Vec<Element> = s.elem_chain.clone();
        cands.extend(blocks.iter().map(|(_, r)| Element { rect: shift(*r), name: "見た目のかたまり".into() }));
        cands.sort_by_key(|e| area(e.rect));
        // ほぼ同じ枠は1つにまとめる(各辺6px以内)
        cands.dedup_by(|b, a| {
            (a.rect.0 - b.rect.0).abs() <= 6
                && (a.rect.1 - b.rect.1).abs() <= 6
                && (a.rect.2 - b.rect.2).abs() <= 6
                && (a.rect.3 - b.rect.3).abs() <= 6
        });

        // 標準の選択位置
        let default_rect =
            if leaf.is_some_and(|l| area(l) * 100 < area(base) * 60) { leaf } else { own_block.map(shift).or(leaf) };
        let near = |a: R, b: R| {
            (a.0 - b.0).abs() <= 6 && (a.1 - b.1).abs() <= 6 && (a.2 - b.2).abs() <= 6 && (a.3 - b.3).abs() <= 6
        };
        let new_default = default_rect.and_then(|d| cands.iter().position(|c| near(c.rect, d))).unwrap_or(0);
        // カーソルが別の要素に移ったら、ホイールで動かした量は捨てる
        if s.cands.get(s.cand_default).map(|c| c.rect) != cands.get(new_default).map(|c| c.rect) {
            s.cand_offset = 0;
        }
        s.cands = cands;
        s.cand_default = new_default;
    }
    /// カーソル下のウィンドウについて、UI要素の一覧を更新する。呼び過ぎないよう間引く。
    fn probe_elements(&self, s: &mut Selecting) {
        let Some((cx, cy)) = capture::cursor_pos() else { return };
        let Some(hwnd) = s.windows.iter().find(|w| w.contains(cx, cy)).map(|w| w.hwnd) else {
            s.elem_chain.clear();
            return;
        };
        let moved = cx.abs_diff(s.probe.0) >= 3 || cy.abs_diff(s.probe.1) >= 3 || hwnd != s.probe.3;
        if !moved || s.probe.2.elapsed() < Duration::from_millis(60) {
            return;
        }
        let Some(uia) = &self.uia else { return };
        let chain = uia.chain_in_window(hwnd, cx, cy);
        s.elem_chain = chain;
        s.probe = (cx, cy, Instant::now(), hwnd);
    }

    /// 前面に出したウィンドウの枠を、いま見えている画面から切り出して編集画面を開く。
    fn finish_window_capture(&mut self, ctx: &Context, rect: (i32, i32, i32, i32), radius: u32) {
        let (cx, cy) = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
        let mut shot = match capture::capture_monitor_at(cx, cy) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("window capture failed: {e:#}");
                return;
            }
        };
        let (iw, ih) = (shot.image.width() as i32, shot.image.height() as i32);
        let x0 = (rect.0 - shot.origin.0).clamp(0, iw - 1);
        let y0 = (rect.1 - shot.origin.1).clamp(0, ih - 1);
        let x1 = (rect.2 - shot.origin.0).clamp(x0 + 1, iw);
        let y1 = (rect.3 - shot.origin.1).clamp(y0 + 1, ih);
        let region = (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        if radius > 0 {
            round_corners(&mut shot.image, region, radius);
        }
        self.mode = self.crop_and_edit(ctx, &shot, region);
    }

    fn start_editing(&mut self, ctx: &Context, s: &Selecting, sel: Rect, screen: Rect) -> Mode {
        let img = &s.shot.image;
        let kx = img.width() as f32 / screen.width();
        let ky = img.height() as f32 / screen.height();
        let x = (((sel.min.x - screen.min.x) * kx).round() as u32).min(img.width() - 1);
        let y = (((sel.min.y - screen.min.y) * ky).round() as u32).min(img.height() - 1);
        let w = ((sel.width() * kx).round() as u32).clamp(1, img.width() - x);
        let h = ((sel.height() * ky).round() as u32).clamp(1, img.height() - y);
        self.crop_and_edit(ctx, &s.shot, (x, y, w, h))
    }

    /// `shot` の (x, y, w, h)(画像ピクセル)を切り出して編集画面を開く。
    fn crop_and_edit(&mut self, ctx: &Context, shot: &Shot, (x, y, w, h): (u32, u32, u32, u32)) -> Mode {
        let img = &shot.image;
        let base = image::imageops::crop_imm(img, x, y, w, h).to_image();
        let monitor = Some((shot.origin.0, shot.origin.1, img.width(), img.height()));
        self.open_editor(ctx, base, monitor)
    }

    /// `base` を編集画面で開く。`monitor`(物理座標の x, y, 幅, 高さ)の中央にウィンドウを置く。
    fn open_editor(&mut self, ctx: &Context, base: image::RgbaImage, monitor: Option<(i32, i32, u32, u32)>) -> Mode {
        let (w, h) = (base.width(), base.height());
        let (mx, my, mw, mh) = monitor.unwrap_or((0, 0, 1920, 1080));
        let ppp = ctx.pixels_per_point();
        let zoom = 1.0 / ppp; // 等倍(1画像ピクセル = 1物理ピクセル)
        let (mon_w, mon_h) = (mw as f32 / ppp, mh as f32 / ppp);
        let win_w = (w as f32 * zoom + MARGIN * 2.0).max(980.0).min(mon_w - 80.0);
        // 左のツールパレット(縦一列)が収まる高さを下限にする
        let win_h = (h as f32 * zoom + MARGIN * 2.0 + TOOLBAR_H + STATUS_H).max(640.0).min(mon_h - 80.0);
        let origin = vec2(mx as f32 / ppp, my as f32 / ppp);
        // 確認ウィンドウは撮影したモニタの中央に出す
        let pos = pos2(origin.x + ((mon_w - win_w) / 2.0).max(0.0), origin.y + ((mon_h - win_h) / 2.0).max(0.0));

        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::Title("SnapMark".into()));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(win_w, win_h)));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        ctx.send_viewport_cmd(ViewportCommand::Focus);

        Mode::Editing(Editor::new(ctx, base, zoom))
    }

    fn editing_ui(&mut self, root: &mut egui::Ui, mut e: Editor) -> Mode {
        let ctx = &root.ctx().clone();
        match e.ui(root) {
            Outcome::Discard => {
                self.go_idle(ctx);
                return Mode::Idle;
            }
            Outcome::Finish => {
                let out = output::apply_resolution(&e.final_image(), &self.cfg);
                let saved = output::save(&out, &self.cfg);
                let copied = output::copy_to_clipboard(&out);
                match (saved, copied) {
                    (Ok(path), Ok(())) => {
                        let _ = path;
                        self.go_idle(ctx);
                        return Mode::Idle;
                    }
                    (s, c) => {
                        let mut msg = String::new();
                        if let Err(err) = s {
                            msg += &format!("保存失敗: {err:#}  ");
                        }
                        if let Err(err) = c {
                            msg += &format!("コピー失敗: {err:#}");
                        }
                        e.status = msg;
                    }
                }
            }
            Outcome::Copy => {
                let out = output::apply_resolution(&e.final_image(), &self.cfg);
                e.status = match output::copy_to_clipboard(&out) {
                    Ok(()) => "クリップボードにコピーしました".into(),
                    Err(err) => format!("コピー失敗: {err:#}"),
                };
            }
            Outcome::None => {}
        }
        Mode::Editing(e)
    }
}

/// ウィンドウ直下の大きな区画(ペイン)を選ぶ。`chain` は内側から外側の順。
/// ウィンドウとほぼ同じ大きさの要素(ウィンドウ自身や、全面を覆う入れ物)は除き、
/// 残った中で最も大きいもの(= 一番外側の区画)を返す。無ければ None(ウィンドウ単位にする)。
fn pick_pane(chain: &[Element], win: (i32, i32, i32, i32)) -> Option<Element> {
    let area = |r: (i32, i32, i32, i32)| (r.2 - r.0).max(0) as i64 * (r.3 - r.1).max(0) as i64;
    let limit = area(win) * 97 / 100;
    chain.iter().filter(|e| area(e.rect) < limit).max_by_key(|e| area(e.rect)).cloned()
}

/// 設定で指定されたキー(`W` や `Tab`、`Shift`/`Ctrl`/`Alt`)を押している間 true。
fn key_held(ctx: &Context, name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() || n.eq_ignore_ascii_case("off") {
        return false;
    }
    ctx.input(|i| match n.to_ascii_lowercase().as_str() {
        "shift" => i.modifiers.shift,
        "ctrl" | "control" => i.modifiers.ctrl,
        "alt" => i.modifiers.alt,
        _ => Key::from_name(n).is_some_and(|k| i.key_down(k)),
    })
}

/// 設定画面での入力チェック用。
/// 押されたキーと修飾キーから、ホットキーの文字列(例: "ctrl+shift+s")を作る。
/// 修飾キー無しの文字キーは、入力を奪ってしまうので受け付けない(F1〜F24 だけは単独で可)。
fn hotkey_text(key: Key, m: Modifiers) -> Option<String> {
    let name = match key {
        Key::ArrowUp => "up".to_string(),
        Key::ArrowDown => "down".to_string(),
        Key::ArrowLeft => "left".to_string(),
        Key::ArrowRight => "right".to_string(),
        Key::Escape => return None,
        k => {
            let n = k.name();
            // 数字キーは "Num1" のように返るので、数字だけにする
            n.strip_prefix("Num").filter(|d| d.len() == 1).unwrap_or(n).to_ascii_lowercase()
        }
    };
    let is_fkey = name.strip_prefix("f").is_some_and(|d| d.parse::<u32>().is_ok());
    if !(m.ctrl || m.alt || m.shift || is_fkey) {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    if m.ctrl {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    parts.push(&name);
    let text = parts.join("+");
    text.parse::<global_hotkey::hotkey::HotKey>().is_ok().then_some(text)
}

fn valid_key_name(name: &str) -> bool {
    let n = name.trim();
    n.is_empty()
        || n.eq_ignore_ascii_case("off")
        || matches!(n.to_ascii_lowercase().as_str(), "shift" | "ctrl" | "control" | "alt")
        || Key::from_name(n).is_some()
}

const LOUPE_INFO_H: f32 = 24.0;

/// ルーペの枠(円+情報札)の矩形と、見える画素数(奇数)を求める。
/// `off` は選んだ隅から画面の内側へのずらし量。`avoid` にカーソル位置を渡すと、
/// 近づいたとき反対側の隅へ逃げる。
fn loupe_geometry(
    screen: Rect,
    right: bool,
    bottom: bool,
    zoom: f32,
    size: f32,
    off: egui::Vec2,
    avoid: Option<Pos2>,
) -> (Rect, i32) {
    // 中心画素を真ん中に置くため、見える画素数は奇数にする
    let n = ((size / zoom).round() as i32).max(3) | 1;
    let side = n as f32 * zoom;
    let place = |right: bool| {
        let x = if right { screen.max.x - side - off.x } else { screen.min.x + off.x };
        let y = if bottom { screen.max.y - side - LOUPE_INFO_H - off.y } else { screen.min.y + off.y };
        Rect::from_min_size(pos2(x, y), vec2(side, side + LOUPE_INFO_H))
    };
    let mut outer = place(right);
    if avoid.is_some_and(|cur| outer.expand(24.0).contains(cur)) {
        outer = place(!right);
    }
    (outer, n)
}

/// カーソル周辺を拡大する円形のルーペを描く。
fn draw_loupe(painter: &egui::Painter, s: &Selecting, screen: Rect, cur: Pos2, outer: Rect, n: i32, zoom: f32) {
    const INFO_H: f32 = LOUPE_INFO_H;
    let side = n as f32 * zoom;

    let img = &s.shot.image;
    let (w, h) = (img.width() as i32, img.height() as i32);
    let px = (((cur.x - screen.min.x) * w as f32 / screen.width()).floor() as i32).clamp(0, w - 1);
    let py = (((cur.y - screen.min.y) * h as f32 / screen.height()).floor() as i32).clamp(0, h - 1);
    let half = n / 2;
    let uv = Rect::from_min_max(
        pos2((px - half) as f32 / w as f32, (py - half) as f32 / h as f32),
        pos2((px + half + 1) as f32 / w as f32, (py + half + 1) as f32 / h as f32),
    );

    let view = Rect::from_min_size(outer.min, vec2(side, side));
    let (center, radius) = (view.center(), side / 2.0);

    // 円形に切り抜くため、画像を貼った円盤のメッシュを自前で作る
    const SEGMENTS: u32 = 72;
    let mut mesh = egui::epaint::Mesh::with_texture(s.tex.id());
    let uv_c = uv.center();
    mesh.vertices.push(egui::epaint::Vertex { pos: center, uv: uv_c, color: Color32::WHITE });
    for i in 0..SEGMENTS {
        let (sin, cos) = (i as f32 / SEGMENTS as f32 * std::f32::consts::TAU).sin_cos();
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center + vec2(cos, sin) * radius,
            uv: uv_c + vec2(cos * uv.width() / 2.0, sin * uv.height() / 2.0),
            color: Color32::WHITE,
        });
    }
    for i in 0..SEGMENTS {
        mesh.indices.extend_from_slice(&[0, 1 + i, 1 + (i + 1) % SEGMENTS]);
    }
    painter.circle_filled(center, radius + 6.0, Color32::from_black_alpha(90)); // 影
    painter.add(egui::Shape::mesh(mesh));

    // 枠取り: 外側の黒 → 白 → 青の三重リング(どんな背景でも輪郭が分かる)
    painter.circle_stroke(center, radius + 4.5, Stroke::new(1.5, Color32::BLACK));
    painter.circle_stroke(center, radius + 2.0, Stroke::new(3.5, Color32::WHITE));
    painter.circle_stroke(center, radius - 1.0, Stroke::new(2.5, Color32::from_rgb(0, 160, 255)));

    // 中心画素の枠(明るい背景でも暗い背景でも見えるよう二重にする)
    let cell = Rect::from_center_size(center, vec2(zoom, zoom));
    painter.rect_stroke(cell.expand(1.0), 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
    painter.rect_stroke(cell, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);

    // 円の下に、座標と色の情報を載せた角丸の札
    let px_color = img.get_pixel(px as u32, py as u32);
    let galley = painter.layout_no_wrap(
        format!("({px}, {py})  #{:02X}{:02X}{:02X}", px_color[0], px_color[1], px_color[2]),
        egui::FontId::monospace(12.0),
        Color32::WHITE,
    );
    let pill_w = 8.0 + 14.0 + 8.0 + galley.size().x + 10.0;
    let pill = Rect::from_min_size(pos2(center.x - pill_w / 2.0, view.max.y + 10.0), vec2(pill_w, INFO_H - 4.0));
    painter.rect_filled(pill, 6.0, Color32::from_black_alpha(210));
    painter.rect_stroke(pill, 6.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
    let swatch = Rect::from_center_size(pos2(pill.min.x + 15.0, pill.center().y), vec2(14.0, 14.0));
    painter.rect_filled(swatch, 2.0, Color32::from_rgb(px_color[0], px_color[1], px_color[2]));
    painter.rect_stroke(swatch, 2.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
    painter.galley(pos2(swatch.max.x + 8.0, pill.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Cmd::Capture => self.request_capture(ctx),
                Cmd::Settings => self.open_settings(ctx),
                Cmd::Album => self.open_album(ctx),
                Cmd::OpenFolder => {
                    let _ = std::fs::create_dir_all(&self.cfg.save_dir);
                    crate::winutil::open_in_explorer(&self.cfg.save_dir);
                }
                Cmd::UpdateChecked(latest) => {
                    self.checking = false;
                    // 通信に失敗したときは時刻を更新せず、次の合図でやり直す
                    if let Some(v) = latest {
                        self.cfg.last_update_check = crate::update::now_secs();
                        self.cfg.latest_version = v;
                        if let Err(e) = crate::config::save(&self.cfg) {
                            eprintln!("ini write failed: {e}");
                        }
                        self.refresh_update_badge();
                    }
                }
                Cmd::OpenUpdatePage => crate::winutil::open_release_page(),
                Cmd::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        if let Some(at) = self.trim_at
            && matches!(self.mode, Mode::Idle)
            && Instant::now() >= at
        {
            crate::winutil::trim_memory();
            // 待機中にじわじわ増える分も、定期的に返す
            self.trim_at = Some(Instant::now() + Duration::from_secs(300));
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.go_idle(ctx);
        }

        if let Some((at, rect, radius)) = self.pending_window {
            if Instant::now() >= at {
                self.pending_window = None;
                self.finish_window_capture(ctx, rect, radius);
            } else {
                ctx.request_repaint_after(Duration::from_millis(20));
            }
        }

        if let Some(at) = self.pending_capture {
            if Instant::now() >= at {
                self.pending_capture = None;
                self.begin_selection(ctx);
            } else {
                ctx.request_repaint_after(Duration::from_millis(20));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let mode = std::mem::replace(&mut self.mode, Mode::Idle);
        self.mode = match mode {
            Mode::Idle => {
                // イベント経由の再描画要求が届かない場合に備えた低頻度の保険
                ctx.request_repaint_after(Duration::from_millis(250));
                Mode::Idle
            }
            Mode::Selecting(s) => self.selecting_ui(ui, s),
            Mode::Editing(e) => self.editing_ui(ui, e),
            Mode::Settings(d) => self.settings_ui(ui, d),
            Mode::Album(a) => self.album_ui(ui, a),
        };

        // 編集・設定ウィンドウは他のウィンドウに隠れても Alt+Tab で戻れるようにする
        let listed = matches!(self.mode, Mode::Editing(_) | Mode::Settings(_) | Mode::Album(_));
        crate::winutil::set_listed(frame, listed);
    }
}

/// `region`(x, y, 幅, 高さ)の四隅を、半径 `radius` の角丸の外側だけ透過にする(縁は滑らかに)。
fn round_corners(img: &mut image::RgbaImage, (x, y, w, h): (u32, u32, u32, u32), radius: u32) {
    let r = radius.min(w / 2).min(h / 2);
    let rf = r as f32;
    for dy in 0..r {
        for dx in 0..r {
            // 角の円の中心から、画素の中心までの距離
            let dist = ((rf - dx as f32 - 0.5).powi(2) + (rf - dy as f32 - 0.5).powi(2)).sqrt();
            let keep = (rf - dist + 0.5).clamp(0.0, 1.0);
            if keep >= 1.0 {
                continue;
            }
            for (px, py) in [(x + dx, y + dy), (x + w - 1 - dx, y + dy), (x + dx, y + h - 1 - dy), (x + w - 1 - dx, y + h - 1 - dy)] {
                if let Some(p) = img.get_pixel_mut_checked(px, py) {
                    p.0[3] = (p.0[3] as f32 * keep).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::round_corners;

    #[test]
    fn corners_become_transparent_but_center_stays() {
        let mut img = image::RgbaImage::from_pixel(40, 30, image::Rgba([10, 20, 30, 255]));
        round_corners(&mut img, (0, 0, 40, 30), 8);
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_eq!(img.get_pixel(39, 29).0[3], 0);
        assert_eq!(img.get_pixel(20, 15).0[3], 255);
        assert_eq!(img.get_pixel(8, 0).0[3], 255);
    }
}

#[cfg(test)]
mod hotkey_tests {
    use super::hotkey_text;
    use egui::{Key, Modifiers};

    #[test]
    fn builds_parseable_hotkey_text() {
        let m = Modifiers { ctrl: true, shift: true, ..Default::default() };
        assert_eq!(hotkey_text(Key::S, m).as_deref(), Some("ctrl+shift+s"));
        assert_eq!(hotkey_text(Key::Num1, Modifiers { alt: true, ..Default::default() }).as_deref(), Some("alt+1"));
        assert_eq!(hotkey_text(Key::F9, Modifiers::NONE).as_deref(), Some("f9"));
        assert_eq!(hotkey_text(Key::ArrowUp, m).as_deref(), Some("ctrl+shift+up"));
        // 修飾キー無しの文字キーは、入力を奪うので不可
        assert_eq!(hotkey_text(Key::S, Modifiers::NONE), None);
    }
}
