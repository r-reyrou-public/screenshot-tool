use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// グローバルホットキー (例: "ctrl+shift+s")。変更は再起動後に反映
    pub hotkey: String,
    /// 保存先フォルダ
    pub save_dir: PathBuf,
    /// 保存形式: "png" または "jpg"
    pub format: String,
    /// ファイル名(拡張子なし)。chrono の書式指定子が使える
    pub name_pattern: String,
    /// JPEG 品質 (1-100)
    pub jpeg_quality: u8,
    /// 出力倍率(%)。100 で等倍
    pub scale_percent: u32,
    /// 出力の長辺の上限(px)。0 で無制限
    pub max_long_edge: u32,
    /// ルーペの表示位置: off / top_left / top_right / bottom_left / bottom_right
    pub loupe_corner: String,
    /// ルーペの倍率(1画素を何ポイントで表示するか)
    pub loupe_zoom: u32,
    /// ルーペの大きさ(ポイント)
    pub loupe_size: u32,
    /// 選んだ隅から画面の内側へのずらし量(ポイント)。範囲選択中の Shift+ドラッグでも変えられる
    pub loupe_offset_x: u32,
    pub loupe_offset_y: u32,
    /// 範囲選択中にウィンドウを自動検出して、クリックでそのウィンドウを撮る
    pub window_snap: bool,
    /// 押している間だけ `window_snap` を反転させるキー(例: "W", "Tab"。"off" で無効)
    pub window_snap_key: String,
    /// ウィンドウ内のUI要素(ボタン・パネルなど)単位まで細かく検出する
    pub element_snap: bool,
    /// 押している間だけ `element_snap` を反転させるキー
    pub element_snap_key: String,
    /// 押している間だけ、ペインではなく最も細かい要素(ボタンやタブ1つなど)を選ぶキー
    pub element_detail_key: String,
    /// 範囲選択中、押している間だけルーペをドラッグで動かせるキー
    pub loupe_move_key: String,
    /// 範囲選択中、押すと画面全体を撮るキー
    pub fullscreen_key: String,
    /// 範囲選択中、押すたびにスクロール撮影の ON/OFF を切り替えるキー
    pub scroll_key: String,
    /// Windows の起動時に、このツールも自動で起動する
    pub autostart: bool,
    /// 見た目のかたまり検出で、同じかたまりとみなす隙間の大きさ(px)。大きいほど大きなかたまりになる
    pub visual_gap: u32,
    /// 数日に1回、新しい版が出ていないか GitHub に問い合わせる
    pub update_check: bool,
    /// 最後に問い合わせた時刻(UNIX 秒)。INI は手で書き換えない前提の内部用
    pub last_update_check: u64,
    /// 問い合わせで分かった最新の版(空なら未確認)
    pub latest_version: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "ctrl+shift+s".into(),
            save_dir: default_save_dir(),
            format: "png".into(),
            name_pattern: "Screenshot_%Y%m%d_%H%M%S".into(),
            jpeg_quality: 90,
            scale_percent: 100,
            max_long_edge: 0,
            loupe_corner: "bottom_right".into(),
            loupe_zoom: 8,
            loupe_size: 180,
            loupe_offset_x: 104,
            loupe_offset_y: 118,
            window_snap: true,
            window_snap_key: "W".into(),
            element_snap: false,
            element_snap_key: "E".into(),
            element_detail_key: "D".into(),
            loupe_move_key: "Shift".into(),
            fullscreen_key: "Enter".into(),
            scroll_key: "S".into(),
            visual_gap: 14,
            autostart: false,
            update_check: true,
            last_update_check: 0,
            latest_version: String::new(),
        }
    }
}

impl Config {
    pub fn is_jpeg(&self) -> bool {
        matches!(self.format.to_ascii_lowercase().as_str(), "jpg" | "jpeg")
    }
}

/// 実行ファイルと同じフォルダの INI。
/// 既定の保存先: 実行ファイルと同じ場所の `img` フォルダ
pub fn default_save_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("img"))).unwrap_or_else(|| PathBuf::from("img"))
}

/// 以前の版が自動で決めていた保存先か(ユーザーが選んだものではない)
fn is_old_default_dir(p: &std::path::Path) -> bool {
    [dirs::picture_dir(), dirs::home_dir()].into_iter().flatten().any(|d| p == d.join("SnapMark"))
}

pub fn ini_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("SnapMark.ini")))
        .unwrap_or_else(|| PathBuf::from("SnapMark.ini"))
}

fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with([';', '#', '[']) {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "hotkey" => cfg.hotkey = v.into(),
            // 以前の版の自動の保存先は、ユーザーの指定ではないので、新しい既定(img)に改める
            "save_dir" if !v.is_empty() && !is_old_default_dir(std::path::Path::new(v)) => cfg.save_dir = PathBuf::from(v),
            "format" => cfg.format = v.to_ascii_lowercase(),
            "name_pattern" if !v.is_empty() => cfg.name_pattern = v.into(),
            "jpeg_quality" => cfg.jpeg_quality = v.parse().unwrap_or(cfg.jpeg_quality).clamp(1, 100),
            "scale_percent" => cfg.scale_percent = v.parse().unwrap_or(cfg.scale_percent).clamp(1, 400),
            "max_long_edge" => cfg.max_long_edge = v.parse().unwrap_or(cfg.max_long_edge),
            "loupe_corner" => cfg.loupe_corner = v.to_ascii_lowercase(),
            "loupe_zoom" => cfg.loupe_zoom = v.parse().unwrap_or(cfg.loupe_zoom).clamp(2, 32),
            "loupe_size" => cfg.loupe_size = v.parse().unwrap_or(cfg.loupe_size).clamp(80, 400),
            "window_snap" => cfg.window_snap = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            "window_snap_key" => cfg.window_snap_key = v.into(),
            "element_snap" => cfg.element_snap = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            "element_snap_key" => cfg.element_snap_key = v.into(),
            "element_detail_key" => cfg.element_detail_key = v.into(),
            "loupe_move_key" => cfg.loupe_move_key = v.into(),
            "fullscreen_key" => cfg.fullscreen_key = v.into(),
            "scroll_key" => cfg.scroll_key = v.into(),
            "autostart" => cfg.autostart = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            "update_check" => cfg.update_check = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            "last_update_check" => cfg.last_update_check = v.parse().unwrap_or(0),
            "latest_version" if v.len() <= 32 => cfg.latest_version = v.into(),
            "visual_gap" => cfg.visual_gap = v.parse().unwrap_or(cfg.visual_gap).clamp(4, 60),
            "loupe_offset_x" => cfg.loupe_offset_x = v.parse().unwrap_or(cfg.loupe_offset_x).min(4000),
            "loupe_offset_y" => cfg.loupe_offset_y = v.parse().unwrap_or(cfg.loupe_offset_y).min(4000),
            _ => {}
        }
    }
    cfg
}

/// 旧名称(ScreenshotTool)の INI が残っていて新しい INI が無ければ、名前を付け替えて設定を引き継ぐ。
fn migrate_legacy_ini(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        let old = dir.join("ScreenshotTool.ini");
        if !path.exists() && old.is_file() {
            let _ = std::fs::rename(old, path);
        }
    }
}

/// INI を読み込む。無ければ既定値で作成する。
pub fn load() -> Config {
    let path = ini_path();
    migrate_legacy_ini(&path);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => {
            let cfg = Config::default();
            if let Err(e) = save(&cfg) {
                eprintln!("ini write failed ({}): {e}", path.display());
            }
            cfg
        }
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let text = format!(
        "[General]\r\n\
         ; グローバルホットキー(変更は再起動後に反映)\r\n\
         hotkey={}\r\n\
         ; Windows の起動時にこのツールも自動で起動する 1=ON 0=OFF\r\n\
         autostart={}\r\n\
         ; 数日に1回、新しい版が出ていないか GitHub に問い合わせる 1=ON 0=OFF\r\n\
         update_check={}\r\n\
         ; 以下の2つは更新確認が自動で書き込む(手で変えなくてよい)\r\n\
         last_update_check={}\r\n\
         latest_version={}\r\n\
         \r\n\
         [Output]\r\n\
         ; 保存先フォルダ\r\n\
         save_dir={}\r\n\
         ; 保存形式: png / jpg\r\n\
         format={}\r\n\
         ; ファイル名(拡張子なし)。%Y年 %m月 %d日 %H時 %M分 %S秒 など\r\n\
         name_pattern={}\r\n\
         ; JPEG品質 1-100\r\n\
         jpeg_quality={}\r\n\
         ; 出力倍率(%) 100で等倍\r\n\
         scale_percent={}\r\n\
         ; 出力の長辺上限(px) 0で無制限\r\n\
         max_long_edge={}\r\n\
         \r\n\
         [Loupe]\r\n\
         ; 範囲選択中のルーペ位置: off / top_left / top_right / bottom_left / bottom_right\r\n\
         loupe_corner={}\r\n\
         ; 倍率 2-32\r\n\
         loupe_zoom={}\r\n\
         ; 大きさ(ポイント) 80-400\r\n\
         loupe_size={}\r\n\
         ; 隅から内側へのずらし量(ポイント)\r\n\
         loupe_offset_x={}\r\n\
         loupe_offset_y={}\r\n\
         \r\n\
         [WindowSnap]\r\n\
         ; ウィンドウ自動検出 1=ON 0=OFF(範囲選択中にウィンドウを枠で示し、クリックでそのウィンドウを撮る)\r\n\
         window_snap={}\r\n\
         ; 押している間だけON/OFFを反転するキー(例: W, Tab, Space)。offで無効\r\n\
         window_snap_key={}\r\n\
         ; ウィンドウ内のUI要素(ボタン・パネルなど)まで細かく検出 1=ON 0=OFF(ホイールで範囲を広げる/狭める)\r\n\
         element_snap={}\r\n\
         ; 押している間だけ element_snap を反転するキー\r\n\
         element_snap_key={}\r\n\
         ; 押している間だけ最も細かい要素(ボタンやタブ1つなど)を選ぶキー。要素検出がOFFでも使える\r\n\
         element_detail_key={}\r\n\
         ; 範囲選択中、押している間だけルーペをドラッグで動かせるキー\r\n\
         loupe_move_key={}\r\n\
         ; 範囲選択中、押すと画面全体を撮るキー\r\n\
         fullscreen_key={}\r\n\
         ; 範囲選択中、押すたびにスクロール撮影の ON/OFF を切り替えるキー\r\n\
         scroll_key={}\r\n\
         ; Webページなど要素を公開しない画面で、詳細キーを押したときに見た目から切り出すかたまりの大きさ\r\n\
         ; (隙間がこのpx以下なら同じかたまり) 4-60。大きいほど大きなかたまりになる\r\n\
         visual_gap={}\r\n",
        cfg.hotkey,
        if cfg.autostart { 1 } else { 0 },
        if cfg.update_check { 1 } else { 0 },
        cfg.last_update_check,
        cfg.latest_version,
        cfg.save_dir.display(),
        cfg.format,
        cfg.name_pattern,
        cfg.jpeg_quality,
        cfg.scale_percent,
        cfg.max_long_edge,
        cfg.loupe_corner,
        cfg.loupe_zoom,
        cfg.loupe_size,
        cfg.loupe_offset_x,
        cfg.loupe_offset_y,
        if cfg.window_snap { 1 } else { 0 },
        cfg.window_snap_key,
        if cfg.element_snap { 1 } else { 0 },
        cfg.element_snap_key,
        cfg.element_detail_key,
        cfg.loupe_move_key,
        cfg.fullscreen_key,
        cfg.scroll_key,
        cfg.visual_gap,
    );
    std::fs::write(ini_path(), text)
}

/// 命名規則から現在時刻のファイル名(拡張子なし)を作る。書式が不正なら None。
pub fn format_name(pattern: &str) -> Option<String> {
    use chrono::format::{Item, StrftimeItems};
    let items: Vec<Item> = StrftimeItems::new(pattern).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return None;
    }
    let name = chrono::Local::now().format_with_items(items.into_iter()).to_string();
    let name: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    // 末尾のピリオドと空白は Windows が黙って削るので、先に落としておく
    let name = name.trim().trim_end_matches('.').trim_end().to_string();
    (!name.is_empty()).then_some(name)
}
