//! 通知領域(トレイ)のアイコンと、バルーン通知(spec.md 9章: Shell_NotifyIcon)。
//!
//! トースト通知の登録(スタートメニューへのショートカット作成)を避けるため、バルーン通知を使う。
//! Windows 10 以降では、バルーン通知はトーストの見た目で出る。
//!
//! アイコンは [`TrayIcon`] を捨てると消す(NIM_DELETE)。消さずにプロセスが終わると、マウスを
//! 重ねるまでアイコンが残るため、終了の前に必ず捨てる。

use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{IDI_APPLICATION, LoadIconW};
use windows::core::{Error, Result};

use super::Window;

/// 通知領域のアイコン。捨てると消す。
#[derive(Debug)]
pub struct TrayIcon {
    hwnd: HWND,
    id: u32,
}

impl TrayIcon {
    /// 通知領域にアイコンを足す(NIM_ADD、NIM_SETVERSION で NOTIFYICON_VERSION_4)。
    ///
    /// アイコンへのマウス操作は `callback_message`(`WM_APP` 以降)で `window` に届く。
    /// `id` は `window` の中でアイコンを区別する番号。`tip` はマウスを重ねたときの文字(127文字まで)。
    /// アイコンの絵は、今はシステムのアプリの既定(M1 で実行時に作る絵に置き換える)。
    /// エクスプローラーが動いていないと失敗する。
    pub fn add(window: Window, id: u32, callback_message: u32, tip: &str) -> Result<Self> {
        // SAFETY: システムのアイコンを読み込む(hinstance に None)。共有のアイコンなので消さない。
        let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }?;
        let mut data = base_data(window.hwnd(), id);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = callback_message;
        data.hIcon = icon;
        copy_wide(&mut data.szTip, tip);
        notify_icon(NIM_ADD, &data)?;
        let tray = Self {
            hwnd: window.hwnd(),
            id,
        };
        let mut version = base_data(tray.hwnd, id);
        version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // 失敗したら tray の Drop でアイコンを消す。
        notify_icon(NIM_SETVERSION, &version)?;
        Ok(tray)
    }

    /// バルーン通知を1件出す(NIM_MODIFY、NIF_INFO)。題は63文字まで、本文は255文字まで。
    ///
    /// 「応答不可」(集中モード)のときは出ないことがある。
    pub fn notify(&self, title: &str, body: &str) -> Result<()> {
        let mut data = base_data(self.hwnd, self.id);
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, body);
        notify_icon(NIM_MODIFY, &data)
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        // 消せなくても、ほかにできることはない。
        let _ = notify_icon(NIM_DELETE, &base_data(self.hwnd, self.id));
    }
}

/// どの操作でも要る部分(大きさ、ウィンドウ、番号)だけを埋めた NOTIFYICONDATAW。
fn base_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    }
}

fn notify_icon(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) -> Result<()> {
    // SAFETY: data は cbSize を設定した NOTIFYICONDATAW で、呼び出しの間生きている。
    // 文字列はすべて固定長の配列の中にあり、NUL 終端している(copy_wide)。
    if unsafe { Shell_NotifyIconW(message, data) }.as_bool() {
        Ok(())
    } else {
        // Shell_NotifyIconW は最後のエラーを設定しないことがある。
        Err(Error::from_hresult(E_FAIL))
    }
}

/// `text` を UTF-16 にして `dest` に写す。入らない分は切り捨て、必ず NUL で終える。
///
/// サロゲートペアの途中では切らない。
fn copy_wide(dest: &mut [u16], text: &str) {
    let Some(capacity) = dest.len().checked_sub(1) else {
        return;
    };
    let mut len = 0;
    for unit in text.encode_utf16().take(capacity) {
        dest[len] = unit;
        len += 1;
    }
    // 最後が上位サロゲートなら、対になる下位サロゲートが入らなかった。
    if len > 0 && (0xD800..0xDC00).contains(&dest[len - 1]) {
        len -= 1;
    }
    dest[len] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 入らない分は切り捨て、必ず NUL で終える。
    #[test]
    fn copy_wide_terminates_and_truncates() {
        let mut dest = [0xFFFFu16; 4];
        copy_wide(&mut dest, "ab");
        assert_eq!(&dest[..3], &[0x61, 0x62, 0]);

        let mut dest = [0xFFFFu16; 4];
        copy_wide(&mut dest, "検知しました");
        assert_eq!(super::super::from_wide(&dest), "検知し");
        assert_eq!(dest[3], 0);
    }

    /// サロゲートペアの途中では切らない。
    #[test]
    fn copy_wide_does_not_split_surrogate_pair() {
        // 🍞 は UTF-16 で2単位。NUL を除いて2単位の枠には「a」しか入らない。
        let mut dest = [0xFFFFu16; 3];
        copy_wide(&mut dest, "a🍞");
        assert_eq!(super::super::from_wide(&dest), "a");
        assert_eq!(dest[1], 0);

        // 3単位の枠なら丸ごと入る。
        let mut dest = [0xFFFFu16; 4];
        copy_wide(&mut dest, "a🍞");
        assert_eq!(super::super::from_wide(&dest), "a🍞");
    }

    /// 写し先が空なら何もしない(添字の範囲外に書かない)。
    #[test]
    fn copy_wide_ignores_empty_destination() {
        let mut dest: [u16; 0] = [];
        copy_wide(&mut dest, "a");
    }
}
