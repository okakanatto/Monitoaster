//! モニターの一覧(EnumDisplayMonitors)。オーバーレイをモニターごとに作るため(spec.md 9章)。
//!
//! 座標は物理ピクセル(Per-Monitor V2)。

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::{BOOL, Error, Result};

/// モニター1台。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    /// モニター全体の矩形(画面座標)。
    pub rect: RECT,
    /// タスクバーなどを除いた作業領域(画面座標)。
    pub work_rect: RECT,
    /// 主モニターなら true。
    pub primary: bool,
}

/// つながっているモニターの一覧。順番は OS が返した順。
pub fn monitors() -> Result<Vec<Monitor>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: コールバックには handles を指すポインターを渡し、呼び出しの間 handles は生きている。
    // EnumDisplayMonitors は同期で列挙し、戻った後はコールバックを呼ばない。
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&raw mut handles) as isize),
        )
    };
    if !ok.as_bool() {
        return Err(Error::from_thread());
    }
    let mut list = Vec::with_capacity(handles.len());
    for handle in handles {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: info は cbSize を設定した MONITORINFO で、呼び出しの間生きている。
        // 列挙の後で外されたモニターなら失敗するだけ。
        if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
            list.push(Monitor {
                rect: info.rcMonitor,
                work_rect: info.rcWork,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
    }
    Ok(list)
}

/// EnumDisplayMonitors のコールバック。モニターのハンドルを集める。
///
/// # Safety
///
/// `data` は `Vec<HMONITOR>` を指し、列挙の間ほかから使われないこと。
unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: monitors() が渡した、生きている Vec<HMONITOR> へのポインター。
    let handles = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    handles.push(monitor);
    true.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一覧を取れて、主モニターは多くても1台。どのモニターも幅と高さがある。
    /// CI の仮想マシンでは画面がないこともあるので、台数は問わない。
    #[test]
    fn lists_monitors_with_at_most_one_primary() {
        let list = monitors().expect("一覧");
        assert!(list.iter().filter(|m| m.primary).count() <= 1);
        for monitor in &list {
            assert!(monitor.rect.right > monitor.rect.left);
            assert!(monitor.rect.bottom > monitor.rect.top);
        }
    }
}
