//! M0 の技術検証用 exe(docs/m0.md 作業5、spec.md 9章の検証項目)。M0 の完了時に削除する。
//!
//! - 画面: 項目ごとのボタン、オーバーレイの代替を切り替えるチェックボックス、結果欄(読み取り専用の
//!   複数行エディット)。結果欄は「結果をすべて選択」→ Ctrl+C で写して報告できる。
//! - `--headless`: ウィンドウを出さずに、自動で判定できる項目(5、7、2・3)だけを実行し、結果を
//!   UTF-8 で標準出力に書いて終了コード 0 で終わる(CI の Windows ランナーで動かす)。失敗しても
//!   パニックせず、失敗として書いて次へ進む。待ちは数百ミリ秒単位で、全体で数秒に収まる。
//!
//! 守ること(CLAUDE.md、spec.md 7章):
//! - unsafe を書かない。Win32 と COM の呼び出しは platform を通す(S-11)。
//! - ファイルに書かない(S-03、S-06)。exe の大きさは読むだけ。画面の画素はメモリ上で数えるだけ。
//! - 他のプロセスに触れない(S-04)。前面のウィンドウは、位置・大きさとハンドルの値だけを読む。
//! - ウィンドウのタイトルを表示・出力しない(S-15)。ファイルのパスも出さない(利用者名が入るため)。
//!   アダプターの説明(GPU の製品名)は、利用者を特定する情報ではないので出す(項目7 などの判断に要る)。
//!
//! 文言は検証用の一時的なものなので、製品の文言(text モジュール)には入れず、このファイルに置く。
//! コンソールサブシステムにしている(標準出力に書くため)。画面で使うときはコンソールから切り離す。

#![forbid(unsafe_code)]

fn main() {
    #[cfg(windows)]
    probe::main();
}

/// OS に依存しない計算(判定と書式)。Linux でも単体テストする。
#[cfg(any(windows, test))]
mod logic {
    /// マゼンタ(#FF00FF)とみなす、R・G・B それぞれの差の上限。
    pub const MAGENTA_TOLERANCE: u8 = 24;

    /// 「写っている」とみなす割合の下限。
    pub const VISIBLE_RATIO: f64 = 0.9;

    /// 「写っていない」とみなす割合の上限。
    pub const HIDDEN_RATIO: f64 = 0.01;

    /// BGRX の画素(1画素4バイト)のうち、マゼンタに近いものの割合(0.0〜1.0)。
    pub fn magenta_ratio(bgrx: &[u8]) -> f64 {
        let total = bgrx.len() / 4;
        if total == 0 {
            return 0.0;
        }
        let near = bgrx
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[0].abs_diff(255) <= MAGENTA_TOLERANCE
                    && pixel[1] <= MAGENTA_TOLERANCE
                    && pixel[2].abs_diff(255) <= MAGENTA_TOLERANCE
            })
            .count();
        near as f64 / total as f64
    }

    /// BGRX の画素の、異なる色(R・G・B。4バイト目は見ない)の数。
    pub fn distinct_colors(bgrx: &[u8]) -> usize {
        let mut colors: Vec<u32> = bgrx
            .chunks_exact(4)
            .map(|p| u32::from(p[0]) | (u32::from(p[1]) << 8) | (u32::from(p[2]) << 16))
            .collect();
        colors.sort_unstable();
        colors.dedup();
        colors.len()
    }

    /// 割合から見た、写り方。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Seen {
        /// 写っている([`VISIBLE_RATIO`] 以上)。
        Visible,
        /// 写っていない([`HIDDEN_RATIO`] 以下)。
        Hidden,
        /// 一部だけ。
        Partial,
    }

    pub fn classify(ratio: f64) -> Seen {
        if ratio >= VISIBLE_RATIO {
            Seen::Visible
        } else if ratio <= HIDDEN_RATIO {
            Seen::Hidden
        } else {
            Seen::Partial
        }
    }

    /// キャプチャ除外の判定。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Exclusion {
        /// アフィニティなしでは写り、ありでは写らない。
        Excluded,
        /// アフィニティありでも写る。
        NotExcluded,
        /// アフィニティありで一部だけ写る。
        Partial,
        /// アフィニティなしでも写らない(表示されていないか、BitBlt に写らない)。
        OverlayNotCaptured,
        /// 取り込みに失敗したなどで判定できない。
        Unknown,
    }

    /// `plain` はアフィニティなし、`excluded` はありのときのマゼンタの割合(取れなければ None)。
    pub fn exclusion(plain: Option<f64>, excluded: Option<f64>) -> Exclusion {
        match (plain.map(classify), excluded.map(classify)) {
            (Some(Seen::Visible), Some(Seen::Hidden)) => Exclusion::Excluded,
            (Some(Seen::Visible), Some(Seen::Visible)) => Exclusion::NotExcluded,
            (Some(Seen::Visible), Some(Seen::Partial)) => Exclusion::Partial,
            (Some(Seen::Hidden | Seen::Partial), _) => Exclusion::OverlayNotCaptured,
            _ => Exclusion::Unknown,
        }
    }

    pub fn exclusion_text(exclusion: Exclusion) -> &'static str {
        match exclusion {
            Exclusion::Excluded => {
                "成功。除外できている(アフィニティなしでは写り、ありでは写らない)"
            }
            Exclusion::NotExcluded => "失敗。除外できていない(アフィニティありでも写る)",
            Exclusion::Partial => "失敗。アフィニティありで一部だけ写る",
            Exclusion::OverlayNotCaptured => {
                "判定できない。アフィニティなしでもオーバーレイが写らない(表示されていないか、BitBlt に写らない)"
            }
            Exclusion::Unknown => {
                "判定できない(取り込みに失敗したか、アフィニティを設定できなかった)"
            }
        }
    }

    /// アフィニティなしの四角の写り方が、CAPTUREBLT の有無で変わるか。
    pub fn captureblt_text(without: Option<f64>, with: Option<f64>) -> &'static str {
        match (without.map(classify), with.map(classify)) {
            (Some(a), Some(b)) if a == b => "CAPTUREBLT の有無で、四角の写り方は変わらない",
            (Some(Seen::Hidden), Some(Seen::Visible)) => {
                "CAPTUREBLT ありでだけ写る(レイヤードウィンドウとして扱われている)"
            }
            (Some(_), Some(_)) => "CAPTUREBLT の有無で写り方が変わる",
            _ => "比べられない(取り込みに失敗した)",
        }
    }

    /// 残り時間(秒、切り上げ)。`total_ms` を過ぎていたら None。
    pub fn remaining_seconds(elapsed_ms: u64, total_ms: u64) -> Option<u64> {
        (elapsed_ms < total_ms).then(|| (total_ms - elapsed_ms).div_ceil(1000))
    }

    /// 3桁ごとにカンマを入れる。
    pub fn group_digits(value: u64) -> String {
        let digits = value.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                out.push(',');
            }
            out.push(digit);
        }
        out
    }

    /// ファイルの大きさ。「1,234,567 バイト(1.18 MiB)」。
    pub fn format_size(bytes: u64) -> String {
        format!(
            "{} バイト({:.2} MiB)",
            group_digits(bytes),
            bytes as f64 / 1_048_576.0
        )
    }

    /// メモリの量。タスクマネージャーに合わせて K(1024 バイト)で出す。「12,345 K(12.1 MiB)」。
    pub fn format_memory(bytes: u64) -> String {
        format!(
            "{} K({:.1} MiB)",
            group_digits(bytes / 1024),
            bytes as f64 / 1_048_576.0
        )
    }

    /// 割合。「99.8%」。
    pub fn format_percent(ratio: f64) -> String {
        format!("{:.1}%", ratio * 100.0)
    }

    /// ハンドルの値。「0x1A2B3C」。
    pub fn format_handle(value: usize) -> String {
        format!("0x{value:X}")
    }

    /// 0 から `span` までを往復する位置。周期は `period_ms`。
    pub fn bounce(elapsed_ms: u64, period_ms: u64, span: f32) -> f32 {
        if period_ms < 2 || span <= 0.0 {
            return 0.0;
        }
        let half = period_ms / 2;
        let phase = elapsed_ms % period_ms;
        let t = if phase < half {
            phase as f32 / half as f32
        } else {
            (period_ms - phase) as f32 / (period_ms - half) as f32
        };
        t * span
    }

    /// 矩形(left, top, right, bottom)の中央に置く、一辺 `size` の正方形の左上。
    pub fn centered(left: i32, top: i32, right: i32, bottom: i32, size: i32) -> (i32, i32) {
        let x = left.saturating_add(right.saturating_sub(left).saturating_sub(size) / 2);
        let y = top.saturating_add(bottom.saturating_sub(top).saturating_sub(size) / 2);
        (x, y)
    }

    /// 矩形の中央の、幅・高さとも `size` まで(矩形が小さければその大きさ)の範囲。
    /// (left, top, width, height)。矩形が空なら None。
    pub fn center_crop(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        size: i32,
    ) -> Option<(i32, i32, i32, i32)> {
        let width = right.saturating_sub(left).min(size);
        let height = bottom.saturating_sub(top).min(size);
        if width <= 0 || height <= 0 {
            return None;
        }
        let x = left.saturating_add((right.saturating_sub(left) - width) / 2);
        let y = top.saturating_add((bottom.saturating_sub(top) - height) / 2);
        Some((x, y, width, height))
    }

    /// ウィンドウのハンドルの値を、分かっているウィンドウなら名前を添えて書く(タイトルは出さない)。
    pub fn describe_window(value: Option<usize>, known: &[(usize, &str)]) -> String {
        let Some(value) = value else {
            return "なし".to_string();
        };
        let name = known
            .iter()
            .find(|(handle, _)| *handle == value)
            .map_or("ほかのウィンドウ", |(_, name)| name);
        format!("{}({name})", format_handle(value))
    }

    /// クリックの直前・直後・200ms 後で、前面のウィンドウが同じだったか。
    pub fn focus_kept(before: Option<usize>, at: Option<usize>, after: Option<usize>) -> bool {
        before.is_some() && before == at && at == after
    }

    /// Microsoft Basic Render Driver の (VendorId, DeviceId)。
    pub const BASIC_RENDER_IDS: (u32, u32) = (0x1414, 0x8C);

    /// ソフトウェアで描くアダプター(Microsoft Basic Render Driver)か。
    ///
    /// GPU のない環境で主アダプターになった Basic Render Driver には DXGI_ADAPTER_FLAG_SOFTWARE が
    /// 立たないので、旗に加えて ID でも見分ける。
    pub fn is_basic_render(vendor_id: u32, device_id: u32, software_flag: bool) -> bool {
        software_flag || (vendor_id, device_id) == BASIC_RENDER_IDS
    }

    /// アダプターの説明と ID。「NVIDIA GeForce RTX 3060(VendorId 0x10DE、DeviceId 0x2504)」。
    /// ソフトウェアで描くアダプターなら、そう明記する。
    pub fn adapter_text(
        description: &str,
        vendor_id: u32,
        device_id: u32,
        software_flag: bool,
    ) -> String {
        let description = if description.is_empty() {
            "説明なし"
        } else {
            description
        };
        let text = format!("{description}(VendorId 0x{vendor_id:04X}、DeviceId 0x{device_id:04X})");
        if is_basic_render(vendor_id, device_id, software_flag) {
            format!("{text}。GPU ではなく、ソフトウェア(Microsoft Basic Render Driver)で描く")
        } else {
            text
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn pixels(colors: &[[u8; 3]]) -> Vec<u8> {
            colors.iter().flat_map(|&[r, g, b]| [b, g, r, 0]).collect()
        }

        /// F-10、E-05(項目2・3): マゼンタに近い画素だけを数える(許容差は R・G・B それぞれ)。
        #[test]
        fn magenta_ratio_counts_near_pixels() {
            let data = pixels(&[[255, 0, 255], [240, 10, 250], [255, 0, 200], [0, 0, 0]]);
            assert_eq!(magenta_ratio(&data), 0.5);
            assert_eq!(magenta_ratio(&[]), 0.0);
            assert_eq!(magenta_ratio(&pixels(&[[255, 0, 255]; 9])), 1.0);
        }

        /// 項目4: 4バイト目は色に数えない。
        #[test]
        fn distinct_colors_ignores_fourth_byte() {
            let mut data = pixels(&[[1, 2, 3], [1, 2, 3], [9, 9, 9]]);
            data[3] = 77;
            assert_eq!(distinct_colors(&data), 2);
            assert_eq!(distinct_colors(&[]), 0);
        }

        /// F-10、E-05(項目2・3): 割合を、写っている(90% 以上)・写っていない(1% 以下)・一部に分ける。
        #[test]
        fn classify_uses_thresholds() {
            assert_eq!(classify(1.0), Seen::Visible);
            assert_eq!(classify(0.9), Seen::Visible);
            assert_eq!(classify(0.5), Seen::Partial);
            assert_eq!(classify(0.01), Seen::Hidden);
            assert_eq!(classify(0.0), Seen::Hidden);
        }

        /// F-10、E-05(項目2): アフィニティなしで写り、ありで写らなければ除外できている。
        /// なしでも写らなければ、除外の判定はできない。
        #[test]
        fn exclusion_needs_visible_overlay_without_affinity() {
            assert_eq!(exclusion(Some(1.0), Some(0.0)), Exclusion::Excluded);
            assert_eq!(exclusion(Some(1.0), Some(1.0)), Exclusion::NotExcluded);
            assert_eq!(exclusion(Some(1.0), Some(0.4)), Exclusion::Partial);
            assert_eq!(
                exclusion(Some(0.0), Some(0.0)),
                Exclusion::OverlayNotCaptured
            );
            assert_eq!(exclusion(Some(0.5), None), Exclusion::OverlayNotCaptured);
            assert_eq!(exclusion(Some(1.0), None), Exclusion::Unknown);
            assert_eq!(exclusion(None, Some(0.0)), Exclusion::Unknown);
        }

        /// 判定の文は、結論(成功 / 失敗 / 判定できない)から始める。
        #[test]
        fn exclusion_text_starts_with_verdict() {
            assert!(exclusion_text(Exclusion::Excluded).starts_with("成功"));
            assert!(exclusion_text(Exclusion::NotExcluded).starts_with("失敗"));
            assert!(exclusion_text(Exclusion::Partial).starts_with("失敗"));
            assert!(exclusion_text(Exclusion::OverlayNotCaptured).starts_with("判定できない"));
            assert!(exclusion_text(Exclusion::Unknown).starts_with("判定できない"));
        }

        /// spec.md 9章の検証項目(項目3): CAPTUREBLT の有無での写り方を、分類どうしで比べる。
        #[test]
        fn captureblt_text_compares_classes() {
            assert!(captureblt_text(Some(1.0), Some(0.95)).contains("変わらない"));
            assert!(captureblt_text(Some(0.0), Some(1.0)).contains("ありでだけ"));
            assert!(captureblt_text(Some(1.0), Some(0.0)).contains("変わる"));
            assert!(captureblt_text(None, Some(1.0)).contains("比べられない"));
        }

        /// 数え下げは切り上げで、期限を過ぎたら終わる(引き算で桁あふれしない)。
        #[test]
        fn remaining_seconds_rounds_up_and_ends() {
            assert_eq!(remaining_seconds(0, 5000), Some(5));
            assert_eq!(remaining_seconds(1, 5000), Some(5));
            assert_eq!(remaining_seconds(4001, 5000), Some(1));
            assert_eq!(remaining_seconds(5000, 5000), None);
            assert_eq!(remaining_seconds(u64::MAX, 5000), None);
        }

        /// 数は3桁ごとにカンマを入れ、大きさ・メモリ・割合・ハンドルを決まった形で書く。
        #[test]
        fn formats_numbers() {
            assert_eq!(group_digits(0), "0");
            assert_eq!(group_digits(999), "999");
            assert_eq!(group_digits(1000), "1,000");
            assert_eq!(group_digits(1_234_567), "1,234,567");
            assert_eq!(format_size(1_048_576), "1,048,576 バイト(1.00 MiB)");
            assert_eq!(format_memory(12 * 1_048_576), "12,288 K(12.0 MiB)");
            assert_eq!(format_percent(0.998), "99.8%");
            assert_eq!(format_handle(0x1A2B), "0x1A2B");
        }

        /// 項目1 の図形の位置は、0 から span までを周期で往復する(周期や span が不正なら 0)。
        #[test]
        fn bounce_goes_and_returns() {
            assert_eq!(bounce(0, 1000, 100.0), 0.0);
            assert_eq!(bounce(250, 1000, 100.0), 50.0);
            assert_eq!(bounce(500, 1000, 100.0), 100.0);
            assert_eq!(bounce(750, 1000, 100.0), 50.0);
            assert_eq!(bounce(1000, 1000, 100.0), 0.0);
            assert_eq!(bounce(10, 0, 100.0), 0.0);
            assert_eq!(bounce(10, 1000, -1.0), 0.0);
        }

        /// 矩形の中央に置く正方形の左上と、中央から切り出す範囲(矩形が小さければその大きさ、空なら None)。
        #[test]
        fn centered_and_center_crop() {
            assert_eq!(centered(0, 0, 1920, 1080, 200), (860, 440));
            assert_eq!(centered(-1920, 0, 0, 1080, 200), (-1060, 440));
            assert_eq!(
                center_crop(100, 100, 500, 300, 100),
                Some((250, 150, 100, 100))
            );
            // 小さいウィンドウは、その大きさまで。
            assert_eq!(center_crop(0, 0, 60, 40, 100), Some((0, 0, 60, 40)));
            assert_eq!(center_crop(0, 0, 0, 40, 100), None);
        }

        /// S-15(項目8): タイトルは出さず、分かっているウィンドウには名前を添える。
        #[test]
        fn describe_window_names_known_handles() {
            let known = [(0x10, "バナー")];
            assert_eq!(describe_window(Some(0x10), &known), "0x10(バナー)");
            assert_eq!(
                describe_window(Some(0x20), &known),
                "0x20(ほかのウィンドウ)"
            );
            assert_eq!(describe_window(None, &known), "なし");
        }

        /// spec.md 9章の検証項目(項目8): クリックの直前・直後・200ms 後で前面が同じときだけ、フォーカスを保てたとみなす。
        #[test]
        fn focus_kept_needs_same_window_three_times() {
            assert!(focus_kept(Some(1), Some(1), Some(1)));
            assert!(!focus_kept(Some(1), Some(2), Some(2)));
            assert!(!focus_kept(Some(1), Some(1), None));
            assert!(!focus_kept(None, None, None));
        }

        /// spec.md 9章の検証項目(項目7、仮想マシン・リモートデスクトップ): Basic Render Driver は、
        /// ソフトウェアの旗か、ID(0x1414、0x8C)で見分ける。
        #[test]
        fn is_basic_render_checks_flag_or_ids() {
            // 主アダプターになった Basic Render Driver(旗は立たない)。
            assert!(is_basic_render(0x1414, 0x8C, false));
            // WARP のデバイスのアダプター(旗が立つ)。
            assert!(is_basic_render(0x1414, 0x8C, true));
            assert!(is_basic_render(0x10DE, 0x2504, true));
            assert!(!is_basic_render(0x10DE, 0x2504, false));
            // ID は2つとも合わなければ違う。
            assert!(!is_basic_render(0x1414, 0x8D, false));
            assert!(!is_basic_render(0x8086, 0x8C, false));
        }

        /// spec.md 9章の検証項目(項目7): アダプターは説明と ID を書き、ソフトウェアで描くならそう明記する。
        #[test]
        fn adapter_text_marks_basic_render() {
            assert_eq!(
                adapter_text("NVIDIA GeForce RTX 3060", 0x10DE, 0x2504, false),
                "NVIDIA GeForce RTX 3060(VendorId 0x10DE、DeviceId 0x2504)"
            );
            assert_eq!(
                adapter_text("Microsoft Basic Render Driver", 0x1414, 0x8C, false),
                "Microsoft Basic Render Driver(VendorId 0x1414、DeviceId 0x008C)。\
                 GPU ではなく、ソフトウェア(Microsoft Basic Render Driver)で描く"
            );
            assert_eq!(
                adapter_text("", 0x1AB8, 0x10, false),
                "説明なし(VendorId 0x1AB8、DeviceId 0x0010)"
            );
        }
    }
}

#[cfg(windows)]
mod probe {
    use std::cell::{Cell, OnceCell, RefCell};
    use std::collections::VecDeque;
    use std::io::Write as _;
    use std::path::Path;
    use std::rc::Rc;
    use std::time::Instant;

    use monitoaster::platform::probe_support::{self, MemoryInfo, StageOutcome};
    use monitoaster::platform::{
        self, AdapterInfo, Canvas, CompositionSurface, ControlClass, Driver, DriverPreference,
        Error, Font, GraphicsDevice, Message, Pump, Rgb, ScreenCapture, Stage, TrayIcon, Window,
        WindowClass, WindowSpec,
    };
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
    use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
    use windows::Win32::UI::WindowsAndMessaging::{
        BN_CLICKED, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CW_USEDEFAULT, ES_AUTOVSCROLL, ES_MULTILINE,
        ES_READONLY, MA_NOACTIVATE, SW_SHOWDEFAULT, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOZORDER, WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_LBUTTONDOWN,
        WM_MOUSEACTIVATE, WM_PAINT, WM_SIZE, WM_TIMER, WS_EX_CLIENTEDGE, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };

    use super::logic;

    pub fn main() {
        // S-09: ほかの処理より先に、動的に読み込む DLL の検索先を System32 に限る。
        // 製品は失敗したら起動しないが、検証用 exe は結果に書いて続ける。
        let dll_search = platform::restrict_dll_search();
        let startup = probe_support::memory_info();
        if std::env::args_os().skip(1).any(|arg| arg == "--headless") {
            run_headless(dll_search, startup);
        } else {
            run_gui(dll_search, startup);
        }
    }

    // ---- 出力 ----------------------------------------------------------------------------

    /// 結果の書き出し先。画面では結果欄、--headless では標準出力。
    trait Report {
        /// 1行書く(text の中の改行もそのまま行に分ける)。
        fn line(&self, text: &str);

        /// 状態欄に書く(画面だけ)。
        fn status(&self, _text: &str) {}
    }

    /// 標準出力(--headless)。1行ごとに書き出す(途中で止まっても、そこまでが残るように)。
    struct StdoutReport;

    impl Report for StdoutReport {
        fn line(&self, text: &str) {
            let mut out = std::io::stdout().lock();
            // 書けなくても(パイプが閉じられたなど)続ける。
            let _ = writeln!(out, "{text}");
            let _ = out.flush();
        }
    }

    /// 画面の結果欄と状態欄。
    #[derive(Clone, Copy)]
    struct WindowReport {
        log: Window,
        status: Window,
    }

    impl Report for WindowReport {
        fn line(&self, text: &str) {
            let mut text = text.replace('\n', "\r\n");
            text.push_str("\r\n");
            self.log.append_text(&text);
        }

        fn status(&self, text: &str) {
            let _ = self.status.set_text(text);
        }
    }

    /// エラーを「0x8007000E 説明」の形にする。
    fn describe_error(error: &Error) -> String {
        let code = error.code().0 as u32;
        let message = error.message();
        let message = message.trim();
        if message.is_empty() {
            format!("0x{code:08X}")
        } else {
            format!("0x{code:08X} {message}")
        }
    }

    /// Win32 のエラー(HRESULT が 0x8007xxxx)なら、GetLastError の値も添える。
    fn describe_win32_error(error: &Error) -> String {
        let code = error.code().0 as u32;
        if code & 0xFFFF_0000 == 0x8007_0000 {
            format!(
                "{}(GetLastError = {})",
                describe_error(error),
                code & 0xFFFF
            )
        } else {
            describe_error(error)
        }
    }

    fn stage_name(stage: Stage) -> &'static str {
        match stage {
            Stage::D3dDevice => "D3D11 デバイス(D3D11CreateDevice、BGRA_SUPPORT)",
            Stage::DxgiDevice => "IDXGIDevice(D3D11 デバイスから)",
            Stage::DxgiFactory => "DXGI ファクトリー(CreateDXGIFactory2)",
            Stage::SwapChain => "スワップチェーン(CreateSwapChainForComposition 64×64)",
            Stage::CompositionDevice => "DirectComposition デバイス(DCompositionCreateDevice)",
            Stage::CompositionTarget => "ターゲット(CreateTargetForHwnd)",
            Stage::CompositionVisual => "ビジュアル(CreateVisual、SetContent、SetRoot)",
            Stage::Commit => "反映(IDCompositionDevice::Commit)",
            Stage::D2dFactory => "Direct2D ファクトリー(D2D1CreateFactory)",
            Stage::D2dDevice => "Direct2D デバイス(ID2D1Factory1::CreateDevice)",
            Stage::D2dContext => "Direct2D デバイスコンテキスト(CreateDeviceContext)",
            Stage::TargetBitmap => "描画先(CreateBitmapFromDxgiSurface)",
            Stage::Draw => "描画(BeginDraw〜EndDraw)",
            Stage::Present => "表示(Present(1, 0))",
        }
    }

    /// ドライバーの種類。D3D_DRIVER_TYPE_HARDWARE でも GPU で描くとは限らないので、GPU とは書かない
    /// (GPU のない環境では Microsoft Basic Render Driver になる。アダプターを添えて見分ける)。
    fn driver_name(driver: Driver) -> &'static str {
        match driver {
            Driver::Hardware => "D3D_DRIVER_TYPE_HARDWARE",
            Driver::Warp => "WARP(ソフトウェア)",
        }
    }

    /// アダプターの説明と ID。読めなければ、その理由。
    fn describe_adapter(adapter: &platform::Result<AdapterInfo>) -> String {
        match adapter {
            Ok(info) => logic::adapter_text(
                &info.description,
                info.vendor_id,
                info.device_id,
                info.software_flag,
            ),
            Err(error) => format!("読めなかった {}", describe_error(error)),
        }
    }

    fn yes_no(flag: bool) -> &'static str {
        if flag { "あり" } else { "なし" }
    }

    fn rect_text(rect: RECT) -> String {
        format!(
            "({}, {})〜({}, {})、{}×{}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            rect.right.saturating_sub(rect.left),
            rect.bottom.saturating_sub(rect.top)
        )
    }

    fn millis_since(started: Instant) -> u64 {
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// `ms` の間、メッセージを処理しながら待つ。終了を頼まれたら false(外側のループにも伝える)。
    fn wait(ms: u32) -> bool {
        match platform::pump_messages_for(ms) {
            Pump::Continue => true,
            Pump::Quit(code) => {
                platform::post_quit(code);
                false
            }
        }
    }

    // ---- ウィンドウクラスとオーバーレイの設定 --------------------------------------------

    #[derive(Clone, Copy)]
    struct Classes {
        main: WindowClass,
        overlay: WindowClass,
        banner: WindowClass,
        hidden: WindowClass,
    }

    impl Classes {
        fn register() -> platform::Result<Self> {
            Ok(Self {
                main: WindowClass::register("Monitoaster.Probe.Main", Some(COLOR_BTNFACE))?,
                overlay: WindowClass::register("Monitoaster.Probe.Overlay", None)?,
                banner: WindowClass::register("Monitoaster.Probe.Banner", None)?,
                hidden: WindowClass::register("Monitoaster.Probe.Hidden", None)?,
            })
        }
    }

    /// オーバーレイの作り方(項目1・2・3・7 の代替を試すための切り替え)。
    #[derive(Clone, Copy, Debug)]
    struct OverlayOptions {
        layered: bool,
        layered_alpha: bool,
        transparent: bool,
        no_redirection: bool,
        warp_only: bool,
    }

    impl OverlayOptions {
        /// spec.md 9章の構成(NOREDIRECTIONBITMAP、TRANSPARENT、LAYERED、TOPMOST、NOACTIVATE、
        /// TOOLWINDOW)。SetLayeredWindowAttributes は呼ばない。D3D はハードウェア→WARP。
        const SPEC: Self = Self {
            layered: true,
            layered_alpha: false,
            transparent: true,
            no_redirection: true,
            warp_only: false,
        };

        fn ex_style(self) -> WINDOW_EX_STYLE {
            let mut style = WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
            if self.layered {
                style |= WS_EX_LAYERED;
            }
            if self.transparent {
                style |= WS_EX_TRANSPARENT;
            }
            if self.no_redirection {
                style |= WS_EX_NOREDIRECTIONBITMAP;
            }
            style
        }

        fn preference(self) -> DriverPreference {
            if self.warp_only {
                DriverPreference::WarpOnly
            } else {
                DriverPreference::HardwareThenWarp
            }
        }

        fn describe(self) -> String {
            format!(
                "WS_EX_LAYERED {}、SetLayeredWindowAttributes(α=255) {}、WS_EX_TRANSPARENT {}、\
                 WS_EX_NOREDIRECTIONBITMAP {}、D3D {}(TOPMOST、NOACTIVATE、TOOLWINDOW は常に付ける)",
                yes_no(self.layered),
                yes_no(self.layered_alpha),
                yes_no(self.transparent),
                yes_no(self.no_redirection),
                if self.warp_only {
                    "WARP だけ"
                } else {
                    "ハードウェア→だめなら WARP"
                },
            )
        }
    }

    // ---- オーバーレイ ----------------------------------------------------------------------

    /// オーバーレイのコマ送りのタイマー。
    const FRAME_TIMER: usize = 1;

    /// コマ送りの間隔(ミリ秒)。
    const FRAME_INTERVAL_MS: u32 = 16;

    /// オーバーレイに描く絵。
    #[derive(Clone, Copy)]
    enum Scene {
        /// 項目1: 透明の上に、縁の枠と、動く半透明の図形。
        Demo,
        /// 項目2・3: 全体を不透明のマゼンタ(#FF00FF)で塗る。
        Magenta,
    }

    fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r, g, b, a }
    }

    fn rect_f(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left,
            top,
            right,
            bottom,
        }
    }

    impl Scene {
        fn paint(self, canvas: &Canvas<'_>, width: f32, height: f32, elapsed_ms: u64) {
            match self {
                Scene::Magenta => canvas.clear(rgba(1.0, 0.0, 1.0, 1.0)),
                Scene::Demo => {
                    canvas.clear(rgba(0.0, 0.0, 0.0, 0.0));
                    let edge = 8.0;
                    let frame = rgba(0.0, 0.75, 1.0, 0.6);
                    canvas.fill_rect(rect_f(0.0, 0.0, width, edge), frame);
                    canvas.fill_rect(rect_f(0.0, height - edge, width, height), frame);
                    canvas.fill_rect(rect_f(0.0, edge, edge, height - edge), frame);
                    canvas.fill_rect(rect_f(width - edge, edge, width, height - edge), frame);
                    let radius = height / 10.0;
                    let x = radius + logic::bounce(elapsed_ms, 2400, width - radius * 2.0);
                    canvas.fill_ellipse(x, height / 2.0, radius, radius, rgba(1.0, 0.55, 0.0, 0.5));
                    let size = height / 8.0;
                    let left = width / 3.0;
                    let top = logic::bounce(elapsed_ms, 1800, height - size);
                    canvas.fill_rect(
                        rect_f(left, top, left + size, top + size),
                        rgba(0.6, 0.2, 0.9, 0.45),
                    );
                }
            }
        }
    }

    /// オーバーレイの描画の記録(処理と共有する)。
    #[derive(Default)]
    struct OverlayStats {
        frames: Cell<u32>,
        error: RefCell<Option<String>>,
    }

    /// 開いているオーバーレイ。[`Overlay::close`] で閉じる。
    struct Overlay {
        window: Window,
        /// 描画面。ウィンドウの処理と共有する。閉じるときに、ウィンドウを破棄してから捨てる。
        surface: Rc<RefCell<Option<CompositionSurface>>>,
        stats: Rc<OverlayStats>,
        /// SetLayeredWindowAttributes を呼んだときの結果(呼ばなければ None)。
        layered_alpha: Option<platform::Result<()>>,
    }

    /// `rect`(画面座標)を覆うオーバーレイを、非表示で作って1コマ描く。表示は呼び出し側が行う。
    fn open_overlay(
        classes: &Classes,
        device: &GraphicsDevice,
        rect: RECT,
        options: OverlayOptions,
        scene: Scene,
        started: Instant,
    ) -> Result<Overlay, String> {
        let width = rect.right.saturating_sub(rect.left);
        let height = rect.bottom.saturating_sub(rect.top);
        let (Ok(surface_width), Ok(surface_height)) = (u32::try_from(width), u32::try_from(height))
        else {
            return Err(format!("大きさが不正です: {}", rect_text(rect)));
        };
        if surface_width == 0 || surface_height == 0 {
            return Err(format!("大きさが不正です: {}", rect_text(rect)));
        }
        let surface: Rc<RefCell<Option<CompositionSurface>>> = Rc::default();
        let stats = Rc::new(OverlayStats::default());
        let handler_surface = Rc::clone(&surface);
        let handler_stats = Rc::clone(&stats);
        let window = platform::create_window(
            classes.overlay,
            &WindowSpec {
                ex_style: options.ex_style(),
                style: WS_POPUP,
                title: "",
                x: rect.left,
                y: rect.top,
                width,
                height,
                parent: None,
            },
            move |window, message| match message.id() {
                // E-01: クリックされても前面にならない。
                WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
                WM_TIMER if message.timer_id() == Some(FRAME_TIMER) => {
                    if !render_frame(&handler_surface, &handler_stats, scene, started) {
                        let _ = window.kill_timer(FRAME_TIMER);
                    }
                    Some(0)
                }
                _ => None,
            },
        )
        .map_err(|error| format!("ウィンドウを作れませんでした: {}", describe_error(&error)))?;
        let layered_alpha = options.layered_alpha.then(|| window.set_layered_alpha(255));
        match CompositionSurface::create(device, window, surface_width, surface_height) {
            Ok(created) => *surface.borrow_mut() = Some(created),
            Err(error) => {
                let _ = window.destroy();
                return Err(format!(
                    "描画面を作れませんでした: {}: {}",
                    stage_name(error.stage),
                    describe_error(&error.error)
                ));
            }
        }
        // 表示する前に1コマ描いておく。
        render_frame(&surface, &stats, scene, started);
        Ok(Overlay {
            window,
            surface,
            stats,
            layered_alpha,
        })
    }

    /// 1コマ描いて出す。続けてよければ true。失敗したら記録して false。
    fn render_frame(
        surface: &RefCell<Option<CompositionSurface>>,
        stats: &OverlayStats,
        scene: Scene,
        started: Instant,
    ) -> bool {
        if stats.error.borrow().is_some() {
            return false;
        }
        let Ok(mut surface) = surface.try_borrow_mut() else {
            return true;
        };
        let Some(surface) = surface.as_mut() else {
            return true;
        };
        let (width, height) = surface.size();
        let elapsed_ms = millis_since(started);
        let result = surface
            .draw(|canvas| scene.paint(canvas, width as f32, height as f32, elapsed_ms))
            .map_err(|error| format!("描画(EndDraw)に失敗: {}", describe_error(&error)))
            .and_then(|()| {
                surface
                    .present()
                    .map_err(|error| format!("表示(Present)に失敗: {}", describe_error(&error)))
            });
        match result {
            Ok(()) => {
                stats.frames.set(stats.frames.get().saturating_add(1));
                true
            }
            Err(message) => {
                *stats.error.borrow_mut() = Some(message);
                false
            }
        }
    }

    impl Overlay {
        /// タイマーを止め、ウィンドウを破棄し、描画面を捨てる。描いたコマ数と、最初のエラーを返す。
        fn close(self) -> (u32, Option<String>) {
            let _ = self.window.kill_timer(FRAME_TIMER);
            let _ = self.window.destroy();
            // 処理が持っていた参照は WM_NCDESTROY で手放している。ここで描画面を捨てる。
            drop(self.surface.borrow_mut().take());
            let error = self.stats.error.borrow().clone();
            (self.stats.frames.get(), error)
        }
    }

    fn report_device(out: &dyn Report, device: &GraphicsDevice) {
        let (major, minor) = device.feature_level();
        out.line(&format!(
            "描画デバイス: {}、機能レベル {major}_{minor}、アダプター: {}",
            driver_name(device.driver()),
            describe_adapter(&device.adapter())
        ));
        if let Some(error) = device.hardware_error() {
            out.line(&format!(
                "- ハードウェアでは作れなかったので WARP にした: {}",
                describe_error(error)
            ));
        }
    }

    fn create_device(out: &dyn Report, options: OverlayOptions) -> Option<GraphicsDevice> {
        match GraphicsDevice::create(options.preference()) {
            Ok(device) => {
                report_device(out, &device);
                Some(device)
            }
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。描画デバイスを作れなかった: {}: {}",
                    stage_name(error.stage),
                    describe_error(&error.error)
                ));
                None
            }
        }
    }

    fn report_layered_alpha(out: &dyn Report, overlay: &Overlay) {
        match &overlay.layered_alpha {
            None => {}
            Some(Ok(())) => out.line("SetLayeredWindowAttributes(α=255): 成功"),
            Some(Err(error)) => out.line(&format!(
                "SetLayeredWindowAttributes(α=255): 失敗 {}",
                describe_win32_error(error)
            )),
        }
    }

    /// 主モニター(なければ最初のモニター)の中央の、一辺 `size` の正方形。
    fn primary_square(out: &dyn Report, size: i32) -> Option<RECT> {
        let monitors = match platform::monitors() {
            Ok(list) => list,
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。モニターの一覧を取れなかった: {}",
                    describe_error(&error)
                ));
                return None;
            }
        };
        let Some(monitor) = monitors.iter().find(|m| m.primary).or(monitors.first()) else {
            out.line("結果: 失敗。モニターが見つからなかった");
            return None;
        };
        let rect = monitor.rect;
        let (x, y) = logic::centered(rect.left, rect.top, rect.right, rect.bottom, size);
        Some(RECT {
            left: x,
            top: y,
            right: x + size,
            bottom: y + size,
        })
    }

    // ---- 項目1 -----------------------------------------------------------------------------

    /// 項目1 でオーバーレイを出しておく時間(ミリ秒)。
    const OVERLAY_DEMO_MS: u64 = 5000;

    fn item1_overlay(out: &dyn Report, classes: &Classes, options: OverlayOptions) {
        out.line("## 1. オーバーレイの透過とクリック素通し");
        out.line(&format!("設定: {}", options.describe()));
        out.line(
            "すること: 先にメモ帳などを開いておく。このボタンを押したら5秒の間に、オーバーレイの下にある\
             そのアプリをクリックして文字を打つ。オーバーレイの図形の上をクリックしても、そのアプリに届くかを見る",
        );
        out.line(
            "見えれば成功: 各モニターの縁に水色の枠が出て、半透明のオレンジの丸と紫の四角が動く。\
             下のアプリが透けて見え、クリックと入力がそのアプリに届き、オーバーレイがフォーカスを取らない",
        );
        let monitors = match platform::monitors() {
            Ok(list) if !list.is_empty() => list,
            Ok(_) => {
                out.line("結果: 失敗。モニターが見つからなかった\n");
                return;
            }
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。モニターの一覧を取れなかった: {}\n",
                    describe_error(&error)
                ));
                return;
            }
        };
        let Some(device) = create_device(out, options) else {
            out.line("");
            return;
        };
        let started = Instant::now();
        let mut overlays = Vec::new();
        for (index, monitor) in monitors.iter().enumerate() {
            match open_overlay(
                classes,
                &device,
                monitor.rect,
                options,
                Scene::Demo,
                started,
            ) {
                Ok(overlay) => overlays.push((index, monitor.rect, overlay)),
                Err(message) => out.line(&format!("- モニター{}: {message}", index + 1)),
            }
        }
        for (_, _, overlay) in &overlays {
            if let Err(error) = overlay.window.set_timer(FRAME_TIMER, FRAME_INTERVAL_MS) {
                out.line(&format!(
                    "- タイマーを設定できなかった: {}",
                    describe_error(&error)
                ));
            }
            overlay.window.show(SW_SHOWNOACTIVATE);
        }
        // 100ms ごとに前面のウィンドウを調べ、オーバーレイが前面になっていないかを見る。
        let mut samples = 0u32;
        let mut overlay_foreground = 0u32;
        let mut completed = true;
        while let Some(left) = logic::remaining_seconds(millis_since(started), OVERLAY_DEMO_MS) {
            out.status(&format!("1. オーバーレイを表示中(残り {left} 秒)"));
            if !wait(100) {
                completed = false;
                break;
            }
            samples += 1;
            let foreground = platform::foreground_window();
            if overlays
                .iter()
                .any(|(_, _, overlay)| Some(overlay.window.id()) == foreground)
            {
                overlay_foreground += 1;
            }
        }
        let seconds = started.elapsed().as_secs_f64().max(0.001);
        // 先にすべて閉じてから(描画面 → デバイスの順に捨ててから)、結果を書く。
        let mut rows = Vec::new();
        for (index, rect, overlay) in overlays {
            let dpi = overlay.window.dpi();
            report_layered_alpha(out, &overlay);
            let (frames, error) = overlay.close();
            rows.push(format!(
                "| {} | {} | {dpi} | {frames} | {:.1} | {} |",
                index + 1,
                rect_text(rect),
                f64::from(frames) / seconds,
                error.as_deref().unwrap_or("なし"),
            ));
        }
        drop(device);
        out.line("| モニター | 矩形 | DPI | 描いたコマ数 | 毎秒 | エラー |");
        out.line("|---|---|---|---|---|---|");
        for row in &rows {
            out.line(row);
        }
        if !completed {
            out.line("結果: 中断した(終了を頼まれた)\n");
            return;
        }
        out.line(&format!(
            "前面のウィンドウを {samples} 回調べ、オーバーレイが前面だったのは {overlay_foreground} 回\
             (0 回なら、オーバーレイはフォーカスを取っていない)"
        ));
        out.line(
            "判定: 目視。報告してほしいこと: (1) 枠と図形がすべてのモニターに見えたか \
             (2) 下のアプリをクリックでき、文字が入ったか (3) オーバーレイの上をクリックしたとき、\
             そのアプリのタイトルバーが前面の色のままだったか (4) 見えなかった・クリックできなかったなら、\
             どの設定で試したか(チェックボックスを変えて再実行した結果も)",
        );
        out.line("");
    }

    // ---- 項目2・3 --------------------------------------------------------------------------

    /// 項目2・3 の四角の一辺(物理ピクセル)。
    const CAPTURE_SIZE: i32 = 200;

    /// DWM が合成し終わるまで待つ時間(ミリ秒)。
    const SETTLE_MS: u32 = 400;

    /// 1回の取り込みの結果。
    struct Measured {
        ratio: f64,
        micros: u128,
    }

    fn measure(
        capture: &mut ScreenCapture,
        rect: RECT,
        capture_blt: bool,
    ) -> Result<Measured, String> {
        let started = Instant::now();
        let pixels = capture
            .capture(rect.left, rect.top, capture_blt)
            .map_err(|error| format!("取り込みに失敗: {}", describe_error(&error)))?;
        let micros = started.elapsed().as_micros();
        Ok(Measured {
            ratio: logic::magenta_ratio(pixels),
            micros,
        })
    }

    fn measured_cell(result: &Result<Measured, String>) -> (String, String) {
        match result {
            Ok(m) => (
                logic::format_percent(m.ratio),
                format!("{:.2} ms", m.micros as f64 / 1000.0),
            ),
            Err(message) => (message.clone(), "-".to_string()),
        }
    }

    /// 4通りの取り込みの結果。
    struct CaptureMatrix {
        plain_without: Result<Measured, String>,
        plain_with: Result<Measured, String>,
        excluded_without: Result<Measured, String>,
        excluded_with: Result<Measured, String>,
        affinity: platform::Result<()>,
        confirmed: platform::Result<WINDOW_DISPLAY_AFFINITY>,
    }

    /// `window`(`rect` を覆う、非表示の四角)を表示し、表示アフィニティのなし・ありそれぞれで、
    /// CAPTUREBLT のなし・ありの2通りを取り込む。表示アフィニティは最後に設定したまま。
    ///
    /// 取り込み先を作れなければ Err、終了を頼まれたら Ok(None)。
    fn capture_matrix(window: Window, rect: RECT) -> Result<Option<CaptureMatrix>, String> {
        let mut capture = ScreenCapture::new(CAPTURE_SIZE, CAPTURE_SIZE)
            .map_err(|error| format!("取り込み先を作れなかった: {}", describe_error(&error)))?;
        window.show(SW_SHOWNOACTIVATE);
        // DWM が合成し終わるまで、メッセージを回しながら待つ。
        if !wait(SETTLE_MS) {
            return Ok(None);
        }
        let plain_without = measure(&mut capture, rect, false);
        let plain_with = measure(&mut capture, rect, true);
        let affinity = window.set_display_affinity(WDA_EXCLUDEFROMCAPTURE);
        let confirmed = window.display_affinity();
        if !wait(SETTLE_MS) {
            return Ok(None);
        }
        let not_measured = || Err("アフィニティを設定できなかったので測っていない".to_string());
        let (excluded_without, excluded_with) = if affinity.is_ok() {
            (
                measure(&mut capture, rect, false),
                measure(&mut capture, rect, true),
            )
        } else {
            (not_measured(), not_measured())
        };
        Ok(Some(CaptureMatrix {
            plain_without,
            plain_with,
            excluded_without,
            excluded_with,
            affinity,
            confirmed,
        }))
    }

    /// 4通りの結果を表にし、判定を書く。
    fn report_matrix(out: &dyn Report, matrix: &CaptureMatrix) {
        match &matrix.affinity {
            Ok(()) => out.line(&format!(
                "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE): 成功(GetWindowDisplayAffinity で {})",
                match &matrix.confirmed {
                    Ok(value) => format!("0x{:X}", value.0),
                    Err(error) => format!("読めず {}", describe_error(error)),
                }
            )),
            Err(error) => out.line(&format!(
                "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE): 失敗 {}",
                describe_win32_error(error)
            )),
        }
        out.line("| 表示アフィニティ | CAPTUREBLT | マゼンタの割合 | 取り込みの時間 |");
        out.line("|---|---|---|---|");
        for (affinity_label, blt, result) in [
            ("なし", false, &matrix.plain_without),
            ("なし", true, &matrix.plain_with),
            ("WDA_EXCLUDEFROMCAPTURE", false, &matrix.excluded_without),
            ("WDA_EXCLUDEFROMCAPTURE", true, &matrix.excluded_with),
        ] {
            let (ratio, time) = measured_cell(result);
            out.line(&format!(
                "| {affinity_label} | {} | {ratio} | {time} |",
                yes_no(blt)
            ));
        }
        let ratio = |result: &Result<Measured, String>| result.as_ref().ok().map(|m| m.ratio);
        out.line(&format!(
            "判定(CAPTUREBLT なし): {}",
            logic::exclusion_text(logic::exclusion(
                ratio(&matrix.plain_without),
                ratio(&matrix.excluded_without)
            ))
        ));
        out.line(&format!(
            "判定(CAPTUREBLT あり): {}",
            logic::exclusion_text(logic::exclusion(
                ratio(&matrix.plain_with),
                ratio(&matrix.excluded_with)
            ))
        ));
        out.line(&format!(
            "CAPTUREBLT の影響: {}",
            logic::captureblt_text(ratio(&matrix.plain_without), ratio(&matrix.plain_with))
        ));
    }

    /// 項目2・3。DirectComposition のオーバーレイ(設定どおり)と、比較用の GDI のレイヤードウィンドウ
    /// (`with_gdi` のときだけ)で、同じ4通りを確かめる。
    fn item23_capture(
        out: &dyn Report,
        classes: &Classes,
        options: OverlayOptions,
        with_gdi: bool,
    ) {
        out.line("## 2・3. キャプチャ除外と CAPTUREBLT");
        out.line(&format!("設定: {}", options.describe()));
        out.line(
            "したこと: 主モニターの中央 200×200px に、不透明のマゼンタ(#FF00FF)の四角を出し、\
             画面 DC から BitBlt で同じ範囲を取り込んで、マゼンタに近い画素の割合を数えた。\
             表示アフィニティ(WDA_EXCLUDEFROMCAPTURE)のなし・あり × CAPTUREBLT のなし・ありの4通り。\
             四角は (A) 製品と同じ DirectComposition のオーバーレイと、(B) 比較用に GDI で塗る\
             レイヤードウィンドウ(ほかのアプリのレイヤードウィンドウの代わり)の2種類",
        );
        let Some(rect) = primary_square(out, CAPTURE_SIZE) else {
            out.line("");
            return;
        };
        out.line(&format!("四角の位置: {}", rect_text(rect)));

        out.line("### (A) DirectComposition のオーバーレイ(上の設定)");
        if let Some(device) = create_device(out, options) {
            match open_overlay(
                classes,
                &device,
                rect,
                options,
                Scene::Magenta,
                Instant::now(),
            ) {
                Ok(overlay) => {
                    report_layered_alpha(out, &overlay);
                    let result = capture_matrix(overlay.window, rect);
                    let (frames, error) = overlay.close();
                    drop(device);
                    out.line(&format!(
                        "オーバーレイの描画: {frames} コマ、エラー {}",
                        error.as_deref().unwrap_or("なし")
                    ));
                    match result {
                        Ok(Some(matrix)) => report_matrix(out, &matrix),
                        Ok(None) => {
                            out.line("結果: 中断した(終了を頼まれた)\n");
                            return;
                        }
                        Err(message) => out.line(&format!("結果: 失敗。{message}")),
                    }
                }
                Err(message) => out.line(&format!("結果: 失敗。{message}")),
            }
        }

        if with_gdi {
            out.line(
                "### (B) 比較: GDI で塗るレイヤードウィンドウ\
                 (WS_EX_LAYERED、WS_EX_TRANSPARENT、SetLayeredWindowAttributes(α=255)、WM_PAINT で塗る)",
            );
            match open_gdi_square(classes, rect) {
                Ok(window) => {
                    let result = capture_matrix(window, rect);
                    let _ = window.destroy();
                    match result {
                        Ok(Some(matrix)) => report_matrix(out, &matrix),
                        Ok(None) => {
                            out.line("結果: 中断した(終了を頼まれた)\n");
                            return;
                        }
                        Err(message) => out.line(&format!("結果: 失敗。{message}")),
                    }
                }
                Err(error) => out.line(&format!(
                    "結果: 失敗。比較用のウィンドウを作れなかった: {}",
                    describe_win32_error(&error)
                )),
            }
        }
        out.line(
            "読み方: アフィニティなしで 100% 近く、ありで 0% なら、自分の BitBlt に写らない(F-10、E-05 を満たす)。\
             アフィニティなしでも 0% なら、四角が表示されていないか BitBlt に写らないため判定できない\
             ((A) ならチェックボックスで設定を変えて試す)。会議アプリの画面共有に写らないかは\
             「2. 画面共有で確認」で目視する",
        );
        if with_gdi {
            out.line(
                "(B) の CAPTUREBLT の影響は、ほかのアプリのレイヤードウィンドウが区画に重なったときの写り方の目安",
            );
        }
        out.line("");
    }

    /// GDI のマゼンタ。
    const MAGENTA: Rgb = Rgb {
        r: 0xFF,
        g: 0x00,
        b: 0xFF,
    };

    /// 比較用の、GDI で塗るレイヤードウィンドウ(非表示で作る)。クリックは素通しし、前面にならない。
    fn open_gdi_square(classes: &Classes, rect: RECT) -> platform::Result<Window> {
        let window = platform::create_window(
            classes.banner,
            &WindowSpec {
                ex_style: WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                style: WS_POPUP,
                title: "",
                x: rect.left,
                y: rect.top,
                width: rect.right.saturating_sub(rect.left),
                height: rect.bottom.saturating_sub(rect.top),
                parent: None,
            },
            |window, message| match message.id() {
                WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
                WM_PAINT => {
                    let paint = window.begin_paint();
                    if let Ok(client) = window.client_rect() {
                        paint.fill_rect(client, MAGENTA);
                    }
                    Some(0)
                }
                _ => None,
            },
        )?;
        if let Err(error) = window.set_layered_alpha(255) {
            let _ = window.destroy();
            return Err(error);
        }
        Ok(window)
    }

    // ---- 項目2(画面共有での目視) ----------------------------------------------------------

    /// 画面共有の確認でオーバーレイを出しておく時間(ミリ秒)。
    const SHARE_MS: u64 = 30_000;

    fn item2_share(out: &dyn Report, classes: &Classes, options: OverlayOptions) {
        out.line("## 2. 画面共有での確認(30秒)");
        out.line(&format!("設定: {}", options.describe()));
        out.line(
            "すること: 先に会議アプリ(Teams、Zoom など)で画面全体の共有を始めておき、このボタンを押す。\
             30秒の間、共有された側の画面(相手の画面、または共有のプレビュー)を見る",
        );
        out.line(
            "見えれば成功: 自分の画面では主モニターの中央にマゼンタの四角が見え、共有された側には写らない\
             (四角の後ろにあるものが写る)",
        );
        let Some(rect) = primary_square(out, CAPTURE_SIZE) else {
            out.line("");
            return;
        };
        let Some(device) = create_device(out, options) else {
            out.line("");
            return;
        };
        let overlay = match open_overlay(
            classes,
            &device,
            rect,
            options,
            Scene::Magenta,
            Instant::now(),
        ) {
            Ok(overlay) => overlay,
            Err(message) => {
                out.line(&format!("結果: 失敗。{message}\n"));
                return;
            }
        };
        report_layered_alpha(out, &overlay);
        match overlay.window.set_display_affinity(WDA_EXCLUDEFROMCAPTURE) {
            Ok(()) => out.line("SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE): 成功"),
            Err(error) => out.line(&format!(
                "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE): 失敗 {}",
                describe_win32_error(&error)
            )),
        }
        overlay.window.show(SW_SHOWNOACTIVATE);
        let started = Instant::now();
        let mut completed = true;
        while let Some(left) = logic::remaining_seconds(millis_since(started), SHARE_MS) {
            out.status(&format!("2. 画面共有の確認中(残り {left} 秒)"));
            if !wait(250) {
                completed = false;
                break;
            }
        }
        let (frames, error) = overlay.close();
        drop(device);
        out.line(&format!(
            "オーバーレイの描画: {frames} コマ、エラー {}",
            error.as_deref().unwrap_or("なし")
        ));
        if !completed {
            out.line("結果: 中断した(終了を頼まれた)\n");
            return;
        }
        out.line(
            "判定: 目視。報告してほしいこと: 会議アプリの名前、自分の画面で四角が見えたか、\
             共有された側で四角が見えたか(見えなければ成功。黒い四角が写ったなら、それも)",
        );
        out.line("");
    }

    // ---- 項目4 -----------------------------------------------------------------------------

    /// 項目4 で取り込む範囲の一辺(物理ピクセル)。
    const ADMIN_CAPTURE_SIZE: i32 = 100;

    fn item4_admin(out: &dyn Report, main: Window) {
        out.line("## 4. 管理者権限のアプリの取り込み");
        out.line(
            "すること: 先に管理者権限で動くアプリ(管理者として実行したメモ帳、またはタスクマネージャー)を\
             開いておく。このボタンを押したら5秒以内にそのアプリをクリックして前面にし、そのまま待つ。\
             アプリの中央に文字や模様が見えている状態にする",
        );
        out.line(
            "したこと: 5秒後に前面のウィンドウの矩形を読み、その中央 100×100px を画面 DC から BitBlt で\
             取り込んで、異なる色の数を数えた。そのアプリが管理者権限かどうかは調べていない(利用者が確かめる)",
        );
        let started = Instant::now();
        while let Some(left) = logic::remaining_seconds(millis_since(started), 5000) {
            out.status(&format!(
                "4. 取り込みまで {left} 秒。管理者権限のアプリを前面に"
            ));
            if !wait(200) {
                out.line("結果: 中断した(終了を頼まれた)\n");
                return;
            }
        }
        let Some(foreground) = platform::foreground_window() else {
            out.line("結果: 失敗。前面のウィンドウがなかった。もう一度試してください\n");
            return;
        };
        let known = [(main.id().value(), "この検証用 exe のメインウィンドウ")];
        out.line(&format!(
            "前面のウィンドウ: {}",
            logic::describe_window(Some(foreground.value()), &known)
        ));
        if foreground == main.id() {
            out.line(
                "結果: やり直し。前面がこの検証用 exe のままだった。5秒以内に管理者権限のアプリをクリックしてください\n",
            );
            return;
        }
        let rect = match foreground.rect() {
            Ok(rect) => rect,
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。ウィンドウの矩形を読めなかった: {}\n",
                    describe_error(&error)
                ));
                return;
            }
        };
        out.line(&format!("ウィンドウの矩形: {}", rect_text(rect)));
        let Some((left, top, width, height)) = logic::center_crop(
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            ADMIN_CAPTURE_SIZE,
        ) else {
            out.line("結果: 失敗。ウィンドウの大きさが 0 だった(最小化されているかもしれない)\n");
            return;
        };
        let mut capture = match ScreenCapture::new(width, height) {
            Ok(capture) => capture,
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。取り込み先を作れなかった: {}\n",
                    describe_error(&error)
                ));
                return;
            }
        };
        out.line(&format!(
            "取り込んだ範囲: ({left}, {top}) から {width}×{height}"
        ));
        let mut best = 0usize;
        for capture_blt in [false, true] {
            match capture.capture(left, top, capture_blt) {
                Ok(pixels) => {
                    let colors = logic::distinct_colors(pixels);
                    best = best.max(colors);
                    out.line(&format!(
                        "- CAPTUREBLT {}: 取り込み成功、異なる色 {colors} 色{}",
                        yes_no(capture_blt),
                        if colors <= 1 {
                            "(全画素が同じ色)"
                        } else {
                            ""
                        }
                    ));
                }
                Err(error) => out.line(&format!(
                    "- CAPTUREBLT {}: 取り込みに失敗 {}",
                    yes_no(capture_blt),
                    describe_error(&error)
                )),
            }
        }
        out.line(&format!(
            "判定(自動): {}",
            if best >= 2 {
                "成功。前面のウィンドウの画面を取り込めた(異なる色が2色以上)"
            } else {
                "失敗。全画素が同じ色か、取り込めなかった(黒や単色で塗られている可能性。アプリの中央が無地なら場所を変えて再実行)"
            }
        ));
        out.line(
            "報告してほしいこと: 前面にしたアプリの名前と、管理者権限で動いていたか\
             (タスクマネージャーの「詳細」タブの「管理者特権」列、または起動時に UAC の確認が出たか)",
        );
        out.line("");
    }

    // ---- 項目5 -----------------------------------------------------------------------------

    fn report_file_size(out: &dyn Report, label: &str, path: &Path) {
        match std::fs::metadata(path) {
            Ok(metadata) => out.line(&format!(
                "- {label}: {}",
                logic::format_size(metadata.len())
            )),
            Err(error) => out.line(&format!("- {label}: 大きさを読めなかった({error})")),
        }
    }

    fn report_memory(out: &dyn Report, label: &str, memory: &platform::Result<MemoryInfo>) {
        match memory {
            Ok(info) => out.line(&format!(
                "- メモリ({label}): 作業セット {}、プライベート使用量 {}、作業セットの最大 {}",
                logic::format_memory(info.working_set as u64),
                logic::format_memory(info.private_bytes as u64),
                logic::format_memory(info.peak_working_set as u64),
            )),
            Err(error) => out.line(&format!(
                "- メモリ({label}): 測れなかった {}",
                describe_error(error)
            )),
        }
    }

    fn item5_size_memory(out: &dyn Report, memories: &[(&str, platform::Result<MemoryInfo>)]) {
        out.line("## 5. サイズとメモリ");
        out.line(
            "したこと: exe の大きさを読み(読むだけ)、K32GetProcessMemoryInfo でこのプロセスの\
             作業セットとプライベート使用量を測った",
        );
        match std::env::current_exe() {
            Ok(path) => {
                report_file_size(out, "この exe(m0_probe.exe)", &path);
                // 同じフォルダーに製品の exe があれば、その大きさも出す(CI の成果物は同じフォルダーに入る)。
                let product = path.with_file_name("Monitoaster.exe");
                if product.is_file() {
                    report_file_size(out, "同じフォルダーの Monitoaster.exe", &product);
                }
            }
            Err(error) => out.line(&format!("- exe の場所を取れなかった({error})")),
        }
        for (label, memory) in memories {
            report_memory(out, label, memory);
        }
        out.line(
            "判定: 自動(値の記録)。目標は exe 3MB 以下、監視中のメモリ 25MB 以下(spec.md 8章)。\
             この値は検証用 exe のもの。製品の骨組みのメモリは、Monitoaster.exe を起動して\
             タスクマネージャーの「詳細」タブの「メモリ(アクティブなプライベート ワーキング セット)」で見る",
        );
        out.line("");
    }

    // ---- 項目6 -----------------------------------------------------------------------------

    /// トレイのアイコンの番号。
    const TRAY_ID: u32 = 1;

    /// トレイのアイコンへの操作が届くメッセージ。
    const TRAY_CALLBACK: u32 = WM_APP + 1;

    fn item6_tray(out: &dyn Report, main: Window, tray: &RefCell<Option<TrayIcon>>) {
        out.line("## 6. バルーン通知");
        out.line(
            "したこと: Shell_NotifyIconW で通知領域にアイコンを足し(NOTIFYICON_VERSION_4)、\
             題と本文のある通知を1件出した。アイコンはこの exe を閉じるときに消す(NIM_DELETE)",
        );
        let Ok(mut tray) = tray.try_borrow_mut() else {
            out.line("結果: 失敗(内部の状態を使えなかった)\n");
            return;
        };
        if tray.is_none() {
            match TrayIcon::add(main, TRAY_ID, TRAY_CALLBACK, "Monitoaster 検証用") {
                Ok(icon) => {
                    *tray = Some(icon);
                    out.line("アイコンの追加: 成功");
                }
                Err(error) => {
                    out.line(&format!(
                        "結果: 失敗。アイコンを足せなかった: {}\n",
                        describe_error(&error)
                    ));
                    return;
                }
            }
        } else {
            out.line("アイコンの追加: 済み(前に足したものを使う)");
        }
        if let Some(icon) = tray.as_ref() {
            match icon.notify(
                "Monitoaster(検証)",
                "区画の表示が変わりました。これは M0 の検証用の通知です",
            ) {
                Ok(()) => out.line("通知: 成功(Shell_NotifyIconW が受け付けた)"),
                Err(error) => out.line(&format!("通知: 失敗 {}", describe_error(&error))),
            }
        }
        out.line(
            "見ること: (1) 画面の右下に通知が出るか。題・本文・アイコン・アプリ名(何と表示されるか)\
             (2) 通知センター(Win+N)に残るか (3) この exe を閉じたあとも通知センターに残るか \
             (4) 通知領域(タスクバーの ^ の中)にアイコンが出ているか。\
             「応答不可」(集中モード)がオンだと出ないことがあるので、その状態も添える",
        );
        out.line("判定: 目視。見えたものをそのまま報告してください(通知のスクリーンショットがあると助かります)");
        out.line("");
    }

    // ---- 項目7 -----------------------------------------------------------------------------

    fn item7_init(out: &dyn Report, classes: &Classes, options: OverlayOptions) {
        out.line(&format!(
            "## 7. 描画の初期化(D3D: {})",
            if options.warp_only {
                "WARP だけ"
            } else {
                "ハードウェア→だめなら WARP"
            }
        ));
        out.line(
            "したこと: 非表示のウィンドウを作り、D3D11 デバイスから Direct2D のデバイスコンテキストまでを\
             1段ずつ作って、1回だけ透明に描いて Present した。作ったものはすべて解放した。\
             ある段が失敗しても、それに頼らない段は続けて試した",
        );
        let window = match platform::create_window(
            classes.hidden,
            &WindowSpec {
                ex_style: options.ex_style(),
                style: WS_POPUP,
                title: "",
                x: 0,
                y: 0,
                width: 64,
                height: 64,
                parent: None,
            },
            |_, _| None,
        ) {
            Ok(window) => window,
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。非表示のウィンドウを作れなかった: {}\n",
                    describe_error(&error)
                ));
                return;
            }
        };
        let started = Instant::now();
        let result = probe_support::staged_graphics_init(options.preference(), window);
        let elapsed_ms = millis_since(started);
        let _ = window.destroy();
        out.line("| 段 | 結果 |");
        out.line("|---|---|");
        for (stage, outcome) in &result.stages {
            let text = match outcome {
                StageOutcome::Done => "成功".to_string(),
                StageOutcome::Failed(error) => format!("失敗 {}", describe_error(error)),
                StageOutcome::NotRun => "未実行(前の段の失敗のため)".to_string(),
            };
            out.line(&format!("| {} | {text} |", stage_name(*stage)));
        }
        match (result.driver, result.feature_level, &result.adapter) {
            (Some(driver), Some((major, minor)), Some(adapter)) => out.line(&format!(
                "ドライバー: {}、機能レベル {major}_{minor}、アダプター: {}",
                driver_name(driver),
                describe_adapter(adapter)
            )),
            _ => out.line("ドライバー: D3D11 デバイスを作れなかった"),
        }
        if let Some(error) = &result.hardware_error {
            out.line(&format!("ハードウェアでの失敗: {}", describe_error(error)));
        }
        let all_done = result
            .stages
            .iter()
            .all(|(_, outcome)| matches!(outcome, StageOutcome::Done));
        out.line(&format!(
            "判定(自動): {}(所要 {elapsed_ms} ms)",
            if all_done {
                "成功。すべての段が通った"
            } else {
                "失敗。通らなかった段がある(製品では E-06 として、バナー・音・トレイ通知だけで知らせる)"
            }
        ));
        out.line("");
    }

    // ---- 項目8 -----------------------------------------------------------------------------

    /// バナーを出しておく時間(ミリ秒)。
    const BANNER_MS: u64 = 20_000;

    /// 前面のウィンドウを記録するタイマー(100ms ごと)。
    const SAMPLE_TIMER: usize = 1;

    /// クリックの 200ms 後に前面のウィンドウを記録するタイマー。
    const AFTER_TIMER: usize = 2;

    /// 1回のクリックの記録(前面のウィンドウのハンドルの値)。
    #[derive(Clone, Copy)]
    struct ClickRecord {
        at_ms: u64,
        before: Option<usize>,
        at: Option<usize>,
        after: Option<usize>,
    }

    /// バナーの処理と共有する状態。
    struct BannerState {
        started: Instant,
        /// 100ms ごとの記録の、最後の値。
        last_sample: Cell<Option<usize>>,
        /// WM_MOUSEACTIVATE を受け取った時点の前面のウィンドウ(クリックの直前)。
        pending_before: Cell<Option<Option<usize>>>,
        /// 前面のウィンドウが変わった時刻と、変わった先。
        changes: RefCell<Vec<(u64, Option<usize>)>>,
        clicks: RefCell<Vec<ClickRecord>>,
        mouse_activations: Cell<u32>,
        font: RefCell<Option<Font>>,
    }

    impl BannerState {
        fn sample(&self) {
            let now = platform::foreground_window().map(|id| id.value());
            if now != self.last_sample.get() {
                self.changes
                    .borrow_mut()
                    .push((millis_since(self.started), now));
            }
            self.last_sample.set(now);
        }
    }

    fn banner_message(state: &BannerState, window: Window, message: Message) -> Option<isize> {
        match message.id() {
            WM_MOUSEACTIVATE => {
                // クリックの直前(前面にするかを OS が決める前)の前面のウィンドウを記録する。
                state
                    .pending_before
                    .set(Some(platform::foreground_window().map(|id| id.value())));
                state
                    .mouse_activations
                    .set(state.mouse_activations.get().saturating_add(1));
                Some(MA_NOACTIVATE as isize)
            }
            WM_TIMER => match message.timer_id() {
                Some(SAMPLE_TIMER) => {
                    state.sample();
                    Some(0)
                }
                Some(AFTER_TIMER) => {
                    let _ = window.kill_timer(AFTER_TIMER);
                    let now = platform::foreground_window().map(|id| id.value());
                    for click in state.clicks.borrow_mut().iter_mut() {
                        if click.after.is_none() {
                            click.after = now;
                        }
                    }
                    Some(0)
                }
                _ => None,
            },
            WM_LBUTTONDOWN => {
                let at = platform::foreground_window().map(|id| id.value());
                let before = state
                    .pending_before
                    .take()
                    .unwrap_or_else(|| state.last_sample.get());
                state.clicks.borrow_mut().push(ClickRecord {
                    at_ms: millis_since(state.started),
                    before,
                    at,
                    after: None,
                });
                window.invalidate();
                let _ = window.set_timer(AFTER_TIMER, 200);
                Some(0)
            }
            WM_PAINT => {
                let paint = window.begin_paint();
                if let Ok(rect) = window.client_rect() {
                    let count = state.clicks.borrow().len();
                    let color = if count.is_multiple_of(2) {
                        Rgb {
                            r: 0x1F,
                            g: 0x5F,
                            b: 0xBF,
                        }
                    } else {
                        Rgb {
                            r: 0xD9,
                            g: 0x6A,
                            b: 0x00,
                        }
                    };
                    paint.fill_rect(rect, color);
                    let inset = RECT {
                        left: rect.left + 8,
                        top: rect.top + 8,
                        right: rect.right - 8,
                        bottom: rect.bottom - 8,
                    };
                    paint.draw_text(
                        &format!("検証用バナー(クリックしてください)\nクリック {count} 回"),
                        inset,
                        Rgb {
                            r: 0xFF,
                            g: 0xFF,
                            b: 0xFF,
                        },
                        state.font.borrow().as_ref(),
                    );
                }
                Some(0)
            }
            _ => None,
        }
    }

    fn item8_banner(out: &dyn Report, classes: &Classes, main: Window) {
        out.line("## 8. フォーカスを奪わないバナー(20秒)");
        out.line(
            "すること: (1) メモ帳を開き、クリックして前面にする (2) 画面上部に出る青い小窓(バナー)をクリックする \
             (3) そのまま文字を打ち、メモ帳に入るかを見る。何回クリックしてもよい。20秒で閉じる",
        );
        out.line(
            "見えれば成功: クリックするたびにバナーの色が変わり、メモ帳のタイトルバーは前面の色のまま、\
             打った文字がメモ帳に入る",
        );
        let state = Rc::new(BannerState {
            started: Instant::now(),
            last_sample: Cell::new(None),
            pending_before: Cell::new(None),
            changes: RefCell::new(Vec::new()),
            clicks: RefCell::new(Vec::new()),
            mouse_activations: Cell::new(0),
            font: RefCell::new(None),
        });
        let handler_state = Rc::clone(&state);
        let window = match platform::create_window(
            classes.banner,
            &WindowSpec {
                ex_style: WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                style: WS_POPUP,
                title: "",
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                parent: None,
            },
            move |window, message| banner_message(&handler_state, window, message),
        ) {
            Ok(window) => window,
            Err(error) => {
                out.line(&format!(
                    "結果: 失敗。バナーを作れなかった: {}\n",
                    describe_error(&error)
                ));
                return;
            }
        };
        // 主モニターの作業領域の上端の中央に置く。大きさはバナーの置かれたモニターの DPI に合わせる。
        let work = platform::monitors()
            .ok()
            .and_then(|list| list.into_iter().find(|m| m.primary))
            .map(|m| m.work_rect)
            .unwrap_or(RECT {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 768,
            });
        let banner_rect = |dpi| {
            let width = platform::scale_for_dpi(420, dpi);
            let height = platform::scale_for_dpi(96, dpi);
            let (x, _) = logic::centered(work.left, work.top, work.right, work.bottom, width);
            (
                x,
                work.top + platform::scale_for_dpi(24, dpi),
                width,
                height,
            )
        };
        let (x, y, width, height) = banner_rect(window.dpi());
        let _ = window.set_pos(None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
        // モニターが変わって DPI が変わったら、置き直す。
        let dpi = window.dpi();
        let (x, y, width, height) = banner_rect(dpi);
        let _ = window.set_pos(None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
        *state.font.borrow_mut() = Font::message_font(dpi).ok();
        state.sample();
        if let Err(error) = window.set_timer(SAMPLE_TIMER, 100) {
            out.line(&format!(
                "- 記録のタイマーを設定できなかった: {}",
                describe_error(&error)
            ));
        }
        window.show(SW_SHOWNOACTIVATE);
        let started = Instant::now();
        let mut completed = true;
        while let Some(left) = logic::remaining_seconds(millis_since(started), BANNER_MS) {
            out.status(&format!(
                "8. バナーを表示中(残り {left} 秒)。メモ帳を前面にしてからバナーをクリック"
            ));
            if !wait(250) {
                completed = false;
                break;
            }
        }
        let banner_id = window.id().value();
        let _ = window.kill_timer(SAMPLE_TIMER);
        let _ = window.kill_timer(AFTER_TIMER);
        let _ = window.destroy();
        if !completed {
            out.line("結果: 中断した(終了を頼まれた)\n");
            return;
        }
        let known = [
            (main.id().value(), "この検証用 exe のメインウィンドウ"),
            (banner_id, "バナー"),
        ];
        let clicks = state.clicks.borrow();
        out.line(&format!(
            "バナー: {}。WM_MOUSEACTIVATE を {} 回受け取り、すべて MA_NOACTIVATE を返した",
            logic::format_handle(banner_id),
            state.mouse_activations.get()
        ));
        if clicks.is_empty() {
            out.line(
                "結果: クリックが記録されなかった。バナーをクリックしてから20秒待ってください\n",
            );
            return;
        }
        out.line("| 回 | 時刻 | 直前 | 直後 | 200ms 後 | 前面は同じか |");
        out.line("|---|---|---|---|---|---|");
        for (index, click) in clicks.iter().enumerate() {
            out.line(&format!(
                "| {} | {:.1} 秒 | {} | {} | {} | {} |",
                index + 1,
                click.at_ms as f64 / 1000.0,
                logic::describe_window(click.before, &known),
                logic::describe_window(click.at, &known),
                logic::describe_window(click.after, &known),
                if logic::focus_kept(click.before, click.at, click.after) {
                    "同じ"
                } else {
                    "変わった"
                },
            ));
        }
        out.line("前面のウィンドウの移り変わり(100ms ごとの記録):");
        for (at_ms, value) in state.changes.borrow().iter() {
            out.line(&format!(
                "- {:.1} 秒: {}",
                *at_ms as f64 / 1000.0,
                logic::describe_window(*value, &known)
            ));
        }
        let all_kept = clicks
            .iter()
            .all(|c| logic::focus_kept(c.before, c.at, c.after));
        let banner_took_focus = clicks
            .iter()
            .any(|c| c.at == Some(banner_id) || c.after == Some(banner_id));
        let main_before = clicks.iter().any(|c| c.before == Some(main.id().value()));
        out.line(&format!(
            "判定(自動): {}",
            if banner_took_focus {
                "失敗。バナーが前面になった"
            } else if all_kept {
                "成功。クリックの直前・直後・200ms 後で、前面のウィンドウが変わらなかった"
            } else {
                "要確認。クリックの前後で前面のウィンドウが変わった(バナー以外へ。表で確かめる)"
            }
        ));
        if main_before {
            out.line(
                "注意: クリックの直前の前面が、この検証用 exe のメインウィンドウだった。\
                 メモ帳を前面にしてからバナーをクリックすると、確かめたいことを試せる",
            );
        }
        out.line("報告してほしいこと: クリックのあとに打った文字がメモ帳に入ったか、バナーの色が変わったか");
        out.line("");
    }

    // ---- --headless ------------------------------------------------------------------------

    fn report_dll_search(out: &dyn Report, result: &platform::Result<()>) {
        match result {
            Ok(()) => out.line("S-09: SetDefaultDllDirectories(System32 だけ): 成功"),
            Err(error) => out.line(&format!(
                "S-09: SetDefaultDllDirectories(System32 だけ): 失敗 {}",
                describe_error(error)
            )),
        }
    }

    fn run_headless(dll_search: platform::Result<()>, startup: platform::Result<MemoryInfo>) {
        let out = StdoutReport;
        let started = Instant::now();
        out.line("# Monitoaster M0 技術検証(m0_probe.exe --headless)");
        out.line("自動で判定できる項目(5、7、2・3)だけを実行する。失敗しても次の項目へ進む");
        report_dll_search(&out, &dll_search);
        out.line("");
        item5_size_memory(&out, &[("起動直後", startup)]);
        match Classes::register() {
            Ok(classes) => {
                item7_init(&out, &classes, OverlayOptions::SPEC);
                item7_init(
                    &out,
                    &classes,
                    OverlayOptions {
                        warp_only: true,
                        ..OverlayOptions::SPEC
                    },
                );
                // 比較用の GDI のレイヤードウィンドウは、設定に関係しないので1回だけ。
                item23_capture(&out, &classes, OverlayOptions::SPEC, true);
                item23_capture(
                    &out,
                    &classes,
                    OverlayOptions {
                        layered_alpha: true,
                        ..OverlayOptions::SPEC
                    },
                    false,
                );
            }
            Err(error) => out.line(&format!(
                "ウィンドウクラスを登録できなかったので、7 と 2・3 は実行していない: {}\n",
                describe_error(&error)
            )),
        }
        out.line("## 終わり");
        report_memory(&out, "終了前", &probe_support::memory_info());
        out.line(&format!("所要時間: {} ms", millis_since(started)));
    }

    // ---- 画面 ------------------------------------------------------------------------------

    /// 項目のボタンが頼む仕事。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Action {
        Overlay,
        Capture,
        Share,
        Admin,
        SizeMemory,
        Tray,
        Init,
        Banner,
    }

    /// 項目のボタン(順に、番号 [`ID_ACTION_BASE`] から)。
    const ACTIONS: [(Action, &str); 8] = [
        (Action::Overlay, "1. オーバーレイ(5秒)"),
        (Action::Capture, "2・3. キャプチャ除外と CAPTUREBLT"),
        (Action::Share, "2. 画面共有で確認(30秒)"),
        (Action::Admin, "4. 管理者権限のアプリ(5秒後に取り込み)"),
        (Action::SizeMemory, "5. サイズとメモリ"),
        (Action::Tray, "6. バルーン通知"),
        (Action::Init, "7. 描画の初期化"),
        (Action::Banner, "8. フォーカスを奪わないバナー(20秒)"),
    ];

    /// オーバーレイの設定のチェックボックス(順に、番号 [`ID_CHECK_BASE`] から)。
    const CHECKS: [&str; 5] = [
        "WS_EX_LAYERED を付ける",
        "SetLayeredWindowAttributes(α=255)を呼ぶ",
        "WS_EX_TRANSPARENT を付ける",
        "WS_EX_NOREDIRECTIONBITMAP を付ける",
        "D3D は WARP だけを使う",
    ];

    const ID_ACTION_BASE: u16 = 100;
    const ID_SELECT_ALL: u16 = 120;
    const ID_LABEL: u16 = 121;
    const ID_CHECK_BASE: u16 = 200;
    const ID_STATUS: u16 = 300;
    const ID_LOG: u16 = 301;

    /// メインウィンドウのクライアント領域の大きさ(96 DPI 基準)。
    const CLIENT_WIDTH: i32 = 980;
    const CLIENT_HEIGHT: i32 = 600;

    /// メインウィンドウの子コントロール。
    #[derive(Clone, Copy)]
    struct Controls {
        buttons: [Window; 8],
        select_all: Window,
        label: Window,
        checks: [Window; 5],
        status: Window,
        log: Window,
    }

    impl Controls {
        fn create(main: Window) -> platform::Result<Self> {
            let child = |class: ControlClass,
                         id: u16,
                         title: &str,
                         style: WINDOW_STYLE,
                         ex_style: WINDOW_EX_STYLE| {
                platform::create_control(
                    class,
                    id,
                    &WindowSpec {
                        ex_style,
                        style: style | WS_VISIBLE,
                        title,
                        parent: Some(main),
                        ..Default::default()
                    },
                )
            };
            let push = WINDOW_STYLE(BS_PUSHBUTTON as u32) | WS_TABSTOP;
            let check = WINDOW_STYLE(BS_AUTOCHECKBOX as u32) | WS_TABSTOP;
            let none = WINDOW_EX_STYLE(0);
            let mut buttons = [main; 8];
            for (index, (_, label)) in ACTIONS.iter().enumerate() {
                buttons[index] = child(
                    ControlClass::Button,
                    ID_ACTION_BASE + index as u16,
                    label,
                    push,
                    none,
                )?;
            }
            let select_all = child(
                ControlClass::Button,
                ID_SELECT_ALL,
                "結果をすべて選択(このあと Ctrl+C で写す)",
                push,
                none,
            )?;
            let label = child(
                ControlClass::Static,
                ID_LABEL,
                "オーバーレイの設定(1、2・3、2 の共有、7 で使う)",
                WINDOW_STYLE(0),
                none,
            )?;
            let mut checks = [main; 5];
            for (index, text) in CHECKS.iter().enumerate() {
                checks[index] = child(
                    ControlClass::Button,
                    ID_CHECK_BASE + index as u16,
                    text,
                    check,
                    none,
                )?;
            }
            let status = child(ControlClass::Static, ID_STATUS, "", WINDOW_STYLE(0), none)?;
            let log = child(
                ControlClass::Edit,
                ID_LOG,
                "",
                WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32) | WS_VSCROLL,
                WS_EX_CLIENTEDGE,
            )?;
            log.remove_text_limit();
            let controls = Self {
                buttons,
                select_all,
                label,
                checks,
                status,
                log,
            };
            controls.set_options(OverlayOptions::SPEC);
            Ok(controls)
        }

        fn all(&self) -> impl Iterator<Item = Window> + '_ {
            self.buttons
                .iter()
                .chain([&self.select_all, &self.label])
                .chain(self.checks.iter())
                .chain([&self.status, &self.log])
                .copied()
        }

        fn set_options(&self, options: OverlayOptions) {
            self.checks[0].set_checked(options.layered);
            self.checks[1].set_checked(options.layered_alpha);
            self.checks[2].set_checked(options.transparent);
            self.checks[3].set_checked(options.no_redirection);
            self.checks[4].set_checked(options.warp_only);
        }

        fn options(&self) -> OverlayOptions {
            OverlayOptions {
                layered: self.checks[0].is_checked(),
                layered_alpha: self.checks[1].is_checked(),
                transparent: self.checks[2].is_checked(),
                no_redirection: self.checks[3].is_checked(),
                warp_only: self.checks[4].is_checked(),
            }
        }

        fn report(&self) -> WindowReport {
            WindowReport {
                log: self.log,
                status: self.status,
            }
        }

        /// 置き場所を決める。左に項目のボタンと設定、右に状態欄と結果欄。
        fn layout(&self, main: Window) {
            let dpi = main.dpi();
            let s = |value| platform::scale_for_dpi(value, dpi);
            let Ok(client) = main.client_rect() else {
                return;
            };
            let place = |window: Window, x, y, width, height| {
                let _ = window.set_pos(None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
            };
            let margin = s(10);
            let column = s(340);
            let mut y = margin;
            for button in &self.buttons {
                place(*button, margin, y, column, s(30));
                y += s(34);
            }
            y += s(6);
            place(self.select_all, margin, y, column, s(30));
            y += s(44);
            place(self.label, margin, y, column, s(20));
            y += s(24);
            for check in &self.checks {
                place(*check, margin, y, column, s(24));
                y += s(26);
            }
            let x = margin + column + margin;
            let width = (client.right - x - margin).max(s(120));
            place(self.status, x, margin, width, s(22));
            let top = margin + s(28);
            place(
                self.log,
                x,
                top,
                width,
                (client.bottom - top - margin).max(s(80)),
            );
        }
    }

    /// メインウィンドウの処理と、画面の仕事のループが共有する状態。
    struct Shared {
        /// ボタンが頼んだ仕事。処理の外(仕事のループ)で実行する。
        queue: RefCell<VecDeque<Action>>,
        /// 仕事を実行中なら true。
        busy: Cell<bool>,
        controls: OnceCell<Controls>,
        font: RefCell<Option<Font>>,
        tray: RefCell<Option<TrayIcon>>,
    }

    fn main_message(shared: &Shared, window: Window, message: Message) -> Option<isize> {
        match message.id() {
            WM_SIZE => {
                if let Some(controls) = shared.controls.get() {
                    controls.layout(window);
                }
                Some(0)
            }
            WM_DPICHANGED => {
                let change = message.dpi_changed()?;
                let rect = change.suggested_rect;
                let _ = window.set_pos(
                    None,
                    rect.left,
                    rect.top,
                    rect.right.saturating_sub(rect.left),
                    rect.bottom.saturating_sub(rect.top),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(controls) = shared.controls.get() {
                    if let Ok(font) = Font::message_font(change.dpi) {
                        for control in controls.all() {
                            control.set_font(&font);
                        }
                        // 新しいフォントを設定してから、古いフォントを捨てる。
                        if let Ok(mut slot) = shared.font.try_borrow_mut() {
                            *slot = Some(font);
                        }
                    }
                    controls.layout(window);
                }
                Some(0)
            }
            WM_COMMAND => {
                let command = message.command()?;
                if u32::from(command.code) != BN_CLICKED {
                    return None;
                }
                let controls = shared.controls.get()?;
                if command.id == ID_SELECT_ALL {
                    controls.log.select_all_text();
                    controls.log.set_focus();
                    return Some(0);
                }
                let index = usize::from(command.id.checked_sub(ID_ACTION_BASE)?);
                let (action, _) = ACTIONS.get(index)?;
                if shared.busy.get() || !shared.queue.borrow().is_empty() {
                    controls.report().line(
                        "(実行中の項目があるため、押したボタンは無視しました。終わってから押してください)",
                    );
                } else {
                    shared.queue.borrow_mut().push_back(*action);
                }
                Some(0)
            }
            WM_DESTROY => {
                // ウィンドウがあるうちにトレイのアイコンを消す。
                if let Ok(mut tray) = shared.tray.try_borrow_mut() {
                    drop(tray.take());
                }
                platform::post_quit(0);
                Some(0)
            }
            _ => None,
        }
    }

    fn intro(out: &dyn Report, dll_search: &platform::Result<()>) {
        out.line("# Monitoaster M0 技術検証(m0_probe)");
        out.line("使い方: 左のボタンで1項目ずつ実行します。結果はこの欄に追記されます。");
        out.line("- 各項目の「すること」に従って操作し、「見えれば成功」と見比べてください");
        out.line(
            "- 押す前の準備(1、2 の共有、4、8 は、押すとすぐに数え始めます): \
             1 は、主モニターにメモ帳などを開いておく。\
             2 の共有は、会議アプリで画面全体の共有を先に始めておく。\
             4 は、管理者権限のアプリ(管理者として実行したメモ帳、またはタスクマネージャー)を先に開いておく。\
             8 は、メモ帳を開いておく",
        );
        out.line("- 最後に「結果をすべて選択」を押し、Ctrl+C で写して報告に貼り付けてください");
        out.line("- 報告に添えてほしいこと: Windows のバージョン(winver で確認)、モニターの台数と拡大率、GPU の種類");
        out.line("- 1、2・3、7 で問題が出たら、左下のチェックボックスで設定を変えて、もう一度試してください");
        out.line(
            "この exe はファイルに書かず、通信もしません。閉じるとトレイのアイコンも消えます。",
        );
        report_dll_search(out, dll_search);
        out.line("");
    }

    fn run_gui(dll_search: platform::Result<()>, startup: platform::Result<MemoryInfo>) {
        // エクスプローラーから起動したときに開くコンソールの窓を閉じる(標準出力は使わない)。
        probe_support::detach_console();
        let Ok(classes) = Classes::register() else {
            return;
        };
        let shared = Rc::new(Shared {
            queue: RefCell::new(VecDeque::new()),
            busy: Cell::new(false),
            controls: OnceCell::new(),
            font: RefCell::new(None),
            tray: RefCell::new(None),
        });
        let handler_shared = Rc::clone(&shared);
        let style = WS_OVERLAPPEDWINDOW;
        let Ok(main) = platform::create_window(
            classes.main,
            &WindowSpec {
                ex_style: WINDOW_EX_STYLE(0),
                style,
                title: "Monitoaster M0 技術検証",
                x: CW_USEDEFAULT,
                y: CW_USEDEFAULT,
                width: CW_USEDEFAULT,
                height: CW_USEDEFAULT,
                parent: None,
            },
            move |window, message| main_message(&handler_shared, window, message),
        ) else {
            return;
        };
        let dpi = main.dpi();
        let client = RECT {
            left: 0,
            top: 0,
            right: platform::scale_for_dpi(CLIENT_WIDTH, dpi),
            bottom: platform::scale_for_dpi(CLIENT_HEIGHT, dpi),
        };
        if let Ok(outer) = platform::adjust_window_rect(client, style, WINDOW_EX_STYLE(0), dpi) {
            let _ = main.set_pos(
                None,
                0,
                0,
                outer.right.saturating_sub(outer.left),
                outer.bottom.saturating_sub(outer.top),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let controls = match Controls::create(main) {
            Ok(controls) => controls,
            Err(_) => {
                let _ = main.destroy();
                return;
            }
        };
        if let Ok(font) = Font::message_font(dpi) {
            for control in controls.all() {
                control.set_font(&font);
            }
            *shared.font.borrow_mut() = Some(font);
        }
        let _ = shared.controls.set(controls);
        controls.layout(main);
        main.show(SW_SHOWDEFAULT);

        let report = controls.report();
        intro(&report, &dll_search);
        let shown = probe_support::memory_info();
        item5_size_memory(
            &report,
            &[
                ("起動直後", startup.clone()),
                ("画面を出した後", shown.clone()),
            ],
        );
        report.status("待機中。左のボタンで項目を実行");

        // 仕事のループ。メッセージを1つ処理するごとに、ボタンが頼んだ仕事を見る。仕事は処理の外で
        // 実行するので、仕事の中でメッセージを回しても、メインウィンドウの処理は普通に呼ばれる。
        while platform::wait_message() == Pump::Continue {
            loop {
                let next = shared.queue.borrow_mut().pop_front();
                let Some(action) = next else {
                    break;
                };
                shared.busy.set(true);
                let options = controls.options();
                match action {
                    Action::Overlay => item1_overlay(&report, &classes, options),
                    Action::Capture => item23_capture(&report, &classes, options, true),
                    Action::Share => item2_share(&report, &classes, options),
                    Action::Admin => item4_admin(&report, main),
                    Action::SizeMemory => item5_size_memory(
                        &report,
                        &[
                            ("起動直後", startup.clone()),
                            ("画面を出した後", shown.clone()),
                            ("今", probe_support::memory_info()),
                        ],
                    ),
                    Action::Tray => item6_tray(&report, main, &shared.tray),
                    Action::Init => item7_init(&report, &classes, options),
                    Action::Banner => item8_banner(&report, &classes, main),
                }
                shared.busy.set(false);
                report.status("待機中。左のボタンで項目を実行");
            }
        }
        // WM_DESTROY で消していなければ、ここでトレイのアイコンを消す。
        drop(shared.tray.borrow_mut().take());
    }
}
