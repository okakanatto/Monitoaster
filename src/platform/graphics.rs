//! 描画デバイスと、ウィンドウに重ねる描画面(spec.md 9章: DirectComposition のスワップチェーンに
//! Direct2D で描く)。
//!
//! - [`GraphicsDevice`]: D3D11 デバイス(D3D_DRIVER_TYPE_HARDWARE、だめなら WARP)、DXGI
//!   ファクトリー、DirectComposition デバイス、Direct2D デバイス。演出の開始時に作り、終了時に
//!   捨てる(8章)。描画に使うアダプターは [`GraphicsDevice::adapter`] で分かる(GPU のない環境では
//!   D3D_DRIVER_TYPE_HARDWARE でも Microsoft Basic Render Driver になり、描画はソフトウェアで行われる)。
//! - [`CompositionSurface`]: ウィンドウ1つ分の描画面。composition 用のスワップチェーン
//!   (B8G8R8A8、プリマルチプライドα、FLIP_SEQUENTIAL、2枚)、DirectComposition のターゲット
//!   (最前面)とビジュアル、Direct2D のデバイスコンテキストとブラシ。
//!
//! # 寿命と解放の順序
//!
//! - COM の参照で数えるので、どの順に捨てても解放済みのものに触れることはない。描画面は、
//!   デバイスのうち自分が使う部分の参照を自分でも持つ。
//! - 推奨の順序: 描画を止める → ウィンドウを破棄する → 描画面を捨てる → デバイスを捨てる。
//! - 描画面を捨てるときは、デバイスコンテキストの描画先を外してから、バックバッファの参照を手放す。
//! - 描画([`CompositionSurface::draw`])や表示([`CompositionSurface::present`])が失敗したら
//!   (デバイスの消失など)、その描画面とデバイスを捨てる。作り直すか諦めるか(E-06)は呼び出し側が
//!   決める。
//!
//! # スレッド
//!
//! Direct2D のファクトリーは SINGLE_THREADED。作ったスレッドだけで使う。windows クレートは
//! Direct2D と DXGI のインターフェースを Send + Sync にしているので、ここの型には
//! [`SingleThread`] を持たせて、Send でも Sync でもなくする(ほかのスレッドから同時に描けないように)。

use std::marker::PhantomData;

use windows::Win32::Foundation::{E_POINTER, HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEBUG_LEVEL_NONE, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device,
    ID2D1DeviceContext, ID2D1Factory1, ID2D1Image, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2,
    IDXGISurface, IDXGISwapChain1,
};
use windows::core::{Error, Interface, Result};

use super::{Window, from_wide};

/// Send でも Sync でもなくするための印(Direct2D の SINGLE_THREADED を守る)。
type SingleThread = PhantomData<*const ()>;

/// どのドライバーで D3D11 デバイスを作るか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverPreference {
    /// D3D_DRIVER_TYPE_HARDWARE を試し、だめなら WARP(ソフトウェア)。
    HardwareThenWarp,
    /// WARP だけ。
    WarpOnly,
}

/// D3D11 デバイスを作れたドライバー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// D3D_DRIVER_TYPE_HARDWARE(既定のアダプター。GPU のない環境では Microsoft Basic Render Driver
    /// になる)。GPU で描くとは限らないので、アダプターは [`GraphicsDevice::adapter`] で確かめる。
    Hardware,
    /// WARP(CPU で描くソフトウェアのドライバー)。
    Warp,
}

/// 描画の準備の段。失敗した段を伝えるのに使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// D3D11CreateDevice。
    D3dDevice,
    /// D3D11 デバイスから IDXGIDevice を得る。
    DxgiDevice,
    /// CreateDXGIFactory2。
    DxgiFactory,
    /// IDXGIFactory2::CreateSwapChainForComposition。
    SwapChain,
    /// DCompositionCreateDevice。
    CompositionDevice,
    /// IDCompositionDevice::CreateTargetForHwnd。
    CompositionTarget,
    /// ビジュアルを作り、スワップチェーンを中身にして、ターゲットの根に置く。
    CompositionVisual,
    /// IDCompositionDevice::Commit。
    Commit,
    /// D2D1CreateFactory。
    D2dFactory,
    /// ID2D1Factory1::CreateDevice。
    D2dDevice,
    /// ID2D1Device::CreateDeviceContext(とブラシ)。
    D2dContext,
    /// バックバッファから Direct2D の描画先を作る。
    TargetBitmap,
    /// BeginDraw〜EndDraw。
    Draw,
    /// IDXGISwapChain::Present。
    Present,
}

/// 描画の準備に失敗した段と、その理由。
#[derive(Clone, Debug, PartialEq)]
pub struct StageError {
    /// 失敗した段。
    pub stage: Stage,
    /// 理由(HRESULT)。
    pub error: Error,
}

impl StageError {
    fn at(stage: Stage) -> impl FnOnce(Error) -> Self {
        move |error| Self { stage, error }
    }
}

/// D3D11 デバイスを作った結果。
pub(super) struct D3dDevice {
    pub(super) device: ID3D11Device,
    pub(super) driver: Driver,
    pub(super) feature_level: D3D_FEATURE_LEVEL,
    /// WARP に落ちたときの、ハードウェアでの失敗の理由。
    pub(super) hardware_error: Option<Error>,
}

/// D3D11 デバイスを作れなかった理由。
pub(super) struct D3dError {
    /// ハードウェアを試したときの失敗の理由。
    pub(super) hardware: Option<Error>,
    /// 最後に試したドライバーでの失敗の理由。
    pub(super) last: Error,
}

/// D3D11 デバイスを、`preference` に従ってハードウェアか WARP で作る。Direct2D 用に BGRA を有効にする。
pub(super) fn create_d3d_device(
    preference: DriverPreference,
) -> std::result::Result<D3dDevice, D3dError> {
    let mut hardware_error = None;
    if preference == DriverPreference::HardwareThenWarp {
        match create_d3d_device_with(D3D_DRIVER_TYPE_HARDWARE) {
            Ok((device, feature_level)) => {
                return Ok(D3dDevice {
                    device,
                    driver: Driver::Hardware,
                    feature_level,
                    hardware_error: None,
                });
            }
            Err(error) => hardware_error = Some(error),
        }
    }
    match create_d3d_device_with(D3D_DRIVER_TYPE_WARP) {
        Ok((device, feature_level)) => Ok(D3dDevice {
            device,
            driver: Driver::Warp,
            feature_level,
            hardware_error,
        }),
        Err(last) => Err(D3dError {
            hardware: hardware_error,
            last,
        }),
    }
}

fn create_d3d_device_with(driver: D3D_DRIVER_TYPE) -> Result<(ID3D11Device, D3D_FEATURE_LEVEL)> {
    let mut device = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    // SAFETY: 出力先は書き込めるローカル変数で、呼び出しの間生きている。アダプターは既定(None)、
    // ソフトウェアのモジュールは使わない(HMODULE は NULL)。機能レベルは既定の並び(None)。
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            Some(&raw mut feature_level),
            None,
        )
    }?;
    let device = device.ok_or_else(|| Error::from_hresult(E_POINTER))?;
    Ok((device, feature_level))
}

/// D3D11 デバイスから IDXGIDevice を得る。
pub(super) fn dxgi_device(device: &ID3D11Device) -> Result<IDXGIDevice> {
    device.cast()
}

/// 描画に使うアダプター(DXGI_ADAPTER_DESC1 の一部)。
///
/// GPU のない仮想マシンやリモートデスクトップでは、D3D_DRIVER_TYPE_HARDWARE で作っても
/// Microsoft Basic Render Driver(VendorId 0x1414、DeviceId 0x8C)になり、ソフトウェアで描く。
/// 主アダプターが Basic Render Driver のときは `software_flag` が立たないので、ID でも見分ける。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    /// アダプターの説明(GPU の製品名など。例: 「Microsoft Basic Render Driver」)。
    pub description: String,
    /// PCI のベンダー ID(Microsoft は 0x1414)。
    pub vendor_id: u32,
    /// PCI のデバイス ID。
    pub device_id: u32,
    /// DXGI_ADAPTER_FLAG_SOFTWARE が立っているか。
    pub software_flag: bool,
}

/// D3D11 デバイスが使っているアダプターの情報(IDXGIDevice::GetAdapter → IDXGIAdapter1::GetDesc1)。
pub(super) fn adapter_info(device: &ID3D11Device) -> Result<AdapterInfo> {
    let dxgi = dxgi_device(device)?;
    // SAFETY: dxgi は有効な COM のインターフェース。返るアダプターは参照を持つ。
    let adapter: IDXGIAdapter1 = unsafe { dxgi.GetAdapter() }?.cast()?;
    // SAFETY: adapter は有効な COM のインターフェース。説明は値として受け取る。
    let desc = unsafe { adapter.GetDesc1() }?;
    Ok(AdapterInfo {
        description: from_wide(&desc.Description).trim().to_string(),
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        software_flag: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
    })
}

/// DXGI のファクトリー(CreateDXGIFactory2)。
pub(super) fn create_dxgi_factory() -> Result<IDXGIFactory2> {
    // SAFETY: フラグを渡すだけ(デバッグ層は使わない)。
    unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
}

/// composition 用のスワップチェーン。B8G8R8A8、プリマルチプライドα、FLIP_SEQUENTIAL、バッファ2枚。
pub(super) fn create_swap_chain(
    factory: &IDXGIFactory2,
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<IDXGISwapChain1> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    };
    // SAFETY: desc は呼び出しの間生きている。device は有効な D3D11 デバイス。出力の制限はなし。
    unsafe { factory.CreateSwapChainForComposition(device, &desc, None) }
}

/// DirectComposition のデバイス(DCompositionCreateDevice)。
pub(super) fn create_composition_device(dxgi: &IDXGIDevice) -> Result<IDCompositionDevice> {
    // SAFETY: dxgi は有効な IDXGIDevice。
    unsafe { DCompositionCreateDevice(dxgi) }
}

/// ウィンドウに重ねる DirectComposition のターゲット。ウィンドウの子より手前(topmost)に置く。
pub(super) fn create_composition_target(
    composition: &IDCompositionDevice,
    hwnd: HWND,
) -> Result<IDCompositionTarget> {
    // SAFETY: composition は有効な COM のインターフェース。hwnd は値として渡すだけで、ポインター
    // としては読まれない。ウィンドウはトップレベルでも子でもよい。呼び出したプロセスのウィンドウで
    // ないとき(DCOMPOSITION_ERROR_ACCESS_DENIED)、無効なハンドルのとき、topmost のターゲットが
    // すでにあるときは、失敗を返すだけ。
    unsafe { composition.CreateTargetForHwnd(hwnd, true) }
}

/// ビジュアルを作り、`swap_chain` を中身にして、`target` の根に置く。
pub(super) fn attach_visual(
    composition: &IDCompositionDevice,
    target: &IDCompositionTarget,
    swap_chain: &IDXGISwapChain1,
) -> Result<IDCompositionVisual> {
    // SAFETY: どれも有効な COM のインターフェース。ビジュアルとターゲットは中身の参照を持つ。
    unsafe {
        let visual = composition.CreateVisual()?;
        visual.SetContent(swap_chain)?;
        target.SetRoot(&visual)?;
        Ok(visual)
    }
}

/// Direct2D のファクトリー(SINGLE_THREADED、デバッグ層なし)。
pub(super) fn create_d2d_factory() -> Result<ID2D1Factory1> {
    let options = D2D1_FACTORY_OPTIONS {
        debugLevel: D2D1_DEBUG_LEVEL_NONE,
    };
    // SAFETY: options は呼び出しの間生きている。
    unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&options)) }
}

/// Direct2D のデバイス(DXGI デバイスの上に作る)。
pub(super) fn create_d2d_device(
    factory: &ID2D1Factory1,
    dxgi: &IDXGIDevice,
) -> Result<ID2D1Device> {
    // SAFETY: どちらも有効な COM のインターフェース。
    unsafe { factory.CreateDevice(dxgi) }
}

/// Direct2D のデバイスコンテキスト。
pub(super) fn create_d2d_context(device: &ID2D1Device) -> Result<ID2D1DeviceContext> {
    // SAFETY: 値を渡すだけ。
    unsafe { device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
}

/// スワップチェーンのバックバッファ(0番)から、Direct2D の描画先にするビットマップを作る。
///
/// FLIP 系のスワップチェーンでは、0番は常にその時点のバックバッファを指すので、作り直さずに使い続けられる。
pub(super) fn create_target_bitmap(
    context: &ID2D1DeviceContext,
    swap_chain: &IDXGISwapChain1,
) -> Result<ID2D1Bitmap1> {
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // 96 DPI にして、Direct2D の座標(DIP)を物理ピクセルと同じにする。
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        ..Default::default()
    };
    // SAFETY: どちらも有効な COM のインターフェース。properties は呼び出しの間生きていて、
    // colorContext は None(参照を持たない)。
    unsafe {
        let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
        context.CreateBitmapFromDxgiSurface(&surface, Some(&properties))
    }
}

/// 描画デバイス。演出の開始時に作り、終了時に捨てる(8章)。
///
/// フィールドは、参照する側から順に並べている(捨てる順になる)。
pub struct GraphicsDevice {
    d2d: ID2D1Device,
    composition: IDCompositionDevice,
    factory: IDXGIFactory2,
    d3d: ID3D11Device,
    driver: Driver,
    feature_level: D3D_FEATURE_LEVEL,
    hardware_error: Option<Error>,
    _single_thread: SingleThread,
}

impl GraphicsDevice {
    /// 描画デバイスを作る。失敗したら、その段と理由を返す(E-06 の判断に使う)。
    ///
    /// 順に D3D11 デバイス → IDXGIDevice → DXGI ファクトリー → DirectComposition デバイス →
    /// Direct2D のファクトリー → Direct2D デバイスを作る。
    pub fn create(preference: DriverPreference) -> std::result::Result<Self, StageError> {
        let d3d = create_d3d_device(preference).map_err(|error| StageError {
            stage: Stage::D3dDevice,
            error: error.last,
        })?;
        let dxgi = dxgi_device(&d3d.device).map_err(StageError::at(Stage::DxgiDevice))?;
        let factory = create_dxgi_factory().map_err(StageError::at(Stage::DxgiFactory))?;
        let composition =
            create_composition_device(&dxgi).map_err(StageError::at(Stage::CompositionDevice))?;
        let d2d_factory = create_d2d_factory().map_err(StageError::at(Stage::D2dFactory))?;
        // Direct2D のデバイスがファクトリーの参照を持つので、ファクトリーはここで手放してよい。
        let d2d =
            create_d2d_device(&d2d_factory, &dxgi).map_err(StageError::at(Stage::D2dDevice))?;
        Ok(Self {
            d2d,
            composition,
            factory,
            d3d: d3d.device,
            driver: d3d.driver,
            feature_level: d3d.feature_level,
            hardware_error: d3d.hardware_error,
            _single_thread: PhantomData,
        })
    }

    /// 作れたドライバー。
    pub fn driver(&self) -> Driver {
        self.driver
    }

    /// 描画に使うアダプター。[`Driver::Hardware`] でも、GPU のない環境では
    /// Microsoft Basic Render Driver(ソフトウェア)になる。
    pub fn adapter(&self) -> Result<AdapterInfo> {
        adapter_info(&self.d3d)
    }

    /// D3D の機能レベル(11_0 なら `(11, 0)`)。
    pub fn feature_level(&self) -> (u32, u32) {
        feature_level_numbers(self.feature_level)
    }

    /// WARP に落ちたときの、ハードウェアでの失敗の理由。
    pub fn hardware_error(&self) -> Option<&Error> {
        self.hardware_error.as_ref()
    }

    /// DirectComposition の変更(ビジュアルの付け替えなど)を画面に反映する。
    pub fn commit(&self) -> Result<()> {
        // SAFETY: 引数なし。
        unsafe { self.composition.Commit() }
    }
}

/// D3D_FEATURE_LEVEL の値(0xb000 など)を (11, 0) の形にする。
pub(super) fn feature_level_numbers(level: D3D_FEATURE_LEVEL) -> (u32, u32) {
    let value = level.0 as u32;
    ((value >> 12) & 0xF, (value >> 8) & 0xF)
}

/// ウィンドウ1つ分の描画面。ウィンドウ全体を覆う DirectComposition のビジュアルに、
/// スワップチェーンを置く。
///
/// フィールドは、参照する側から順に並べている(捨てる順になる)。
pub struct CompositionSurface {
    brush: ID2D1SolidColorBrush,
    context: ID2D1DeviceContext,
    // 下の3つは、持っておくだけ(描画面を捨てるまで生かしておくため)。
    _target_bitmap: ID2D1Bitmap1,
    _visual: IDCompositionVisual,
    _target: IDCompositionTarget,
    swap_chain: IDXGISwapChain1,
    width: u32,
    height: u32,
    _single_thread: SingleThread,
}

impl CompositionSurface {
    /// `window` に重ねる、幅 `width`、高さ `height`(物理ピクセル)の描画面を作る。
    ///
    /// 作った時点では中身は透明(まだ1度も Present していない)。1つのウィンドウに作れる描画面は1つ。
    pub fn create(
        device: &GraphicsDevice,
        window: Window,
        width: u32,
        height: u32,
    ) -> std::result::Result<Self, StageError> {
        let swap_chain = create_swap_chain(&device.factory, &device.d3d, width, height)
            .map_err(StageError::at(Stage::SwapChain))?;
        let target = create_composition_target(&device.composition, window.hwnd())
            .map_err(StageError::at(Stage::CompositionTarget))?;
        let visual = attach_visual(&device.composition, &target, &swap_chain)
            .map_err(StageError::at(Stage::CompositionVisual))?;
        let context = create_d2d_context(&device.d2d).map_err(StageError::at(Stage::D2dContext))?;
        let target_bitmap = create_target_bitmap(&context, &swap_chain)
            .map_err(StageError::at(Stage::TargetBitmap))?;
        let white = D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        // SAFETY: context と target_bitmap は有効。色は呼び出しの間生きている。
        let brush = unsafe {
            context.SetTarget(&target_bitmap);
            context.CreateSolidColorBrush(&white, None)
        }
        .map_err(StageError::at(Stage::D2dContext))?;
        device.commit().map_err(StageError::at(Stage::Commit))?;
        Ok(Self {
            brush,
            context,
            _target_bitmap: target_bitmap,
            _visual: visual,
            _target: target,
            swap_chain,
            width,
            height,
            _single_thread: PhantomData,
        })
    }

    /// 幅と高さ(物理ピクセル)。
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// バックバッファに描く(BeginDraw → `paint` → EndDraw)。画面に出すには [`Self::present`] を呼ぶ。
    ///
    /// EndDraw の失敗(D2DERR_RECREATE_TARGET など)を返す。失敗したら描画面を作り直す。
    pub fn draw(&mut self, paint: impl FnOnce(&Canvas<'_>)) -> Result<()> {
        // SAFETY: 描画先はこの描画面のバックバッファ。BeginDraw と EndDraw はここで対にする。
        unsafe { self.context.BeginDraw() };
        paint(&Canvas {
            context: &self.context,
            brush: &self.brush,
            _single_thread: PhantomData,
        });
        // SAFETY: 上の BeginDraw と対になる。タグは要らない(None)。
        unsafe { self.context.EndDraw(None, None) }
    }

    /// 描いた内容を画面に出す(Present(1, 0)。次の垂直同期まで待つことがある)。
    ///
    /// 隠れていて表示されない(DXGI_STATUS_OCCLUDED)のは成功として扱う。
    pub fn present(&self) -> Result<()> {
        // SAFETY: 値を渡すだけ。
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) }.ok()
    }
}

impl Drop for CompositionSurface {
    fn drop(&mut self) {
        // バックバッファの参照を、スワップチェーンより先に手放す。
        // SAFETY: 描画中ではない(draw の外。&mut self を取れている)。None で描画先を外す。
        unsafe { self.context.SetTarget(None::<&ID2D1Image>) };
    }
}

/// [`CompositionSurface::draw`] の中で使う描画の道具。座標は物理ピクセル、色はストレートα
/// (Direct2D がプリマルチプライドにする)。
pub struct Canvas<'a> {
    context: &'a ID2D1DeviceContext,
    brush: &'a ID2D1SolidColorBrush,
    _single_thread: SingleThread,
}

impl Canvas<'_> {
    /// 全体を `color` で塗りつぶす(透明にするなら a = 0)。
    pub fn clear(&self, color: D2D1_COLOR_F) {
        // SAFETY: 描画中(BeginDraw の後)。color は呼び出しの間生きている。
        unsafe { self.context.Clear(Some(&color)) };
    }

    /// 矩形を塗る。
    pub fn fill_rect(&self, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        // SAFETY: 描画中。rect と color は呼び出しの間生きている。
        unsafe {
            self.brush.SetColor(&color);
            self.context.FillRectangle(&rect, self.brush);
        }
    }

    /// 楕円を塗る。中心 (`center_x`, `center_y`)、半径 `radius_x`、`radius_y`。
    pub fn fill_ellipse(
        &self,
        center_x: f32,
        center_y: f32,
        radius_x: f32,
        radius_y: f32,
        color: D2D1_COLOR_F,
    ) {
        let mut ellipse = D2D1_ELLIPSE {
            radiusX: radius_x,
            radiusY: radius_y,
            ..Default::default()
        };
        ellipse.point.X = center_x;
        ellipse.point.Y = center_y;
        // SAFETY: 描画中。ellipse と color は呼び出しの間生きている。
        unsafe {
            self.brush.SetColor(&color);
            self.context.FillEllipse(&ellipse, self.brush);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_11_0};

    /// D3D_FEATURE_LEVEL の値を、主と副の番号(11_0 なら (11, 0))に分ける。
    #[test]
    fn feature_level_numbers_split_major_minor() {
        assert_eq!(feature_level_numbers(D3D_FEATURE_LEVEL_11_0), (11, 0));
        assert_eq!(feature_level_numbers(D3D_FEATURE_LEVEL_9_1), (9, 1));
    }
}
