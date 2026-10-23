//! UI Automation による UI 要素(ボタン・パネル・入力欄など)の検出。

use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationFocusChangedEventHandler,
    IUIAutomationFocusChangedEventHandler_Impl, TreeScope_Children, UIA_BoundingRectanglePropertyId,
    UIA_NamePropertyId,
};
use windows::core::implement;

/// 何もしないフォーカス変更ハンドラ。
/// ChromiumやElectron系のアプリは、UIAの利用者が待機していると分かるまでWeb部分の
/// アクセシビリティの木を作らないので、「待機中」だと知らせるためだけに登録する。
#[implement(IUIAutomationFocusChangedEventHandler)]
struct NoopFocusHandler;

impl IUIAutomationFocusChangedEventHandler_Impl for NoopFocusHandler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        _sender: windows::core::Ref<'_, IUIAutomationElement>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// 物理ピクセルでの (left, top, right, bottom)
    pub rect: (i32, i32, i32, i32),
    pub name: String,
}

pub struct Uia {
    auto: IUIAutomation,
}

/// 親をたどる最大の段数(異常に深い木での暴走防止)
const MAX_DEPTH: usize = 30;

impl Uia {
    pub fn new() -> Option<Self> {
        // SAFETY: COM の初期化と生成。すでに別モードで初期化済みなら CoInitializeEx は失敗するが、そのまま使える
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let handler: IUIAutomationFocusChangedEventHandler = NoopFocusHandler.into();
            let _ = auto.AddFocusChangedEventHandler(None, &handler);
            Some(Self { auto })
        }
    }

    /// ウィンドウ `hwnd` の要素木を上から降り、物理座標 (x, y) を含む要素の一覧を、内側(小さい)から順に返す。
    /// 最後の要素がウィンドウ自身。同じ枠が続く要素や、極端に小さい要素は省く。
    ///
    /// 画面全体への問い合わせ(ElementFromPoint)だと、最前面の選択画面自身が返ってしまうため、
    /// 対象ウィンドウから子を1段ずつたどる。各段は必要な属性だけをまとめて取得する。
    pub fn chain_in_window(&self, hwnd: isize, x: i32, y: i32) -> Vec<Element> {
        let mut path: Vec<Element> = Vec::new();
        // SAFETY: UI Automation の COM 呼び出し。失敗は Err で返るので、その時点で打ち切る
        unsafe {
            let Ok(mut cur) = self.auto.ElementFromHandle(HWND(hwnd as *mut c_void)) else { return path };
            let (Ok(cond), Ok(cache)) = (self.auto.CreateTrueCondition(), self.auto.CreateCacheRequest()) else {
                return path;
            };
            let _ = cache.AddProperty(UIA_BoundingRectanglePropertyId);
            let _ = cache.AddProperty(UIA_NamePropertyId);
            if let Some(e) = current_info(&cur) {
                path.push(e);
            }
            for _ in 0..MAX_DEPTH {
                let Ok(kids) = cur.FindAllBuildCache(TreeScope_Children, &cond, &cache) else { break };
                let n = kids.Length().unwrap_or(0);
                // 兄弟の後ろにあるものほど手前に描かれるので、後ろから探す
                let found = (0..n).rev().find_map(|i| {
                    let k = kids.GetElement(i).ok()?;
                    let r = k.CachedBoundingRectangle().ok()?;
                    let inside = x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                    inside.then_some(k)
                });
                let Some(k) = found else { break };
                if let Some(e) = cached_info(&k)
                    && path.last().is_none_or(|l| l.rect != e.rect)
                {
                    path.push(e);
                }
                cur = k;
            }
        }
        path.reverse();
        path.retain(|e| e.rect.2 - e.rect.0 >= 4 && e.rect.3 - e.rect.1 >= 4);
        path
    }
}

unsafe fn current_info(el: &IUIAutomationElement) -> Option<Element> {
    // SAFETY: 呼び出し元で COM 初期化済み
    let r = unsafe { el.CurrentBoundingRectangle() }.ok()?;
    let name = unsafe { el.CurrentName() }.map(|s| s.to_string()).unwrap_or_default();
    Some(Element { rect: (r.left, r.top, r.right, r.bottom), name })
}

unsafe fn cached_info(el: &IUIAutomationElement) -> Option<Element> {
    // SAFETY: 呼び出し元で COM 初期化済み。取得時にキャッシュ要求した属性のみ読む
    let r = unsafe { el.CachedBoundingRectangle() }.ok()?;
    let name = unsafe { el.CachedName() }.map(|s| s.to_string()).unwrap_or_default();
    Some(Element { rect: (r.left, r.top, r.right, r.bottom), name })
}
