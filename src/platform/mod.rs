//! Win32 と COM の安全なラッパー。unsafe を書ける唯一の場所(S-11)。
//!
//! - 公開する関数と型はすべて安全(unsafe でない)にする。前提が崩れたときも、OS が失敗を返す
//!   だけで済む形にする。
//! - unsafe ブロックには、その前提を `// SAFETY:` コメントで書く(lint で強制)。
//! - 薄く保つ。判定や計算などのロジックは OS 非依存のモジュールに置く。
//!
//! エラーは windows クレートの [`Error`](HRESULT を持つ)で返す。呼び出し側は画面に出すときに
//! `code()` を使う。

#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

mod capture;
mod graphics;
mod instance;
mod monitor;
pub mod probe_support;
mod tray;
mod window;

use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};

pub use capture::ScreenCapture;
pub use graphics::{
    AdapterInfo, Canvas, CompositionSurface, Driver, DriverPreference, GraphicsDevice, Stage,
    StageError,
};
pub use instance::{Instance, InstanceLock, acquire_instance};
pub use monitor::{Monitor, monitors};
pub use tray::TrayIcon;
pub use window::{
    BASE_DPI, Command, ControlClass, DpiChange, Font, Message, Paint, Pump, Rgb, Window,
    WindowClass, WindowId, WindowSpec, adjust_window_rect, create_control, create_window,
    foreground_window, post_quit, pump_messages_for, run_message_loop, scale_for_dpi, wait_message,
};
pub use windows::core::{Error, Result};

/// 動的に読み込む DLL の検索先を System32 に限る(S-09)。起動直後、ほかの処理より先に呼ぶ。
///
/// 静的にインポートする DLL は、リンカーの /DEPENDENTLOADFLAG:0x800 で同じように限っている
/// (build.rs)。起動後のこの設定は、起動前に読み込まれる DLL には効かないため。
pub fn restrict_dll_search() -> Result<()> {
    // SAFETY: 定数のフラグを渡すだけで、ポインターを渡さない。
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }
}

/// Win32 に渡す、NUL 終端の UTF-16 文字列に変換する。
///
/// 途中に NUL があれば、Win32 はそこまでを文字列とみなす(メモリの安全には影響しない)。
pub fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Win32 から受け取った UTF-16 を文字列に変換する。
///
/// 最初の NUL の手前までを使う(NUL がなければ全体)。不正なサロゲートは U+FFFD に置き換える。
pub fn from_wide(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日本語の文字列は、UTF-16 との往復で変わらない(NUL で終わる)。
    #[test]
    fn wide_round_trip_keeps_japanese() {
        let wide = to_wide("監視を開始");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(from_wide(&wide), "監視を開始");
    }

    /// 最初の NUL の手前までを文字列にする(NUL がなければ全体)。
    #[test]
    fn from_wide_stops_at_first_nul() {
        let wide = [0x41, 0x42, 0, 0x43];
        assert_eq!(from_wide(&wide), "AB");
        assert_eq!(from_wide(&[0x41, 0x42]), "AB");
    }

    /// S-09: 検索先の限定が通る(Windows 10 以降は必ずある API)。
    #[test]
    fn restrict_dll_search_succeeds() {
        assert!(restrict_dll_search().is_ok());
    }
}
