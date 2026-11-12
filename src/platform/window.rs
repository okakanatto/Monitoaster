//! ウィンドウの土台: ウィンドウクラス、ウィンドウと子コントロールの作成、メッセージ処理、
//! メッセージループ、タイマー、フォント、DPI。
//!
//! # メッセージ処理の決まり
//!
//! - [`create_window`] に渡した処理(クロージャ)はウィンドウが持つ。`GWLP_USERDATA` に置き、
//!   `WM_NCDESTROY` で手放す。処理が `None` を返したメッセージは `DefWindowProcW` に回す。
//! - 処理の中から同じウィンドウに入れ子でメッセージが届いたとき(処理の中で [`Window::destroy`]
//!   や [`Window::set_pos`] を呼んだときなど)は、処理を二重に呼ばず `DefWindowProcW` に回す。
//!   `&mut` の二重借用を避けるため。入れ子で届いたメッセージは処理からは見えない。
//! - `WM_NCCREATE` より前に届くメッセージ(`WM_GETMINMAXINFO` など)は、処理がまだ結び付いて
//!   いないので `DefWindowProcW` に回す。処理が最初に受け取るのは `WM_NCCREATE`。
//! - 処理が持つ値は、ウィンドウが破棄され、処理の呼び出しがすべて終わったときに解放する。
//!   処理の中で自分のウィンドウを破棄してもよい。
//! - ただし、処理の中で自分のウィンドウを破棄すると、入れ子で届く `WM_DESTROY` と `WM_NCDESTROY`
//!   は処理からは見えない(`DefWindowProcW` に回る)。終了の処理([`post_quit`] など)を
//!   `WM_DESTROY` に置いている場合は、処理の外で破棄するか、処理の中で破棄した後に自分で行うこと
//!   (「終了」のコマンドの処理の中で破棄すると、`WM_DESTROY` に置いた [`post_quit`] が呼ばれない)。
//! - `WM_COMMAND` の lParam(送ってきたコントロール)は、自分の子であることを確かめてから
//!   [`Command::control`] にする。`WM_COMMAND` はほかのプロセスからも任意の lParam で送れるため
//!   (S-04: ほかのプロセスのウィンドウは [`WindowId`] でしか渡さない)。
//! - 複数のウィンドウの処理が値を共有する場合(`Rc<RefCell<_>>` など)、入れ子の呼び出しで
//!   借用が重なりうる。共有する側でも `try_borrow_mut` などで守ること。
//!
//! 座標と大きさは物理ピクセル(spec.md 9章、Per-Monitor V2)。

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::{without_provenance, without_provenance_mut};
use std::rc::Rc;

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, E_FAIL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK,
    DeleteObject, DrawTextW, EndPaint, FillRect, HBRUSH, HFONT, InvalidateRect, PAINTSTRUCT,
    SYS_COLOR_INDEX, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, EM_REPLACESEL, EM_SETLIMITTEXT, EM_SETSEL,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect,
    GetForegroundWindow, GetMessageW, GetWindowDisplayAffinity, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HMENU, IDC_ARROW, IsChild,
    KillTimer, LWA_ALPHA, LoadCursorW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    NONCLIENTMETRICSW, PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT, RegisterClassExW,
    SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SPI_GETNONCLIENTMETRICS, SendMessageW,
    SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_DISPLAY_AFFINITY,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DPICHANGED, WM_NCCREATE, WM_NCDESTROY, WM_QUIT,
    WM_SETFONT, WM_TIMER, WNDCLASSEXW, WS_CHILD,
};
use windows::core::{Error, PCWSTR, Result};

use super::{from_wide, to_wide};

/// 基準の DPI(拡大率100%)。
pub const BASE_DPI: u32 = 96;

/// 96 DPI 基準の長さを `dpi` に合わせて拡大縮小する。
///
/// 端数は四捨五入する(0.5 は 0 から遠い方へ。Win32 の MulDiv と同じ)。i32 に収まらなければ
/// 端の値にする。
pub fn scale_for_dpi(value: i32, dpi: u32) -> i32 {
    let base = i64::from(BASE_DPI);
    let product = i64::from(value) * i64::from(dpi);
    let half = base / 2;
    let rounded = if product >= 0 {
        (product + half) / base
    } else {
        (product - half) / base
    };
    i32::try_from(rounded).unwrap_or(if rounded < 0 { i32::MIN } else { i32::MAX })
}

/// クライアント領域の矩形から、枠を含めたウィンドウの矩形を求める。
///
/// 枠の太さは `dpi` に合わせる(AdjustWindowRectExForDpi)。メニューなしのウィンドウ向け。
pub fn adjust_window_rect(
    client: RECT,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    dpi: u32,
) -> Result<RECT> {
    let mut rect = client;
    // SAFETY: rect は書き込める RECT で、呼び出しの間生きている。
    unsafe { AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, dpi) }?;
    Ok(rect)
}

/// この exe のモジュールハンドル。
fn module_instance() -> Result<HINSTANCE> {
    // SAFETY: None を渡すと、呼び出したプロセスの exe のハンドルを返す。参照カウントは増えない。
    unsafe { GetModuleHandleW(None) }.map(HINSTANCE::from)
}

/// 登録済みのウィンドウクラス。プロセスが終わるまで登録したままにする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowClass {
    atom: u16,
}

impl WindowClass {
    /// ウィンドウクラスを登録する。メッセージは [`create_window`] に渡した処理に届く。
    ///
    /// `background` は背景を塗るシステム色(`COLOR_BTNFACE` など)。`None` なら背景を塗らない
    /// (全面を自前で描くウィンドウ向け)。カーソルは矢印。
    /// 同じ名前は1つのプロセスで1回だけ登録できる。2回目はエラーになる。
    pub fn register(name: &str, background: Option<SYS_COLOR_INDEX>) -> Result<Self> {
        let instance = module_instance()?;
        let name = to_wide(name);
        // SAFETY: IDC_ARROW はシステム定義のカーソルの番号。hinstance に None を渡すと
        // システムのカーソルを読み込む。共有のカーソルなので解放しない。
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
        // システム色の番号+1 をブラシとして渡すと、その色で背景を塗る(WNDCLASSEXW の決まり)。
        let brush = background.map_or(HBRUSH::default(), |color| {
            HBRUSH(without_provenance_mut((color.0 as usize).wrapping_add(1)))
        });
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            hbrBackground: brush,
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: class は初期化済みで、クラス名は呼び出しの間生きている(OS が複製する)。
        // window_proc は WNDPROC の形をしている。
        let atom = unsafe { RegisterClassExW(&class) };
        if atom == 0 {
            return Err(Error::from_thread());
        }
        Ok(Self { atom })
    }
}

/// ウィンドウを作るときの指定。座標と大きさは物理ピクセル。
///
/// トップレベルのウィンドウでは `x`、`y`、`width`、`height` に `CW_USEDEFAULT` も使える。
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowSpec<'a> {
    /// 拡張スタイル(`WS_EX_NOACTIVATE`、`WS_EX_TOPMOST` など)。
    pub ex_style: WINDOW_EX_STYLE,
    /// スタイル(`WS_OVERLAPPED`、`WS_POPUP`、`WS_VISIBLE` など)。
    pub style: WINDOW_STYLE,
    /// タイトル。ボタンやエディットでは表示する文字。
    pub title: &'a str,
    /// 左上の x 座標。子ウィンドウでは親のクライアント座標。
    pub x: i32,
    /// 左上の y 座標。子ウィンドウでは親のクライアント座標。
    pub y: i32,
    /// 幅。
    pub width: i32,
    /// 高さ。
    pub height: i32,
    /// 子ウィンドウなら親、トップレベルのウィンドウなら持ち主(なくてよい)。
    pub parent: Option<Window>,
}

/// ウィンドウのメッセージを処理するクロージャ。`None` を返すと `DefWindowProcW` に回す。
type Handler = dyn FnMut(Window, Message) -> Option<isize>;

/// ウィンドウが持つ状態。`Rc` で数え、ウィンドウと処理中の呼び出しがそれぞれ1つずつ持つ。
struct WindowState {
    /// WM_NCCREATE でウィンドウに結び付けたら true。
    attached: Cell<bool>,
    handler: RefCell<Box<Handler>>,
}

/// [`WindowClass`] のウィンドウを作る。`handler` がこのウィンドウのメッセージを処理する。
///
/// メッセージ処理の決まりはモジュールの説明を参照。`WS_VISIBLE` を付けなければ非表示で作る。
pub fn create_window(
    class: WindowClass,
    spec: &WindowSpec<'_>,
    handler: impl FnMut(Window, Message) -> Option<isize> + 'static,
) -> Result<Window> {
    let instance = module_instance()?;
    let title = to_wide(spec.title);
    let state = Rc::new(WindowState {
        attached: Cell::new(false),
        handler: RefCell::new(Box::new(handler)),
    });
    // ウィンドウに渡す強参照。WM_NCCREATE で受け取ったら、ウィンドウの持ち物になる。
    let param = Rc::into_raw(Rc::clone(&state));
    // SAFETY: クラスは登録で得た atom で指定する(MAKEINTATOM)。title は呼び出しの間生きている。
    // param は Rc::into_raw で得たポインターで、state が別に強参照を持つので呼び出しの間は生きている。
    let created = unsafe {
        CreateWindowExW(
            spec.ex_style,
            PCWSTR(without_provenance::<u16>(usize::from(class.atom))),
            PCWSTR(title.as_ptr()),
            spec.style,
            spec.x,
            spec.y,
            spec.width,
            spec.height,
            spec.parent.map(Window::hwnd),
            None,
            Some(instance),
            Some(param.cast::<c_void>()),
        )
    };
    if !state.attached.get() {
        // WM_NCCREATE まで進まなかった。ウィンドウに渡すはずだった強参照を取り戻す。
        // SAFETY: param は上の Rc::into_raw で得たもので、ウィンドウには渡っていない。
        drop(unsafe { Rc::from_raw(param) });
    }
    // 作成に失敗した場合でも、WM_NCCREATE まで進んでいれば WM_NCDESTROY で手放している。
    created.map(Window)
}

/// [`WindowClass`] のウィンドウのプロシージャ。
///
/// # Safety
///
/// OS がメッセージとともに呼ぶ。`WM_NCCREATE` の `lparam` は `CREATESTRUCTW` を指していること。
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: WM_NCCREATE の lparam は、OS が用意した CREATESTRUCTW を指す。
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let param = create.lpCreateParams as *const WindowState;
        if !param.is_null() {
            // SAFETY: このクラスのウィンドウは create_window だけが作り、lpCreateParams には
            // Rc::into_raw で得たポインターを渡している。create_window が別に強参照を持つので生きている。
            unsafe { (*param).attached.set(true) };
            // SAFETY: hwnd は今作っている自分のスレッドのウィンドウ。強参照1つをウィンドウに預ける。
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize) };
        }
    }

    // SAFETY: hwnd は OS から渡された、このスレッドのウィンドウ。
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowState;
    if raw.is_null() {
        // WM_NCCREATE より前(WM_GETMINMAXINFO など)か、WM_NCDESTROY の後。
        // SAFETY: OS から受け取った引数をそのまま渡す。
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // この呼び出しの間 state を生かしておくため、強参照を1つ持つ。入れ子で WM_NCDESTROY が
    // 届いてウィンドウが強参照を手放しても、外側の呼び出しが持つ分で生きている。
    // SAFETY: raw は Rc::into_raw で得たポインターで、ウィンドウが強参照を1つ持っている。
    let state = unsafe {
        Rc::increment_strong_count(raw);
        Rc::from_raw(raw)
    };

    // SAFETY: OS から受け取ったメッセージの引数そのもの。
    let message = unsafe { Message::from_raw(hwnd, msg, wparam, lparam) };
    // 処理を呼ぶのは借用できたときだけ。入れ子で届いたメッセージ(借用中)は既定の処理に回す。
    let result = match state.handler.try_borrow_mut() {
        Ok(mut handler) => handler(Window(hwnd), message),
        Err(_) => None,
    };

    if msg == WM_NCDESTROY {
        // ウィンドウが持っていた強参照を手放す。以降このウィンドウのメッセージは既定の処理に回る。
        // SAFETY: hwnd は破棄中の自分のウィンドウ。
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        // SAFETY: raw はウィンドウが持っていた強参照で、ここで1回だけ手放す。
        drop(unsafe { Rc::from_raw(raw) });
    }

    let result = match result {
        Some(value) => LRESULT(value),
        // SAFETY: OS から受け取った引数をそのまま渡す。
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    // 最後の強参照ならここで処理(クロージャ)を解放する。借用はすでに終わっている。
    drop(state);
    result
}

/// ウィンドウに届いたメッセージ。platform のウィンドウプロシージャだけが作る。
///
/// ポインターを含む引数は、作るときに値として読み取っておく(処理が終わったあとに読まないため)。
#[derive(Clone, Copy, Debug)]
pub struct Message {
    id: u32,
    wparam: usize,
    lparam: isize,
    /// WM_DPICHANGED のときだけ、lparam が指す提案の矩形。
    suggested_rect: Option<RECT>,
    /// WM_COMMAND のときだけ、lparam が自分の子のコントロールなら、そのハンドル([`own_child`])。
    control: Option<HWND>,
}

impl Message {
    /// # Safety
    ///
    /// ウィンドウプロシージャが受け取った引数そのものであること(`hwnd` はメッセージを受け取った
    /// 自分のウィンドウ)。`WM_DPICHANGED` の `lparam` は RECT を指していること。
    unsafe fn from_raw(hwnd: HWND, id: u32, wparam: WPARAM, lparam: LPARAM) -> Self {
        let suggested_rect = if id == WM_DPICHANGED && lparam.0 != 0 {
            // SAFETY: WM_DPICHANGED の lparam は、OS が用意した RECT を指す(呼び出し側の前提)。
            Some(unsafe { (lparam.0 as *const RECT).read_unaligned() })
        } else {
            None
        };
        // WM_COMMAND はほかのプロセスからも任意の lparam で送れる。自分の子だけを受け取る(S-04)。
        let control = if id == WM_COMMAND && lparam.0 != 0 {
            own_child(hwnd, lparam.0)
        } else {
            None
        };
        Self {
            id,
            wparam: wparam.0,
            lparam: lparam.0,
            suggested_rect,
            control,
        }
    }

    /// メッセージの番号(`WM_COMMAND` など)。
    pub fn id(&self) -> u32 {
        self.id
    }

    /// wParam。
    pub fn wparam(&self) -> usize {
        self.wparam
    }

    /// lParam。ポインターを含むメッセージでも、ここから先を読む手段は用意しない。
    ///
    /// ハンドルを含むメッセージでも、この値から [`Window`] を作る手段は用意しない
    /// (`WM_COMMAND` のコントロールは [`Message::command`] で、確かめたものだけを受け取る)。
    pub fn lparam(&self) -> isize {
        self.lparam
    }

    /// `WM_COMMAND` なら、その中身。
    pub fn command(&self) -> Option<Command> {
        if self.id != WM_COMMAND {
            return None;
        }
        Some(Command {
            id: low_word(self.wparam),
            code: high_word(self.wparam),
            control: self.control.map(Window),
        })
    }

    /// `WM_TIMER` なら、タイマーの番号([`Window::set_timer`] に渡したもの)。
    pub fn timer_id(&self) -> Option<usize> {
        (self.id == WM_TIMER).then_some(self.wparam)
    }

    /// `WM_DPICHANGED` なら、新しい DPI と、OS が提案するウィンドウの矩形。
    pub fn dpi_changed(&self) -> Option<DpiChange> {
        if self.id != WM_DPICHANGED {
            return None;
        }
        Some(DpiChange {
            dpi: u32::from(low_word(self.wparam)),
            suggested_rect: self.suggested_rect?,
        })
    }
}

/// `WM_COMMAND` の中身。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    /// コントロールの番号([`create_control`] に渡したもの)、またはメニューの番号。
    pub id: u16,
    /// 通知コード(`BN_CLICKED`、`EN_CHANGE` など)。メニューなら 0。
    pub code: u16,
    /// 送ってきたコントロール。メニューやアクセラレーターなら `None`。
    ///
    /// lParam が自分の子(子孫を含む)でなければ `None`(ほかのプロセスが任意の lParam で送って
    /// きた場合など。S-04)。
    pub control: Option<Window>,
}

/// `candidate`(`WM_COMMAND` の lParam)が `parent` の子(子孫を含む)で、このスレッドのウィンドウなら、
/// そのハンドル。そうでなければ `None`。
///
/// IsChild だけでは、ほかのプロセスが自分のウィンドウを SetParent で `parent` の子にした場合を
/// 除けないので、ウィンドウを作ったスレッドも比べる(子コントロールは [`create_control`] が
/// 親と同じスレッドで作る)。
fn own_child(parent: HWND, candidate: isize) -> Option<HWND> {
    let child = HWND(without_provenance_mut(candidate as usize));
    // SAFETY: ハンドルを2つ渡すだけで、ポインターとしては読まれない。無効なハンドルなら偽が返る。
    if !unsafe { IsChild(parent, child) }.as_bool() {
        return None;
    }
    // SAFETY: ハンドルを渡すだけ。プロセス ID は受け取らない(None)。無効なハンドルなら 0 が返る。
    let owner = unsafe { GetWindowThreadProcessId(child, None) };
    // SAFETY: 引数なし。
    let current = unsafe { GetCurrentThreadId() };
    (owner != 0 && owner == current).then_some(child)
}

/// `WM_DPICHANGED` の中身。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DpiChange {
    /// 新しい DPI。
    pub dpi: u32,
    /// OS が提案するウィンドウの矩形(画面座標、物理ピクセル)。Per-Monitor V2 ではこれに合わせる。
    pub suggested_rect: RECT,
}

fn low_word(value: usize) -> u16 {
    (value & 0xFFFF) as u16
}

fn high_word(value: usize) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

/// 子コントロールにできるシステムのクラス。
///
/// 作成時に追加のデータ(lpParam)を要らないものだけを並べる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlClass {
    /// ボタン、チェックボックス、ラジオボタン(`BUTTON`)。
    Button,
    /// 入力欄(`EDIT`)。
    Edit,
    /// 文字や枠の表示(`STATIC`)。
    Static,
}

impl ControlClass {
    fn name(self) -> &'static str {
        match self {
            Self::Button => "BUTTON",
            Self::Edit => "EDIT",
            Self::Static => "STATIC",
        }
    }
}

/// 子コントロールを作る。`spec.parent` が要る。
///
/// `spec.style` に `WS_CHILD` を足して作る(表示するなら `WS_VISIBLE` も指定する)。
/// 通知は親の `WM_COMMAND` に、`id` を付けて届く([`Message::command`])。
pub fn create_control(class: ControlClass, id: u16, spec: &WindowSpec<'_>) -> Result<Window> {
    let instance = module_instance()?;
    let class_name = to_wide(class.name());
    let title = to_wide(spec.title);
    // SAFETY: 文字列は呼び出しの間生きている。WS_CHILD のとき hMenu は子ウィンドウの番号として
    // 扱われ、ポインターとしては読まれない。これらのクラスは lpParam を使わない。
    unsafe {
        CreateWindowExW(
            spec.ex_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            spec.style | WS_CHILD,
            spec.x,
            spec.y,
            spec.width,
            spec.height,
            spec.parent.map(Window::hwnd),
            Some(HMENU(without_provenance_mut(usize::from(id)))),
            Some(instance),
            None,
        )
    }
    .map(Window)
}

/// ウィンドウのハンドル。所有しない(コピーしても、捨てても、ウィンドウは閉じない)。
///
/// すでに破棄されたウィンドウを指していても、各操作は OS が失敗を返すだけで安全。
/// 操作はウィンドウを作ったスレッドから行う(タイマーや破棄は、ほかのスレッドからは失敗する)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window(HWND);

impl Window {
    /// Win32 のハンドル。platform の中で、ほかの API に渡すために使う。
    pub fn hwnd(self) -> HWND {
        self.0
    }

    /// 識別子(ハンドルの値)。[`foreground_window`] と比べるときに使う。
    pub fn id(self) -> WindowId {
        WindowId(self.0)
    }

    /// ウィンドウが置かれたモニターの DPI。取れなければ [`BASE_DPI`]。
    pub fn dpi(self) -> u32 {
        // SAFETY: HWND を渡すだけ。無効なら 0 が返る。
        match unsafe { GetDpiForWindow(self.0) } {
            0 => BASE_DPI,
            dpi => dpi,
        }
    }

    /// クライアント領域の矩形(クライアント座標。左上は 0, 0)。
    pub fn client_rect(self) -> Result<RECT> {
        let mut rect = RECT::default();
        // SAFETY: rect は書き込める RECT で、呼び出しの間生きている。
        unsafe { GetClientRect(self.0, &mut rect) }?;
        Ok(rect)
    }

    /// 枠を含めたウィンドウの矩形(画面座標)。
    pub fn window_rect(self) -> Result<RECT> {
        let mut rect = RECT::default();
        // SAFETY: rect は書き込める RECT で、呼び出しの間生きている。
        unsafe { GetWindowRect(self.0, &mut rect) }?;
        Ok(rect)
    }

    /// 位置、大きさ、重なり順を変える(SetWindowPos)。
    ///
    /// `insert_after` には `HWND_TOPMOST` なども使える。変えない項目は `flags`
    /// (`SWP_NOMOVE`、`SWP_NOSIZE`、`SWP_NOZORDER` など)で指定する。
    pub fn set_pos(
        self,
        insert_after: Option<HWND>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: SET_WINDOW_POS_FLAGS,
    ) -> Result<()> {
        // SAFETY: ハンドルと値を渡すだけで、ポインターを渡さない。
        unsafe { SetWindowPos(self.0, insert_after, x, y, width, height, flags) }
    }

    /// 表示の状態を変える(ShowWindow)。前に表示されていたら true。
    pub fn show(self, command: SHOW_WINDOW_CMD) -> bool {
        // SAFETY: ハンドルと値を渡すだけ。
        unsafe { ShowWindow(self.0, command) }.as_bool()
    }

    /// ウィンドウを破棄する(DestroyWindow)。子ウィンドウもまとめて破棄される。
    ///
    /// 自分のウィンドウの処理の中から呼んでもよい。ただしそのとき、このウィンドウの `WM_DESTROY` と
    /// `WM_NCDESTROY` は処理に届かない(モジュールの説明を参照)。
    pub fn destroy(self) -> Result<()> {
        // SAFETY: ハンドルを渡すだけ。処理の再入はウィンドウプロシージャが守る。
        unsafe { DestroyWindow(self.0) }
    }

    /// タイトル(ボタンやエディットでは表示する文字)を変える。
    pub fn set_text(self, text: &str) -> Result<()> {
        let text = to_wide(text);
        // SAFETY: text は NUL 終端の UTF-16 で、呼び出しの間生きている。
        unsafe { SetWindowTextW(self.0, PCWSTR(text.as_ptr())) }
    }

    /// タイトル(ボタンやエディットでは表示している文字)。取れなければ空。
    pub fn text(self) -> String {
        // SAFETY: ハンドルを渡すだけ。
        let len = unsafe { GetWindowTextLengthW(self.0) };
        let Ok(len) = usize::try_from(len) else {
            return String::new();
        };
        if len == 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len + 1];
        // SAFETY: buffer は書き込める領域で、長さは呼び出しに渡る。
        let copied = unsafe { GetWindowTextW(self.0, &mut buffer) };
        let copied = usize::try_from(copied).unwrap_or(0).min(buffer.len());
        from_wide(&buffer[..copied])
    }

    /// フォントを設定し、描き直す(WM_SETFONT)。主に子コントロールに使う。
    ///
    /// `font` は、このウィンドウが使っている間は生かしておくこと。先に消すと、OS の既定の
    /// フォントで描かれる。
    pub fn set_font(self, font: &Font) {
        // SAFETY: WM_SETFONT の wparam はフォントのハンドル、lparam は描き直すかどうか。
        // どちらもポインターとしては読まれない。
        unsafe {
            SendMessageW(
                self.0,
                WM_SETFONT,
                Some(WPARAM(font.0.0.addr())),
                Some(LPARAM(1)),
            )
        };
    }

    /// クライアント領域全体を描き直す対象にする(InvalidateRect)。WM_PAINT が後で届く。
    pub fn invalidate(self) {
        // SAFETY: ハンドルだけを渡す。矩形は None(全体)。
        let _ = unsafe { InvalidateRect(Some(self.0), None, false) };
    }

    /// タイマーを設定する。`interval_ms` ごとに `WM_TIMER` が `id` 付きで届く。
    ///
    /// 同じ `id` のタイマーがあれば置き換える。
    pub fn set_timer(self, id: usize, interval_ms: u32) -> Result<()> {
        // SAFETY: ハンドルと値を渡すだけ。コールバックは使わない(WM_TIMER で受ける)。
        match unsafe { SetTimer(Some(self.0), id, interval_ms, None) } {
            0 => Err(Error::from_thread()),
            _ => Ok(()),
        }
    }

    /// タイマーを止める。
    pub fn kill_timer(self, id: usize) -> Result<()> {
        // SAFETY: ハンドルと値を渡すだけ。
        unsafe { KillTimer(Some(self.0), id) }
    }

    /// キーボードの入力先にする(SetFocus)。自分のスレッドのウィンドウにだけ効く。
    pub fn set_focus(self) {
        // 戻り値は前の入力先。前に入力先がなかったときも Err になるので見ない。
        // SAFETY: ハンドルを渡すだけ。
        let _ = unsafe { SetFocus(Some(self.0)) };
    }

    /// レイヤードウィンドウ(`WS_EX_LAYERED`)の不透明度を設定する
    /// (SetLayeredWindowAttributes、LWA_ALPHA)。255 で不透明。
    ///
    /// レイヤードウィンドウは、これか UpdateLayeredWindow を呼ぶまで画面に出ない。
    /// `WS_EX_LAYERED` でないウィンドウでは失敗する。
    pub fn set_layered_alpha(self, alpha: u8) -> Result<()> {
        // SAFETY: ハンドルと値を渡すだけ。
        unsafe { SetLayeredWindowAttributes(self.0, COLORREF(0), alpha, LWA_ALPHA) }
    }

    /// 画面の取り込みでの扱いを設定する(SetWindowDisplayAffinity)。
    ///
    /// `WDA_EXCLUDEFROMCAPTURE` で、BitBlt や画面共有に写らなくなる(Windows 10 2004 以降。
    /// E-05、F-10)。`WDA_NONE` で元に戻す。トップレベルの自分のウィンドウにだけ設定できる。
    /// DWM が合成していないときは失敗する。
    pub fn set_display_affinity(self, affinity: WINDOW_DISPLAY_AFFINITY) -> Result<()> {
        // SAFETY: ハンドルと値を渡すだけ。
        unsafe { SetWindowDisplayAffinity(self.0, affinity) }
    }

    /// 取り込みでの扱い(GetWindowDisplayAffinity)。
    pub fn display_affinity(self) -> Result<WINDOW_DISPLAY_AFFINITY> {
        let mut affinity = 0u32;
        // SAFETY: affinity は書き込める u32 で、呼び出しの間生きている。
        unsafe { GetWindowDisplayAffinity(self.0, &mut affinity) }?;
        Ok(WINDOW_DISPLAY_AFFINITY(affinity))
    }

    /// 描画を始める(BeginPaint)。`WM_PAINT` の処理の中で呼び、[`Paint`] を捨てると終わる。
    pub fn begin_paint(self) -> Paint {
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: paint は書き込める PAINTSTRUCT で、Paint が持って EndPaint に渡す。
        // 失敗すると hdc が NULL になり、以降の描画は何もしない。
        unsafe { BeginPaint(self.0, &mut paint) };
        Paint {
            hwnd: self.0,
            paint,
        }
    }

    /// エディットに入れられる文字数の上限を、なしにする(EM_SETLIMITTEXT 0)。
    ///
    /// 既定の上限(32767文字)はプログラムからの追加にも効くため、記録を溜めるエディットで使う。
    pub fn remove_text_limit(self) {
        // SAFETY: EM_SETLIMITTEXT の引数は数値で、ポインターとしては読まれない。
        unsafe { SendMessageW(self.0, EM_SETLIMITTEXT, Some(WPARAM(0)), Some(LPARAM(0))) };
    }

    /// エディットの末尾に文字を足し、そこまでスクロールする(EM_SETSEL、EM_REPLACESEL)。
    ///
    /// 複数行のエディットでは、改行を `"\r\n"` で渡す。
    pub fn append_text(self, text: &str) {
        // SAFETY: ハンドルを渡すだけ。
        let len = unsafe { GetWindowTextLengthW(self.0) }.max(0) as usize;
        let text = to_wide(text);
        // SAFETY: EM_SETSEL の引数は文字の位置(数値)。EM_REPLACESEL の lparam は NUL 終端の
        // UTF-16 を指し、text は同期の呼び出しの間生きている。wparam は「元に戻す」に積むか。
        unsafe {
            SendMessageW(
                self.0,
                EM_SETSEL,
                Some(WPARAM(len)),
                Some(LPARAM(len as isize)),
            );
            SendMessageW(
                self.0,
                EM_REPLACESEL,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
    }

    /// エディットの文字をすべて選ぶ(EM_SETSEL 0, -1)。
    pub fn select_all_text(self) {
        // SAFETY: EM_SETSEL の引数は文字の位置(数値)。
        unsafe { SendMessageW(self.0, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1))) };
    }

    /// チェックボックスに印が付いているか(BM_GETCHECK)。
    pub fn is_checked(self) -> bool {
        // SAFETY: 引数を持たないメッセージ。
        let state = unsafe { SendMessageW(self.0, BM_GETCHECK, None, None) };
        state.0 == BST_CHECKED.0 as isize
    }

    /// チェックボックスの印を付ける・外す(BM_SETCHECK)。
    pub fn set_checked(self, checked: bool) {
        let state = if checked { BST_CHECKED } else { BST_UNCHECKED };
        // SAFETY: BM_SETCHECK の wparam は状態(数値)。
        unsafe { SendMessageW(self.0, BM_SETCHECK, Some(WPARAM(state.0 as usize)), None) };
    }
}

/// ウィンドウの識別子(ハンドルの値)。ほかのプロセスのウィンドウかもしれない。
///
/// 位置と大きさを読むことと、比べることだけができる(S-04: 他のプロセスに触れない)。
/// 文字やタイトルを読む手段は用意しない(S-15)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId(HWND);

impl WindowId {
    /// ハンドルの値。表示と比較のためだけに使う。
    pub fn value(self) -> usize {
        self.0.0.addr()
    }

    /// 枠を含めたウィンドウの矩形(画面座標)。
    pub fn rect(self) -> Result<RECT> {
        let mut rect = RECT::default();
        // SAFETY: rect は書き込める RECT で、呼び出しの間生きている。読むだけの API。
        unsafe { GetWindowRect(self.0, &mut rect) }?;
        Ok(rect)
    }
}

/// 前面のウィンドウ(GetForegroundWindow)。ない(切り替えの途中など)ときは `None`。
pub fn foreground_window() -> Option<WindowId> {
    // SAFETY: 引数なし。
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(WindowId(hwnd))
}

/// GDI の色(赤、緑、青)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// 赤。
    pub r: u8,
    /// 緑。
    pub g: u8,
    /// 青。
    pub b: u8,
}

impl Rgb {
    fn colorref(self) -> COLORREF {
        COLORREF(u32::from(self.r) | (u32::from(self.g) << 8) | (u32::from(self.b) << 16))
    }
}

/// 描画中のウィンドウ(BeginPaint〜EndPaint)。捨てると EndPaint する。
pub struct Paint {
    hwnd: HWND,
    paint: PAINTSTRUCT,
}

impl Paint {
    /// 矩形を塗る(クライアント座標)。
    pub fn fill_rect(&self, rect: RECT, color: Rgb) {
        // SAFETY: 値を渡すだけ。作ったブラシは下で消す。
        let brush = unsafe { CreateSolidBrush(color.colorref()) };
        if brush.is_invalid() {
            return;
        }
        // SAFETY: hdc は BeginPaint が返したもの(NULL なら失敗するだけ)。rect は呼び出しの間生きている。
        unsafe { FillRect(self.paint.hdc, &rect, brush) };
        // SAFETY: brush は上で作り、どの DC にも選択していない。
        let _ = unsafe { DeleteObject(brush.into()) };
    }

    /// 文字を矩形の中に、左右中央で折り返して描く。背景は塗らない。
    pub fn draw_text(&self, text: &str, rect: RECT, color: Rgb, font: Option<&Font>) {
        let mut text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = rect;
        // SAFETY: hdc は BeginPaint が返したもの。font は呼び出しの間生きていて、描いた後で
        // 元のフォントに戻す。text と rect は呼び出しの間生きている(DT_MODIFYSTRING は使わない)。
        unsafe {
            let previous = font.map(|font| SelectObject(self.paint.hdc, font.0.into()));
            SetBkMode(self.paint.hdc, TRANSPARENT);
            SetTextColor(self.paint.hdc, color.colorref());
            DrawTextW(
                self.paint.hdc,
                &mut text,
                &mut rect,
                DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
            );
            if let Some(previous) = previous {
                SelectObject(self.paint.hdc, previous);
            }
        }
    }
}

impl Drop for Paint {
    fn drop(&mut self) {
        // SAFETY: self.paint は BeginPaint が埋めたもので、ここでだけ EndPaint に渡す。
        let _ = unsafe { EndPaint(self.hwnd, &self.paint) };
    }
}

/// GDI のフォント。Drop で消す。
#[derive(Debug)]
pub struct Font(HFONT);

impl Font {
    /// システムのメッセージフォント(docs/ux.md 12章)を、`dpi` に合わせた大きさで作る。
    ///
    /// DPI が変わったら作り直して設定し直す。
    pub fn message_font(dpi: u32) -> Result<Self> {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        // SAFETY: metrics は cbSize を設定した NONCLIENTMETRICSW で、呼び出しの間生きている。
        // uiparam にはその大きさを渡す。
        unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&raw mut metrics).cast::<c_void>()),
                0,
                dpi,
            )
        }?;
        // SAFETY: lfMessageFont は OS が埋めた LOGFONTW で、呼び出しの間生きている。
        let font = unsafe { CreateFontIndirectW(&metrics.lfMessageFont) };
        if font.is_invalid() {
            // CreateFontIndirectW は最後のエラーを設定しない。
            return Err(Error::from_hresult(E_FAIL));
        }
        Ok(Self(font))
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: self.0 は CreateFontIndirectW が返したフォントで、ここでだけ消す。
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

/// メッセージループを回す。`WM_QUIT` が来たら、[`post_quit`] に渡した終了コードを返す。
///
/// メッセージを取れなかったときは -1 を返す。
pub fn run_message_loop() -> i32 {
    let mut msg = MSG::default();
    loop {
        // SAFETY: msg は書き込める MSG で、呼び出しの間生きている。
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match status.0 {
            0 => return msg.wParam.0 as i32,
            -1 => return -1,
            _ => {
                // SAFETY: msg は GetMessageW が埋めたもの。
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

/// メッセージループを終わらせる(このスレッドに WM_QUIT を送る)。
pub fn post_quit(exit_code: i32) {
    // SAFETY: 値を渡すだけ。
    unsafe { PostQuitMessage(exit_code) };
}

/// [`pump_messages_for`] と [`wait_message`] の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pump {
    /// 続けてよい。
    Continue,
    /// WM_QUIT を受け取った([`post_quit`] に渡した終了コード)。WM_QUIT はキューから取り除かれている。
    /// 外側のループにも終わりを伝えるなら、もう一度 [`post_quit`] する。
    Quit(i32),
}

/// キューからメッセージを1つ取り出して処理する。なければ `None`。WM_QUIT は処理せずに `Quit` を返す。
fn dispatch_one(msg: &mut MSG) -> Option<Pump> {
    // SAFETY: msg は書き込める MSG で、呼び出しの間生きている。
    if !unsafe { PeekMessageW(msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        return None;
    }
    if msg.message == WM_QUIT {
        return Some(Pump::Quit(msg.wParam.0 as i32));
    }
    // SAFETY: msg は PeekMessageW が埋めたもの。
    unsafe {
        let _ = TranslateMessage(msg);
        DispatchMessageW(msg);
    }
    Some(Pump::Continue)
}

/// `duration_ms` が過ぎるまで、届いたメッセージを処理しながら待つ。
///
/// 処理の外(メッセージループの代わり)から呼ぶ。ウィンドウの処理の中から呼ぶと、そのウィンドウに
/// 届くメッセージは処理に入らず既定の処理に回る(モジュールの説明を参照)。
pub fn pump_messages_for(duration_ms: u32) -> Pump {
    let deadline = Instant::now() + Duration::from_millis(u64::from(duration_ms));
    let mut msg = MSG::default();
    loop {
        // 届き続けても待ち時間を守るよう、1つ処理するごとに期限を見る。
        while let Some(result) = dispatch_one(&mut msg) {
            if result != Pump::Continue {
                return result;
            }
            if Instant::now() >= deadline {
                return Pump::Continue;
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Pump::Continue;
        }
        let remaining_ms = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: ハンドルは渡さない。キューにメッセージが来るか、時間が過ぎるまで待つだけ。
        unsafe {
            MsgWaitForMultipleObjectsEx(None, remaining_ms.max(1), QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
    }
}

/// メッセージが1つ届くまで待ち、それを処理して戻る。
///
/// メッセージループの1回分。処理の外で別の仕事を挟むループ(検証用 exe など)に使う。
/// メッセージを取れなかったときは `Pump::Quit(-1)` を返す。
pub fn wait_message() -> Pump {
    let mut msg = MSG::default();
    // SAFETY: msg は書き込める MSG で、呼び出しの間生きている。
    match unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 {
        0 => Pump::Quit(msg.wParam.0 as i32),
        -1 => Pump::Quit(-1),
        _ => {
            // SAFETY: msg は GetMessageW が埋めたもの。
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            Pump::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        BN_CLICKED, IsWindow, WM_CLOSE, WM_CREATE, WM_DESTROY, WS_OVERLAPPEDWINDOW,
    };

    /// 96 DPI 基準の長さを、MulDiv と同じ四捨五入で拡大縮小する(i32 に収まらなければ端の値)。
    #[test]
    fn scale_for_dpi_rounds_like_muldiv() {
        assert_eq!(scale_for_dpi(640, 96), 640);
        assert_eq!(scale_for_dpi(640, 144), 960);
        assert_eq!(scale_for_dpi(400, 120), 500);
        // 1 × 144 / 96 = 1.5 → 2、-1.5 → -2(0 から遠い方へ)
        assert_eq!(scale_for_dpi(1, 144), 2);
        assert_eq!(scale_for_dpi(-1, 144), -2);
        // 1 × 120 / 96 = 1.25 → 1
        assert_eq!(scale_for_dpi(1, 120), 1);
        assert_eq!(scale_for_dpi(i32::MAX, 192), i32::MAX);
        assert_eq!(scale_for_dpi(i32::MIN, 192), i32::MIN);
    }

    /// wParam を下位と上位の16ビット(WM_COMMAND の番号と通知コード)に分ける。
    #[test]
    fn message_words_split_wparam() {
        assert_eq!(low_word(0x1234_5678), 0x5678);
        assert_eq!(high_word(0x1234_5678), 0x1234);
    }

    /// 解放されたら立つ旗。処理(クロージャ)に持たせ、解放の時期を調べる。
    struct DropFlag(Rc<Cell<bool>>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    fn hidden_spec() -> WindowSpec<'static> {
        WindowSpec {
            style: WS_OVERLAPPEDWINDOW,
            title: "test",
            width: 200,
            height: 100,
            ..Default::default()
        }
    }

    fn is_window(window: Window) -> bool {
        // SAFETY: ハンドルを渡すだけ。
        unsafe { IsWindow(Some(window.hwnd())) }.as_bool()
    }

    fn send(window: Window, msg: u32) -> LRESULT {
        // SAFETY: 引数を持たないメッセージを自分のスレッドのウィンドウに送るだけ。
        unsafe { SendMessageW(window.hwnd(), msg, None, None) }
    }

    /// 処理が最初に受け取るのは WM_NCCREATE。それより前のメッセージは既定の処理に回る。
    /// 外から破棄すると、処理は WM_DESTROY と WM_NCDESTROY を受け取り、その後で解放される。
    #[test]
    fn handler_sees_nccreate_first_and_is_freed_after_ncdestroy() {
        let class = WindowClass::register("Monitoaster.Test.Lifecycle", None).expect("登録");
        let log = Rc::new(RefCell::new(Vec::new()));
        let dropped = Rc::new(Cell::new(false));
        let flag = DropFlag(Rc::clone(&dropped));
        let seen = Rc::clone(&log);
        let window = create_window(class, &hidden_spec(), move |_, message| {
            let _ = &flag;
            seen.borrow_mut().push(message.id());
            None
        })
        .expect("作成");
        assert_eq!(log.borrow().first(), Some(&WM_NCCREATE));
        assert!(log.borrow().contains(&WM_CREATE));
        assert!(!dropped.get());

        window.destroy().expect("破棄");
        assert!(!is_window(window));
        assert!(log.borrow().contains(&WM_DESTROY));
        assert_eq!(log.borrow().last(), Some(&WM_NCDESTROY));
        assert!(dropped.get());
    }

    /// 記録用の印。破棄が通り、その時点で処理が持つ値が生きていたこと。
    const AFTER_DESTROY: u32 = u32::MAX;

    /// 処理の中で自分を破棄しても、入れ子の WM_DESTROY / WM_NCDESTROY は処理に入らず、
    /// 処理(クロージャ)は外側の呼び出しが終わってから解放される。
    #[test]
    fn destroying_self_inside_handler_is_safe() {
        let class = WindowClass::register("Monitoaster.Test.Reentry", None).expect("登録");
        let log = Rc::new(RefCell::new(Vec::new()));
        let dropped = Rc::new(Cell::new(false));
        let flag = DropFlag(Rc::clone(&dropped));
        let seen = Rc::clone(&log);
        let window = create_window(class, &hidden_spec(), move |window, message| {
            seen.borrow_mut().push(message.id());
            if message.id() == WM_CLOSE {
                // ここで panic すると呼び出し元(OS)を越えられずに止まるので、結果は記録して外で確かめる。
                let destroyed = window.destroy().is_ok();
                // 破棄の後も、処理が持つ値はまだ生きている。
                let alive = !flag.0.get();
                seen.borrow_mut()
                    .push(if destroyed && alive { AFTER_DESTROY } else { 0 });
                return Some(0);
            }
            None
        })
        .expect("作成");

        assert_eq!(send(window, WM_CLOSE), LRESULT(0));
        assert!(!is_window(window));
        let log = log.borrow();
        let close = log.iter().position(|&m| m == WM_CLOSE).expect("WM_CLOSE");
        assert_eq!(&log[close..], &[WM_CLOSE, AFTER_DESTROY]);
        assert!(!log.contains(&WM_NCDESTROY));
        assert!(dropped.get());
    }

    /// WM_CREATE で -1 を返すと作成は失敗し、処理は解放される。
    #[test]
    fn failing_wm_create_frees_handler() {
        let class = WindowClass::register("Monitoaster.Test.CreateFails", None).expect("登録");
        let dropped = Rc::new(Cell::new(false));
        let flag = DropFlag(Rc::clone(&dropped));
        let result = create_window(class, &hidden_spec(), move |_, message| {
            let _ = &flag;
            (message.id() == WM_CREATE).then_some(-1)
        });
        assert!(result.is_err());
        assert!(dropped.get());
    }

    /// WM_NCCREATE で 0 を返すと作成は失敗する。続けて届く WM_NCDESTROY で処理は解放される。
    #[test]
    fn failing_wm_nccreate_frees_handler() {
        let class = WindowClass::register("Monitoaster.Test.NcCreateFails", None).expect("登録");
        let dropped = Rc::new(Cell::new(false));
        let flag = DropFlag(Rc::clone(&dropped));
        let result = create_window(class, &hidden_spec(), move |_, message| {
            let _ = &flag;
            (message.id() == WM_NCCREATE).then_some(0)
        });
        assert!(result.is_err());
        assert!(dropped.get());
    }

    /// 子コントロールを作り、文字の読み書きとタイマーが使える。
    #[test]
    fn control_text_and_timer() {
        let class = WindowClass::register("Monitoaster.Test.Controls", None).expect("登録");
        let parent = create_window(class, &hidden_spec(), |_, _| None).expect("親");
        let edit = create_control(
            ControlClass::Edit,
            101,
            &WindowSpec {
                title: "区画 220 × 48 px",
                width: 150,
                height: 24,
                parent: Some(parent),
                ..Default::default()
            },
        )
        .expect("エディット");
        assert_eq!(edit.text(), "区画 220 × 48 px");
        edit.set_text("監視中").expect("文字の設定");
        assert_eq!(edit.text(), "監視中");

        let font = Font::message_font(144).expect("フォント");
        edit.set_font(&font);

        parent.set_timer(7, 1000).expect("タイマー");
        parent.kill_timer(7).expect("タイマーの停止");
        parent.destroy().expect("破棄");
        assert!(!is_window(edit));
    }

    /// S-04: WM_COMMAND の lParam は、自分の子のコントロールのときだけ Command::control になる。
    /// 別のトップレベルウィンドウを指す lParam や、0(メニュー)では None。
    #[test]
    fn command_control_requires_own_child() {
        let class = WindowClass::register("Monitoaster.Test.CommandControl", None).expect("登録");
        let commands = Rc::new(RefCell::new(Vec::new()));
        let seen = Rc::clone(&commands);
        let parent = create_window(class, &hidden_spec(), move |_, message| {
            let command = message.command()?;
            seen.borrow_mut().push(command);
            Some(0)
        })
        .expect("親");
        let button = create_control(
            ControlClass::Button,
            42,
            &WindowSpec {
                title: "ボタン",
                width: 80,
                height: 24,
                parent: Some(parent),
                ..Default::default()
            },
        )
        .expect("ボタン");
        let other = create_window(class, &hidden_spec(), |_, _| None).expect("別のウィンドウ");
        // 作成のときに届いたものは見ない。
        commands.borrow_mut().clear();

        let clicked = BN_CLICKED as u16;
        let send_command = |control: HWND| {
            let wparam = (usize::from(clicked) << 16) | 42;
            // SAFETY: WM_COMMAND の引数は数値とハンドルで、ポインターとしては読まれない。
            // 送り先は自分のスレッドのウィンドウ。
            unsafe {
                SendMessageW(
                    parent.hwnd(),
                    WM_COMMAND,
                    Some(WPARAM(wparam)),
                    Some(LPARAM(control.0.addr() as isize)),
                )
            };
        };
        send_command(button.hwnd());
        send_command(other.hwnd());
        send_command(HWND::default());

        let expected = |control| Command {
            id: 42,
            code: clicked,
            control,
        };
        assert_eq!(
            *commands.borrow(),
            [expected(Some(button)), expected(None), expected(None)]
        );
        other.destroy().expect("別のウィンドウの破棄");
        parent.destroy().expect("破棄");
    }

    /// 同じ名前のクラスは2回登録できない。
    #[test]
    fn registering_same_class_twice_fails() {
        WindowClass::register("Monitoaster.Test.Twice", None).expect("1回目");
        assert!(WindowClass::register("Monitoaster.Test.Twice", None).is_err());
    }
}
