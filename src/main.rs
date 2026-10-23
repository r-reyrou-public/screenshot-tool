#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod capture;
mod config;
mod editor;
mod icons;
mod output;
mod raster;
mod shapes;
mod text;
mod uia;
mod visual;
mod winlist;
mod winutil;

use anyhow::{Result, anyhow};
use app::{App, Cmd};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::sync::{Arc, mpsc};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

fn tray_icon_image() -> Option<Icon> {
    const N: u32 = 32;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            // 青地に白いファインダー枠
            let edge = x.min(y).min(N - 1 - x).min(N - 1 - y);
            let px = if (5..=7).contains(&edge) { [255, 255, 255, 255] } else { [30, 120, 230, 255] };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, N, N).ok()
}

fn setup_fonts(ctx: &egui::Context) {
    let Some(dir) = winutil::fonts_dir() else { return };
    let candidates = ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"];
    let Some(bytes) = candidates.iter().find_map(|name| std::fs::read(dir.join(name)).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("jp".into(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("jp".into());
    }
    ctx.set_fonts(fonts);
}

/// リリース版にはコンソールが無いので、起動時のエラーはメッセージボックスで知らせる。
fn main() {
    if let Err(e) = run() {
        winutil::error_box(&format!("{e:#}"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if !winutil::acquire_single_instance() {
        winutil::error_box("すでに起動しています。タスクトレイのアイコンを確認してください。");
        return Ok(());
    }
    let cfg = config::load();
    // 設定の自動起動の有無と、レジストリの登録(exe の場所を含む)を合わせる
    if let Err(e) = autostart::sync(cfg.autostart) {
        eprintln!("autostart: {e}");
    }
    let hotkey: HotKey = cfg.hotkey.parse().map_err(|e| anyhow!("ホットキー設定が不正です ({}): {e}", cfg.hotkey))?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("スクショ編集")
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size([1.0, 1.0])
            .with_position([-32000.0, -32000.0]),
        ..Default::default()
    };

    let app_cfg = cfg.clone();
    eframe::run_native(
        "screenshot-tool",
        options,
        Box::new(move |cc| {
            setup_fonts(&cc.egui_ctx);
            let (tx, rx) = mpsc::channel::<Cmd>();

            // グローバルホットキー(イベントループのあるこのスレッドで登録する必要がある)
            let manager = GlobalHotKeyManager::new()?;
            manager.register(hotkey)?;
            {
                let (tx, ctx) = (tx.clone(), cc.egui_ctx.clone());
                GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                    if e.state == HotKeyState::Pressed {
                        let _ = tx.send(Cmd::Capture);
                        ctx.request_repaint();
                    }
                }));
            }

            // タスクトレイ
            let menu = Menu::new();
            let capture_item = MenuItem::new("撮影", true, None);
            let folder_item = MenuItem::new("保存フォルダを開く", true, None);
            let config_item = MenuItem::new("設定...", true, None);
            let quit_item = MenuItem::new("終了", true, None);
            menu.append_items(&[
                &capture_item,
                &PredefinedMenuItem::separator(),
                &folder_item,
                &config_item,
                &PredefinedMenuItem::separator(),
                &quit_item,
            ])?;
            let ids =
                (capture_item.id().clone(), folder_item.id().clone(), config_item.id().clone(), quit_item.id().clone());
            {
                let (tx, ctx) = (tx.clone(), cc.egui_ctx.clone());
                MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                    if e.id == ids.0 {
                        let _ = tx.send(Cmd::Capture);
                    } else if e.id == ids.1 {
                        let _ = tx.send(Cmd::OpenFolder);
                    } else if e.id == ids.2 {
                        let _ = tx.send(Cmd::Settings);
                    } else if e.id == ids.3 {
                        let _ = tx.send(Cmd::Quit);
                    }
                    ctx.request_repaint();
                }));
            }
            {
                let (tx, ctx) = (tx.clone(), cc.egui_ctx.clone());
                TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left, button_state: MouseButtonState::Up, ..
                    } = e
                    {
                        let _ = tx.send(Cmd::Capture);
                        ctx.request_repaint();
                    }
                }));
            }
            let mut tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("スクショツール");
            if let Some(icon) = tray_icon_image() {
                tray = tray.with_icon(icon);
            }
            let tray = tray.build()?;

            // ホットキーとトレイはアプリが生きている間ずっと保持する
            std::mem::forget(manager);
            std::mem::forget(tray);

            Ok(Box::new(App::new(app_cfg, rx)))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}
