//! 状態機械(待機・監視中・検知済み・一時停止)とメッセージループ(spec.md 9章)。
//!
//! M0 では起動の手順とメインウィンドウの骨組みだけを置く。骨組みだけの exe のサイズとメモリを
//! 測るため(spec.md 9章の技術検証項目)。トレイ常駐と、多重起動時に動いている方を前面に出す
//! 処理は M1。

#![forbid(unsafe_code)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, SW_SHOWDEFAULT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_DESTROY, WM_DPICHANGED, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};

use crate::platform::{self, Instance, Message, Window, WindowClass, WindowSpec};
use crate::text;

/// 多重起動の検出に使うミューテックスの名前(S-14)。ログオンセッションの中だけで見える。
const INSTANCE_MUTEX_NAME: &str = "Local\\Monitoaster-24E6DD02-4A51-4532-8102-0437BDC2448E";

/// メインウィンドウのクラス名。
const MAIN_CLASS_NAME: &str = "Monitoaster.Main";

/// メインウィンドウのクライアント領域の幅(96 DPI 基準)。
const MAIN_CLIENT_WIDTH: i32 = 640;

/// メインウィンドウのクライアント領域の高さ(96 DPI 基準)。
const MAIN_CLIENT_HEIGHT: i32 = 400;

/// メインウィンドウのスタイル。大きさ固定で、最大化なし。
const MAIN_STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

/// メインウィンドウの拡張スタイル。
const MAIN_EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0);

/// アプリを動かす。メインウィンドウが閉じられたら戻る。
pub fn run() {
    // S-09: 動的に読み込む DLL の検索先を、ほかの処理より先に System32 に限る。
    // 限れなければ起動しない(安全を優先する。Windows 10 以降では失敗しない API)。
    if platform::restrict_dll_search().is_err() {
        return;
    }
    // S-14: すでに動いていれば、何もせずに終わる。
    // 検出そのものに失敗したときは起動を続ける(起動しないより、2つ動く方が害が小さい)。
    let _instance_lock = match platform::acquire_instance(INSTANCE_MUTEX_NAME) {
        Ok(Instance::First(lock)) => Some(lock),
        Ok(Instance::AlreadyRunning) => return,
        Err(_) => None,
    };
    let Ok(window) = create_main_window() else {
        return;
    };
    window.show(SW_SHOWDEFAULT);
    platform::run_message_loop();
}

/// メインウィンドウを非表示で作り、置かれたモニターの DPI に合わせて大きさを決める。
fn create_main_window() -> platform::Result<Window> {
    let class = WindowClass::register(MAIN_CLASS_NAME, Some(COLOR_BTNFACE))?;
    let window = platform::create_window(
        class,
        &WindowSpec {
            ex_style: MAIN_EX_STYLE,
            style: MAIN_STYLE,
            title: text::APP_NAME,
            x: CW_USEDEFAULT,
            y: CW_USEDEFAULT,
            width: CW_USEDEFAULT,
            height: CW_USEDEFAULT,
            parent: None,
        },
        handle_main_message,
    )?;
    let (width, height) = main_window_size(window.dpi())?;
    window.set_pos(
        None,
        0,
        0,
        width,
        height,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    )?;
    Ok(window)
}

/// クライアント領域が 640×400(96 DPI 基準)になる、枠を含めたウィンドウの大きさ。
fn main_window_size(dpi: u32) -> platform::Result<(i32, i32)> {
    let client = RECT {
        left: 0,
        top: 0,
        right: platform::scale_for_dpi(MAIN_CLIENT_WIDTH, dpi),
        bottom: platform::scale_for_dpi(MAIN_CLIENT_HEIGHT, dpi),
    };
    let outer = platform::adjust_window_rect(client, MAIN_STYLE, MAIN_EX_STYLE, dpi)?;
    Ok((
        outer.right.saturating_sub(outer.left),
        outer.bottom.saturating_sub(outer.top),
    ))
}

/// メインウィンドウのメッセージ処理。
fn handle_main_message(window: Window, message: Message) -> Option<isize> {
    match message.id() {
        WM_DPICHANGED => {
            // Per-Monitor V2: OS が提案する矩形にそのまま合わせる。
            let rect = message.dpi_changed()?.suggested_rect;
            // 合わせられなくても、元の大きさのまま使える。
            let _ = window.set_pos(
                None,
                rect.left,
                rect.top,
                rect.right.saturating_sub(rect.left),
                rect.bottom.saturating_sub(rect.top),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            Some(0)
        }
        WM_DESTROY => {
            platform::post_quit(0);
            Some(0)
        }
        _ => None,
    }
}
