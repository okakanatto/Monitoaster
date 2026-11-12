//! 画面の取り込み(spec.md 9章: スクリーンDCから GDI の BitBlt)。
//!
//! メモリ DC と 32 ビットの DIB セクションを1組持ち、同じ大きさの取り込みに使い回す(8章)。
//! 画素は上から下、1画素4バイト(B, G, R, X)、1行は `幅 * 4` バイトで詰め物なし。
//! detect の `Frame` にそのまま渡せる。X(4バイト目)は不定。
//!
//! S-03: 画素はメモリ上だけにある。[`ScreenCapture`] を捨てると DIB セクションごと消える。

use std::ffi::c_void;

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ROP_CODE,
    ReleaseDC, SRCCOPY, SelectObject,
};
use windows::core::{Error, Result};

/// 画面の矩形を BGRA で取り込む道具。
pub struct ScreenCapture {
    dc: HDC,
    bitmap: HBITMAP,
    /// DIB セクションを選択する前に dc に入っていたビットマップ。消す前に戻す。
    previous: HGDIOBJ,
    /// DIB セクションの画素の先頭。bitmap を消すまで有効。
    bits: *const u8,
    width: i32,
    height: i32,
    len: usize,
}

impl ScreenCapture {
    /// 幅 `width`、高さ `height`(物理ピクセル)の取り込み先を用意する。
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let len = buffer_len(width, height).ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        // SAFETY: None を渡すと、画面と互換のメモリ DC を作る。下で消す。
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(Error::from_thread());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // 負の高さで、上から下の並びにする。
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: info は初期化済みで、呼び出しの間生きている。bits は書き込めるポインター。
        // DIB_RGB_COLORS なので DC は使われない。
        let bitmap =
            match unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) } {
                Ok(bitmap) if !bits.is_null() => bitmap,
                Ok(bitmap) => {
                    // SAFETY: 上で作ったビットマップと DC を消す。どちらも他では使っていない。
                    unsafe {
                        let _ = DeleteObject(bitmap.into());
                        let _ = DeleteDC(dc);
                    }
                    return Err(Error::from_hresult(E_INVALIDARG));
                }
                Err(error) => {
                    // SAFETY: 上で作った DC を消す。
                    let _ = unsafe { DeleteDC(dc) };
                    return Err(error);
                }
            };
        // SAFETY: dc と bitmap は上で作ったもの。前に入っていたものは Drop で戻す。
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        Ok(Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast::<u8>(),
            width,
            height,
            len,
        })
    }

    /// 幅(物理ピクセル)。
    pub fn width(&self) -> i32 {
        self.width
    }

    /// 高さ(物理ピクセル)。
    pub fn height(&self) -> i32 {
        self.height
    }

    /// 画面座標 (`left`, `top`) から幅×高さを取り込み、画素を返す。
    ///
    /// `capture_blt` が true なら CAPTUREBLT を付ける(レイヤードウィンドウも含めるための旗。
    /// DWM の下での効き方は M0 の技術検証で確かめる)。画面の外の部分は不定。
    pub fn capture(&mut self, left: i32, top: i32, capture_blt: bool) -> Result<&[u8]> {
        let rop = if capture_blt {
            ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0)
        } else {
            SRCCOPY
        };
        // SAFETY: None を渡すと画面全体の DC を返す。下で ReleaseDC する。
        let screen = unsafe { GetDC(None) };
        if screen.is_invalid() {
            return Err(Error::from_thread());
        }
        // SAFETY: 転送先は自分のメモリ DC(DIB セクションを選択済み)、転送元は画面の DC。
        // 大きさは DIB セクションと同じ。
        let copied = unsafe {
            BitBlt(
                self.dc,
                0,
                0,
                self.width,
                self.height,
                Some(screen),
                left,
                top,
                rop,
            )
        };
        // SAFETY: 上で GetDC(None) した DC を返す。
        unsafe { ReleaseDC(None, screen) };
        copied?;
        // GDI はまとめて後で描くことがあるので、画素を読む前に書き終えさせる。
        // SAFETY: 引数なし。
        let _ = unsafe { GdiFlush() };
        // SAFETY: bits は DIB セクションの画素で、len バイトある(new で確かめた大きさ)。
        // bitmap は self が消すまで生きていて、書き込むのは &mut self の BitBlt だけなので、
        // 返す借用の間は変わらない。
        Ok(unsafe { std::slice::from_raw_parts(self.bits, self.len) })
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        // SAFETY: new で作った DC とビットマップ。選択を戻してから、ビットマップ、DC の順に消す。
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// 画素のバイト数(幅 × 高さ × 4)。幅と高さが正で、i32 と usize に収まるときだけ。
fn buffer_len(width: i32, height: i32) -> Option<usize> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let len = width.checked_mul(height)?.checked_mul(4)?;
    usize::try_from(len).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_len_rejects_empty_and_overflow() {
        assert_eq!(buffer_len(200, 100), Some(80_000));
        assert_eq!(buffer_len(0, 100), None);
        assert_eq!(buffer_len(100, -1), None);
        assert_eq!(buffer_len(i32::MAX, 2), None);
        assert_eq!(buffer_len(40_000, 20_000), None);
    }

    /// 取り込み先を作り、画面から取り込める(画面がない環境では BitBlt の失敗も許す)。
    #[test]
    fn captures_into_buffer_of_expected_size() {
        let mut capture = ScreenCapture::new(16, 8).expect("取り込み先");
        assert_eq!((capture.width(), capture.height()), (16, 8));
        if let Ok(pixels) = capture.capture(0, 0, false) {
            assert_eq!(pixels.len(), 16 * 8 * 4);
        }
        assert!(ScreenCapture::new(0, 8).is_err());
    }
}
