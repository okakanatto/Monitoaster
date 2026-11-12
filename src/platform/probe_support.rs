//! 検証用 exe(src/bin/m0_probe.rs)だけが使う小物。M0 の完了時に、検証用 exe と一緒に削除する
//! (docs/m0.md 作業7)。
//!
//! 製品(Monitoaster.exe)からは使わない。リンク時の最適化で製品の exe には入らない。

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::{Error, Result};

use super::Window;
use super::graphics::{
    Driver, DriverPreference, Stage, attach_visual, create_composition_device,
    create_composition_target, create_d2d_context, create_d2d_device, create_d2d_factory,
    create_d3d_device, create_dxgi_factory, create_swap_chain, create_target_bitmap, dxgi_device,
    feature_level_numbers,
};

/// コンソールから切り離す(FreeConsole)。
///
/// 検証用 exe はコンソールサブシステム(--headless で標準出力に書くため)。画面で使うときは、
/// エクスプローラーから起動したときに開くコンソールの窓を閉じる。窓を閉じられてプロセスごと
/// 終わると、トレイのアイコンが残るため。
pub fn detach_console() {
    // コンソールがない(すでに切り離した)ときの失敗は気にしない。
    // SAFETY: 引数なし。
    let _ = unsafe { FreeConsole() };
}

/// このプロセスのメモリの使用量(バイト)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryInfo {
    /// 作業セット(物理メモリに載っている分)。
    pub working_set: usize,
    /// 作業セットの最大値。
    pub peak_working_set: usize,
    /// プライベート使用量(コミットしたプライベートなメモリ)。
    pub private_bytes: usize,
}

/// このプロセスのメモリの使用量(K32GetProcessMemoryInfo。GetCurrentProcess の疑似ハンドル)。
pub fn memory_info() -> Result<MemoryInfo> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: 疑似ハンドル(閉じなくてよい)と、大きさを cb に入れた書き込める構造体を渡す。
    // PROCESS_MEMORY_COUNTERS_EX は PROCESS_MEMORY_COUNTERS を先頭に含む(大きさは cb で伝わる)。
    unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    }
    .ok()?;
    Ok(MemoryInfo {
        working_set: counters.WorkingSetSize,
        peak_working_set: counters.PeakWorkingSetSize,
        private_bytes: counters.PrivateUsage,
    })
}

/// 段の結果。
#[derive(Clone, Debug, PartialEq)]
pub enum StageOutcome {
    /// 通った。
    Done,
    /// 失敗した。
    Failed(Error),
    /// 前の段が失敗したので試していない。
    NotRun,
}

/// [`staged_graphics_init`] の結果。
#[derive(Clone, Debug)]
pub struct StagedInit {
    /// 試した順の、段とその結果。
    pub stages: Vec<(Stage, StageOutcome)>,
    /// D3D11 デバイスを作れたドライバー。
    pub driver: Option<Driver>,
    /// D3D の機能レベル(11_0 なら (11, 0))。
    pub feature_level: Option<(u32, u32)>,
    /// ハードウェアでの D3D11 デバイスの作成に失敗した理由(試していなければ None)。
    pub hardware_error: Option<Error>,
}

/// 描画の初期化を1段ずつ試し、各段の成否を返す(検証項目7)。作ったものは戻る前にすべて捨てる。
///
/// 順序: D3D11 デバイス → IDXGIDevice → CreateDXGIFactory2 → CreateSwapChainForComposition
/// (64×64)→ DCompositionCreateDevice → `window` への CreateTargetForHwnd → ビジュアル → Commit →
/// D2D1CreateFactory → ID2D1Device → ID2D1DeviceContext → バックバッファの描画先 → 透明で1回描く →
/// Present。ある段が失敗しても、それに頼らない段は続けて試す。
pub fn staged_graphics_init(preference: DriverPreference, window: Window) -> StagedInit {
    let mut stages = Vec::new();
    let (device, driver, feature_level, hardware_error) = match create_d3d_device(preference) {
        Ok(d3d) => {
            stages.push((Stage::D3dDevice, StageOutcome::Done));
            (
                Some(d3d.device),
                Some(d3d.driver),
                Some(feature_level_numbers(d3d.feature_level)),
                d3d.hardware_error,
            )
        }
        Err(error) => {
            stages.push((Stage::D3dDevice, StageOutcome::Failed(error.last)));
            (None, None, None, error.hardware)
        }
    };
    let mut log = StageLog(&mut stages);
    let dxgi = log.step(Stage::DxgiDevice, device.as_ref().map(dxgi_device));
    let factory = log.step(Stage::DxgiFactory, Some(create_dxgi_factory()));
    let swap_chain = log.step(
        Stage::SwapChain,
        factory
            .as_ref()
            .zip(device.as_ref())
            .map(|(factory, device)| create_swap_chain(factory, device, 64, 64)),
    );
    let composition = log.step(
        Stage::CompositionDevice,
        dxgi.as_ref().map(create_composition_device),
    );
    let target = log.step(
        Stage::CompositionTarget,
        composition
            .as_ref()
            .map(|composition| create_composition_target(composition, window.hwnd())),
    );
    let visual = log.step(
        Stage::CompositionVisual,
        composition
            .as_ref()
            .zip(target.as_ref())
            .zip(swap_chain.as_ref())
            .map(|((composition, target), swap_chain)| {
                attach_visual(composition, target, swap_chain)
            }),
    );
    log.step(
        Stage::Commit,
        composition
            .as_ref()
            .filter(|_| visual.is_some())
            // SAFETY: 引数なし。
            .map(|composition| unsafe { composition.Commit() }),
    );
    let d2d_factory = log.step(Stage::D2dFactory, Some(create_d2d_factory()));
    let d2d_device = log.step(
        Stage::D2dDevice,
        d2d_factory
            .as_ref()
            .zip(dxgi.as_ref())
            .map(|(factory, dxgi)| create_d2d_device(factory, dxgi)),
    );
    let context = log.step(
        Stage::D2dContext,
        d2d_device.as_ref().map(create_d2d_context),
    );
    let bitmap = log.step(
        Stage::TargetBitmap,
        context
            .as_ref()
            .zip(swap_chain.as_ref())
            .map(|(context, swap_chain)| create_target_bitmap(context, swap_chain)),
    );
    let drawn = log.step(
        Stage::Draw,
        context
            .as_ref()
            .zip(bitmap.as_ref())
            .map(|(context, bitmap)| {
                let transparent = D2D1_COLOR_F::default();
                // SAFETY: context と bitmap は有効。BeginDraw と EndDraw をここで対にし、終わったら
                // 描画先を外す(バックバッファの参照を、スワップチェーンより先に手放すため)。
                unsafe {
                    context.SetTarget(bitmap);
                    context.BeginDraw();
                    context.Clear(Some(&transparent));
                    let result = context.EndDraw(None, None);
                    context.SetTarget(None::<&windows::Win32::Graphics::Direct2D::ID2D1Image>);
                    result
                }
            }),
    );
    log.step(
        Stage::Present,
        swap_chain
            .as_ref()
            .filter(|_| drawn.is_some())
            // SAFETY: 値を渡すだけ。
            .map(|swap_chain| unsafe { swap_chain.Present(1, DXGI_PRESENT(0)) }.ok()),
    );
    // 作ったものは、ここで参照する側から順に捨てる。
    drop((bitmap, context, d2d_device, d2d_factory));
    drop((
        visual,
        target,
        composition,
        swap_chain,
        factory,
        dxgi,
        device,
    ));
    StagedInit {
        stages,
        driver,
        feature_level,
        hardware_error,
    }
}

/// 段の結果を記録する道具。
struct StageLog<'a>(&'a mut Vec<(Stage, StageOutcome)>);

impl StageLog<'_> {
    /// `result` が None なら「試していない」、Some なら成否を記録し、成功した値を返す。
    fn step<T>(&mut self, stage: Stage, result: Option<Result<T>>) -> Option<T> {
        let (outcome, value) = match result {
            None => (StageOutcome::NotRun, None),
            Some(Ok(value)) => (StageOutcome::Done, Some(value)),
            Some(Err(error)) => (StageOutcome::Failed(error), None),
        };
        self.0.push((stage, outcome));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_info_reports_nonzero_working_set() {
        let info = memory_info().expect("メモリの使用量");
        assert!(info.working_set > 0);
        assert!(info.peak_working_set >= info.working_set);
    }
}
