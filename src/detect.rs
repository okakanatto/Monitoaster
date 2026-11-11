//! 判定ロジック。区画の画像を1コマずつ受け取り、条件を満たしたかを判定する。
//!
//! OS に依存しない純粋なロジックで、Linux 上でも単体テストできる。
//! Python 試作の `Detector` の移植で、挙動は試作に合わせている(違いは各項目のdocに書く)。
//!
//! - 「違う画素」: B・G・R のいずれかの差の絶対値が許容差を**厳密に超える**画素。
//! - 「指定色に近い画素」: B・G・R すべての差の絶対値が許容差以内の画素。
//! - 画素数が [`MAX_SAMPLES`] を超えるフレームは縦横同じ整数ステップで間引いて数える。

#![forbid(unsafe_code)]

/// 間引かずに数える画素数の上限。これを超えるフレームは間引く。
pub const MAX_SAMPLES: u64 = 1_000_000;

/// `interval_ms` の下限(ミリ秒)。これより短い間隔は下限として扱う。
pub const MIN_INTERVAL_MS: u32 = 50;

/// 何を検知するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 最初のコマ(基準)から変化した。
    Change,
    /// 一度動いたあと、静止した。
    Still,
    /// 指定色が現れた。
    ColorIn,
    /// 一度現れた指定色が消えた。
    ColorOut,
}

/// 判定の設定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// 何を検知するか。
    pub trigger: Trigger,
    /// 色成分ごとの許容差。差がこの値と等しい場合は「同じ」とみなす。
    pub tolerance: u8,
    /// 判定に使う面積の割合(0.0〜1.0)。
    pub area_ratio: f64,
    /// 条件がこの時間続いたら発火する(ミリ秒)。
    pub hold_ms: u32,
    /// コマを渡す間隔(ミリ秒)。[`MIN_INTERVAL_MS`] 未満は下限として扱う。
    pub interval_ms: u32,
    /// 色モードで見る色(R, G, B の順)。
    pub color: [u8; 3],
}

/// 1コマの画像。
///
/// 上から下へ並び、1画素4バイト(B, G, R, X)、1行は `width * 4` バイト(詰め物なし)。
/// 4バイト目(X)は判定に使わない。
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// 幅(画素)。
    pub width: u32,
    /// 高さ(画素)。
    pub height: u32,
    /// 画素データ。長さは `width * height * 4` でなければならない。
    pub bgra: &'a [u8],
}

/// 1コマ判定した結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// 現在値(0.0〜1.0)。メーター表示用。
    pub value: f64,
    /// 発火したか。条件が続く限り true のままになる(止めるのは呼び出し側)。
    pub fired: bool,
}

/// 何コマ続けて条件を満たせば発火するか。
///
/// `max(1, ceil(hold_ms / max(interval_ms, 50)))` を整数演算で求める。
/// 試作は浮動小数で計算していたため 900/300 が 4 になっていたが、正しくは 3。
pub fn required_streak(hold_ms: u32, interval_ms: u32) -> u32 {
    hold_ms.div_ceil(interval_ms.max(MIN_INTERVAL_MS)).max(1)
}

/// 色モードで直前の値を使い回すための記録。
#[derive(Debug, Clone, Copy)]
struct ColorCache {
    color: [u8; 3],
    tolerance: u8,
    value: f64,
}

/// 判定器。コマを [`Detector::feed`] に順に渡して使う。
///
/// 基準コマと直前コマのバッファは内部に持ち、確保した領域を使い回す。
#[derive(Debug)]
pub struct Detector {
    /// 基準コマ(Change で使う)。
    base: Vec<u8>,
    has_base: bool,
    /// 直前コマ(Still と色モードで使う)。
    prev: Vec<u8>,
    has_prev: bool,
    /// 直前に受け取ったコマの大きさ。
    size: Option<(u32, u32)>,
    /// 直前に使ったトリガー。
    trigger: Option<Trigger>,
    /// 条件を続けて満たしたコマ数。
    streak: u32,
    /// Still / ColorOut で、一度「動いた」「現れた」か。
    primed: bool,
    /// 色モードの直前の値。
    color_cache: Option<ColorCache>,
    /// 完全一致のときに計算を省くか。テストで省かない場合と比べるためだけに切り替える。
    skip_identical: bool,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

impl Detector {
    /// 空の判定器を作る。
    pub fn new() -> Self {
        Self {
            base: Vec::new(),
            has_base: false,
            prev: Vec::new(),
            has_prev: false,
            size: None,
            trigger: None,
            streak: 0,
            primed: false,
            color_cache: None,
            skip_identical: true,
        }
    }

    /// 基準・直前コマ・連続数・primed を捨てる。次のコマは最初のコマとして扱う。
    ///
    /// バッファの領域は解放せず、次に使い回す。
    pub fn reset(&mut self) {
        self.base.clear();
        self.has_base = false;
        self.prev.clear();
        self.has_prev = false;
        self.size = None;
        self.trigger = None;
        self.streak = 0;
        self.primed = false;
        self.color_cache = None;
    }

    /// 1コマ渡して判定する。
    ///
    /// フレームが不正(幅か高さが0、またはデータの長さが `width * height * 4` でない)なら
    /// `None` を返し、内部状態は変えない。
    ///
    /// 区画の大きさ、またはトリガーが前のコマと変わったら、自動で [`Detector::reset`] して
    /// 最初のコマとして扱う。
    pub fn feed(&mut self, frame: &Frame, p: &Params) -> Option<Reading> {
        let (w, h) = frame_dims(frame)?;

        if self.size != Some((frame.width, frame.height)) || self.trigger != Some(p.trigger) {
            self.reset();
        }
        self.size = Some((frame.width, frame.height));
        self.trigger = Some(p.trigger);

        let step = sample_step(w, h);
        let samples = (w.div_ceil(step) * h.div_ceil(step)) as f64;
        let area = p.area_ratio;
        let data = frame.bgra;

        let (value, hit) = match p.trigger {
            Trigger::Change => {
                if !self.has_base {
                    copy_into(&mut self.base, data);
                    self.has_base = true;
                    return Some(Reading {
                        value: 0.0,
                        fired: false,
                    });
                }
                let value = if self.skip_identical && self.base.as_slice() == data {
                    0.0
                } else {
                    count_changed(&self.base, data, w, step, p.tolerance) as f64 / samples
                };
                (value, value >= area)
            }
            Trigger::Still => {
                if !self.has_prev {
                    copy_into(&mut self.prev, data);
                    self.has_prev = true;
                    return Some(Reading {
                        value: 0.0,
                        fired: false,
                    });
                }
                let value = if self.skip_identical && self.prev.as_slice() == data {
                    0.0
                } else {
                    let v = count_changed(&self.prev, data, w, step, p.tolerance) as f64 / samples;
                    copy_into(&mut self.prev, data);
                    v
                };
                if value >= area {
                    self.primed = true;
                }
                (value, self.primed && value < area)
            }
            Trigger::ColorIn | Trigger::ColorOut => {
                let reuse = match self.color_cache {
                    Some(c)
                        if self.skip_identical
                            && self.has_prev
                            && c.color == p.color
                            && c.tolerance == p.tolerance
                            && self.prev.as_slice() == data =>
                    {
                        Some(c.value)
                    }
                    _ => None,
                };
                let value = match reuse {
                    Some(v) => v,
                    None => {
                        let v = count_near(data, w, step, p.color, p.tolerance) as f64 / samples;
                        if self.skip_identical {
                            copy_into(&mut self.prev, data);
                            self.has_prev = true;
                        }
                        self.color_cache = Some(ColorCache {
                            color: p.color,
                            tolerance: p.tolerance,
                            value: v,
                        });
                        v
                    }
                };
                if p.trigger == Trigger::ColorIn {
                    (value, value >= area)
                } else {
                    if value >= area {
                        self.primed = true;
                    }
                    (value, self.primed && value < area)
                }
            }
        };

        self.streak = if hit {
            self.streak.saturating_add(1)
        } else {
            0
        };
        Some(Reading {
            value,
            fired: self.streak >= required_streak(p.hold_ms, p.interval_ms),
        })
    }
}

/// フレームが正しければ (幅, 高さ) を usize で返す。
fn frame_dims(frame: &Frame) -> Option<(usize, usize)> {
    let w = usize::try_from(frame.width).ok()?;
    let h = usize::try_from(frame.height).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let len = w.checked_mul(h)?.checked_mul(4)?;
    (len == frame.bgra.len()).then_some((w, h))
}

/// 間引きのステップ。標本数 `ceil(w/s) * ceil(h/s)` が [`MAX_SAMPLES`] 以下になる最小の s。
fn sample_step(w: usize, h: usize) -> usize {
    let count = |s: usize| (w.div_ceil(s) as u64) * (h.div_ceil(s) as u64);
    let mut s = 1;
    while count(s) > MAX_SAMPLES {
        s += 1;
    }
    s
}

/// 確保済みの領域を使い回してコピーする。
fn copy_into(dst: &mut Vec<u8>, src: &[u8]) {
    dst.clear();
    dst.extend_from_slice(src);
}

/// 標本にする行を順に返す。各行は `w * 4` バイト。
fn sampled_rows(data: &[u8], w: usize, step: usize) -> impl Iterator<Item = &[u8]> {
    data.chunks_exact(w * 4).step_by(step)
}

/// 2枚で「違う画素」(B・G・R のいずれかの差が tol を超える)の標本数。
fn count_changed(a: &[u8], b: &[u8], w: usize, step: usize, tol: u8) -> u64 {
    let mut n = 0u64;
    for (ra, rb) in sampled_rows(a, w, step).zip(sampled_rows(b, w, step)) {
        for (pa, pb) in ra.chunks_exact(4).zip(rb.chunks_exact(4)).step_by(step) {
            let differs = pa[0].abs_diff(pb[0]) > tol
                || pa[1].abs_diff(pb[1]) > tol
                || pa[2].abs_diff(pb[2]) > tol;
            n += u64::from(differs);
        }
    }
    n
}

/// 指定色(R, G, B)に近い画素(全成分の差が tol 以内)の標本数。
fn count_near(data: &[u8], w: usize, step: usize, rgb: [u8; 3], tol: u8) -> u64 {
    let [r, g, b] = rgb;
    let mut n = 0u64;
    for row in sampled_rows(data, w, step) {
        for px in row.chunks_exact(4).step_by(step) {
            let near =
                px[0].abs_diff(b) <= tol && px[1].abs_diff(g) <= tol && px[2].abs_diff(r) <= tol;
            n += u64::from(near);
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 3] = [255, 0, 0];
    const GRAY: [u8; 3] = [100, 100, 100];
    const BLACK: [u8; 3] = [0, 0, 0];

    /// 単色(R, G, B)で塗った BGRX 画像を作る。X は 0。
    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
        }
        v
    }

    /// 左端の `percent`% の列を別色で塗る。
    fn paint_left(buf: &mut [u8], w: u32, percent: u32, rgb: [u8; 3]) {
        let cols = (w * percent / 100) as usize;
        for row in buf.chunks_exact_mut(w as usize * 4) {
            for px in row.chunks_exact_mut(4).take(cols) {
                px[..3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
            }
        }
    }

    /// 単色の上に左端 `percent`% を別色で塗った画像。
    fn left(w: u32, h: u32, bg: [u8; 3], percent: u32, fg: [u8; 3]) -> Vec<u8> {
        let mut v = solid(w, h, bg);
        paint_left(&mut v, w, percent, fg);
        v
    }

    fn frame(w: u32, h: u32, data: &[u8]) -> Frame<'_> {
        Frame {
            width: w,
            height: h,
            bgra: data,
        }
    }

    /// hold 0 なら1コマで発火する設定。
    fn params(trigger: Trigger, area: f64) -> Params {
        Params {
            trigger,
            tolerance: 10,
            area_ratio: area,
            hold_ms: 0,
            interval_ms: 500,
            color: RED,
        }
    }

    fn feed(d: &mut Detector, data: &[u8], p: &Params) -> Reading {
        d.feed(&frame(100, 100, data), p).expect("正しいフレーム")
    }

    /// 設計書11章(必須1): 1%の変化では検知しない(area 3%)。
    #[test]
    fn one_percent_change_does_not_fire() {
        let p = params(Trigger::Change, 0.03);
        let mut d = Detector::new();
        let base = solid(100, 100, GRAY);
        let moved = left(100, 100, GRAY, 1, RED);
        assert_eq!(
            feed(&mut d, &base, &p),
            Reading {
                value: 0.0,
                fired: false
            }
        );
        for _ in 0..5 {
            let r = feed(&mut d, &moved, &p);
            assert!((r.value - 0.01).abs() < 1e-12);
            assert!(!r.fired);
        }
    }

    /// 設計書11章(必須2): 9%の変化が2回続くと検知する(area 3%、hold 1000ms、interval 500ms)。
    #[test]
    fn nine_percent_change_twice_fires() {
        let p = Params {
            hold_ms: 1000,
            interval_ms: 500,
            ..params(Trigger::Change, 0.03)
        };
        let mut d = Detector::new();
        let moved = left(100, 100, GRAY, 9, RED);
        assert!(!feed(&mut d, &solid(100, 100, GRAY), &p).fired);
        let r1 = feed(&mut d, &moved, &p);
        assert!((r1.value - 0.09).abs() < 1e-12);
        assert!(!r1.fired);
        assert!(feed(&mut d, &moved, &p).fired);
        // 条件が続く限り発火したまま。
        assert!(feed(&mut d, &moved, &p).fired);
    }

    /// 設計書11章(必須3): 許容差以下の色ずれは 0% になる。差==tolerance で 0、tolerance+1 で 100%。
    #[test]
    fn shift_within_tolerance_is_zero() {
        for trigger in [Trigger::Change, Trigger::Still] {
            for ch in 0..3 {
                let p = params(trigger, 0.5);
                let mut d = Detector::new();
                feed(&mut d, &solid(100, 100, GRAY), &p);
                let mut c = GRAY;
                c[ch] += 10;
                assert_eq!(feed(&mut d, &solid(100, 100, c), &p).value, 0.0);

                let mut d = Detector::new();
                feed(&mut d, &solid(100, 100, GRAY), &p);
                c[ch] += 1;
                assert_eq!(feed(&mut d, &solid(100, 100, c), &p).value, 1.0);
            }
        }
        // 色モードも同じ境界(差==tolerance は「近い」)。
        let p = Params {
            color: GRAY,
            ..params(Trigger::ColorIn, 0.5)
        };
        let mut d = Detector::new();
        assert_eq!(
            feed(&mut d, &solid(100, 100, [110, 90, 110]), &p).value,
            1.0
        );
        assert_eq!(
            feed(&mut d, &solid(100, 100, [111, 100, 100]), &p).value,
            0.0
        );
        assert_eq!(
            feed(&mut d, &solid(100, 100, [100, 89, 100]), &p).value,
            0.0
        );
    }

    /// 設計書11章(必須4): Still は最初から静止している画面では検知しない。
    #[test]
    fn still_does_not_fire_on_screen_still_from_start() {
        let p = params(Trigger::Still, 0.03);
        let mut d = Detector::new();
        let img = solid(100, 100, GRAY);
        for _ in 0..10 {
            let r = feed(&mut d, &img, &p);
            assert_eq!(r.value, 0.0);
            assert!(!r.fired);
        }
    }

    /// 設計書11章(必須5): ColorOut は一度現れるまで検知しない。
    #[test]
    fn color_out_does_not_fire_until_color_appears() {
        let p = params(Trigger::ColorOut, 0.05);
        let mut d = Detector::new();
        let absent = solid(100, 100, GRAY);
        let present = left(100, 100, GRAY, 10, RED);
        for _ in 0..3 {
            assert!(!feed(&mut d, &absent, &p).fired);
        }
        let r = feed(&mut d, &present, &p);
        assert!((r.value - 0.10).abs() < 1e-12);
        assert!(!r.fired);
        let r = feed(&mut d, &absent, &p);
        assert_eq!(r.value, 0.0);
        assert!(r.fired);
    }

    /// 発火に必要な連続数(整数演算、interval の下限 50ms)。
    #[test]
    fn required_streak_table() {
        assert_eq!(required_streak(1000, 500), 2);
        assert_eq!(required_streak(900, 300), 3);
        assert_eq!(required_streak(0, 500), 1);
        assert_eq!(required_streak(600_000, 200), 3000);
        assert_eq!(required_streak(1000, 0), 20);
        assert_eq!(required_streak(1000, 10), 20);
        assert_eq!(required_streak(1000, 49), 20);
        assert_eq!(required_streak(1000, 50), 20);
        assert_eq!(required_streak(1001, 500), 3);
        assert_eq!(required_streak(1, 500), 1);
        assert_eq!(required_streak(u32::MAX, 0), u32::MAX.div_ceil(50));
    }

    /// 途中で条件が外れたら連続数を数え直す。
    #[test]
    fn streak_restarts_when_condition_breaks() {
        let p = Params {
            hold_ms: 1500,
            interval_ms: 500,
            ..params(Trigger::Change, 0.03)
        };
        let mut d = Detector::new();
        let base = solid(100, 100, GRAY);
        let moved = left(100, 100, GRAY, 9, RED);
        feed(&mut d, &base, &p);
        assert!(!feed(&mut d, &moved, &p).fired);
        assert!(!feed(&mut d, &moved, &p).fired);
        assert!(!feed(&mut d, &base, &p).fired); // 外れた
        assert!(!feed(&mut d, &moved, &p).fired);
        assert!(!feed(&mut d, &moved, &p).fired);
        assert!(feed(&mut d, &moved, &p).fired);
        assert!(!feed(&mut d, &base, &p).fired); // 外れたら止まる
    }

    /// 区画の大きさが変わったら基準を取り直す(最初のコマ扱い)。
    #[test]
    fn size_change_rebases() {
        let p = params(Trigger::Change, 0.03);
        let mut d = Detector::new();
        let a = solid(100, 100, GRAY);
        feed(&mut d, &a, &p);
        assert!(feed(&mut d, &solid(100, 100, RED), &p).fired);

        let small = solid(50, 40, RED);
        let r = d.feed(&frame(50, 40, &small), &p).unwrap();
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
        // 新しい大きさのコマが基準になっている。
        let r = d.feed(&frame(50, 40, &small), &p).unwrap();
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
        let other = solid(50, 40, GRAY);
        assert!(d.feed(&frame(50, 40, &other), &p).unwrap().fired);
    }

    /// Still は「動いたあと止まり続けたら」発火する。
    #[test]
    fn still_fires_after_motion_then_stillness() {
        let p = Params {
            hold_ms: 1000,
            interval_ms: 500,
            ..params(Trigger::Still, 0.03)
        };
        let mut d = Detector::new();
        let a = solid(100, 100, GRAY);
        let b = left(100, 100, GRAY, 20, RED);
        assert!(!feed(&mut d, &a, &p).fired); // 最初のコマ
        let r = feed(&mut d, &b, &p); // 動いた
        assert!((r.value - 0.20).abs() < 1e-12);
        assert!(!r.fired);
        assert!(!feed(&mut d, &b, &p).fired); // 止まった1コマ目
        assert!(feed(&mut d, &b, &p).fired); // 止まった2コマ目で発火
        assert!(feed(&mut d, &b, &p).fired);
        assert!(!feed(&mut d, &a, &p).fired); // また動いた
        assert!(!feed(&mut d, &a, &p).fired);
        assert!(feed(&mut d, &a, &p).fired);
    }

    /// ColorIn の基本: 指定色の面積が area 以上で発火し、減ったら止まる。
    #[test]
    fn color_in_basic() {
        let p = params(Trigger::ColorIn, 0.05);
        let mut d = Detector::new();
        let r = feed(&mut d, &solid(100, 100, GRAY), &p);
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
        let r = feed(&mut d, &left(100, 100, GRAY, 4, RED), &p);
        assert!((r.value - 0.04).abs() < 1e-12);
        assert!(!r.fired);
        let r = feed(&mut d, &left(100, 100, GRAY, 5, RED), &p);
        assert!((r.value - 0.05).abs() < 1e-12);
        assert!(r.fired);
        // 最初のコマでも色モードは判定する。
        let mut d = Detector::new();
        assert!(feed(&mut d, &solid(100, 100, RED), &p).fired);
        assert!(!feed(&mut d, &solid(100, 100, BLACK), &p).fired);
    }

    /// 間引きのステップ: 標本数が 100万以下になる最小の整数。
    #[test]
    fn sample_step_values() {
        assert_eq!(sample_step(1000, 1000), 1);
        assert_eq!(sample_step(1001, 1000), 2);
        assert_eq!(sample_step(2000, 1000), 2);
        assert_eq!(sample_step(2560, 1440), 2);
        assert_eq!(sample_step(3840, 2160), 3);
        assert_eq!(sample_step(1, 5_000_000), 5);
    }

    /// 間引き(2000x1000)でも割合は概ね正しい。
    #[test]
    fn subsampled_ratio_is_about_right() {
        let (w, h) = (2000, 1000);
        let base = solid(w, h, GRAY);
        let mut d = Detector::new();
        let p = params(Trigger::Change, 0.5);
        d.feed(&frame(w, h, &base), &p).unwrap();
        for percent in [1, 30, 77] {
            let img = left(w, h, GRAY, percent, RED);
            let r = d.feed(&frame(w, h, &img), &p).unwrap();
            assert!(
                (r.value - f64::from(percent) / 100.0).abs() < 0.005,
                "{percent}%: {r:?}"
            );
        }
        let p = params(Trigger::ColorIn, 0.5);
        let img = left(w, h, GRAY, 30, RED);
        let r = Detector::new().feed(&frame(w, h, &img), &p).unwrap();
        assert!((r.value - 0.30).abs() < 0.005);
    }

    /// 早期終了ありとなしで、どのモードでも結果が一致する(色・許容差を途中で変えても)。
    #[test]
    fn early_exit_matches_full_computation() {
        let gray = solid(100, 100, GRAY);
        let red10 = left(100, 100, GRAY, 10, RED);
        let red40 = left(100, 100, GRAY, 40, RED);
        let near = left(100, 100, GRAY, 40, [250, 5, 5]);
        let seq: [&[u8]; 12] = [
            &gray, &gray, &red10, &red10, &red10, &gray, &red40, &red40, &near, &near, &gray, &gray,
        ];
        for trigger in [
            Trigger::Change,
            Trigger::Still,
            Trigger::ColorIn,
            Trigger::ColorOut,
        ] {
            let mut fast = Detector::new();
            let mut slow = Detector::new();
            slow.skip_identical = false;
            for (i, img) in seq.iter().enumerate() {
                let p = Params {
                    hold_ms: 1000,
                    // 同じコマのまま色・許容差を変えても、使い回しで古い値を返さないこと。
                    tolerance: if i >= 9 { 2 } else { 10 },
                    color: if i == 3 { GRAY } else { RED },
                    ..params(trigger, 0.3)
                };
                let a = feed(&mut fast, img, &p);
                let b = feed(&mut slow, img, &p);
                assert_eq!(a.value.to_bits(), b.value.to_bits(), "{trigger:?} #{i}");
                assert_eq!(a.fired, b.fired, "{trigger:?} #{i}");
            }
        }
    }

    /// 4バイト目(X)が違っても差とみなさない。
    #[test]
    fn fourth_byte_is_ignored() {
        let a = solid(100, 100, GRAY);
        let mut b = a.clone();
        for px in b.chunks_exact_mut(4) {
            px[3] = 0xff;
        }
        for skip in [true, false] {
            for trigger in [Trigger::Change, Trigger::Still] {
                let p = params(trigger, 0.01);
                let mut d = Detector::new();
                d.skip_identical = skip;
                feed(&mut d, &a, &p);
                let r = feed(&mut d, &b, &p);
                assert_eq!(r.value, 0.0);
                assert!(!r.fired);
            }
            let p = Params {
                color: GRAY,
                ..params(Trigger::ColorIn, 0.5)
            };
            let mut d = Detector::new();
            d.skip_identical = skip;
            assert_eq!(feed(&mut d, &a, &p).value, 1.0);
            assert_eq!(feed(&mut d, &b, &p).value, 1.0);
        }
    }

    /// 長さ不正のフレームはパニックせず None を返し、状態も変えない。
    #[test]
    fn invalid_frame_returns_none_without_panic() {
        let p = params(Trigger::Change, 0.03);
        let mut d = Detector::new();
        let base = solid(100, 100, GRAY);
        feed(&mut d, &base, &p);

        let short = vec![0u8; 100 * 100 * 4 - 1];
        let long = vec![0u8; 100 * 100 * 4 + 4];
        assert_eq!(d.feed(&frame(100, 100, &short), &p), None);
        assert_eq!(d.feed(&frame(100, 100, &long), &p), None);
        assert_eq!(d.feed(&frame(0, 100, &[]), &p), None);
        assert_eq!(d.feed(&frame(100, 0, &[]), &p), None);
        assert_eq!(d.feed(&frame(u32::MAX, u32::MAX, &[]), &p), None);
        assert_eq!(d.feed(&frame(1, 1, &[0, 0, 0]), &p), None);

        // 基準は残っている。
        let r = feed(&mut d, &left(100, 100, GRAY, 9, RED), &p);
        assert!((r.value - 0.09).abs() < 1e-12);
        assert!(r.fired);
    }

    /// reset() で基準・直前・連続数・primed を捨てる。
    #[test]
    fn reset_discards_state() {
        let p = params(Trigger::ColorOut, 0.05);
        let mut d = Detector::new();
        feed(&mut d, &solid(100, 100, RED), &p); // primed
        d.reset();
        assert!(!feed(&mut d, &solid(100, 100, GRAY), &p).fired);

        let p = params(Trigger::Change, 0.03);
        let mut d = Detector::new();
        feed(&mut d, &solid(100, 100, GRAY), &p);
        d.reset();
        let r = feed(&mut d, &solid(100, 100, RED), &p);
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
    }

    /// トリガーを途中で切り替えても、パニックや誤った発火をしない(最初のコマ扱い)。
    #[test]
    fn switching_trigger_without_reset_is_safe() {
        let gray = solid(100, 100, GRAY);
        let red = solid(100, 100, RED);
        let mut d = Detector::new();
        // Still で primed にしてから ColorOut へ。色はまだ一度も現れていないので発火しない。
        feed(&mut d, &gray, &params(Trigger::Still, 0.03));
        feed(&mut d, &red, &params(Trigger::Still, 0.03));
        let r = feed(&mut d, &gray, &params(Trigger::ColorOut, 0.05));
        assert!(!r.fired);
        // ColorIn から Change へ: Change の最初のコマになる。
        feed(&mut d, &red, &params(Trigger::ColorIn, 0.05));
        let r = feed(&mut d, &gray, &params(Trigger::Change, 0.03));
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
        // Change から Still へ: Still の最初のコマになる。
        let r = feed(&mut d, &red, &params(Trigger::Still, 0.03));
        assert_eq!(
            r,
            Reading {
                value: 0.0,
                fired: false
            }
        );
        // Still(primed 前)から ColorIn へ、色が見えていれば普通に発火する。
        assert!(feed(&mut d, &red, &params(Trigger::ColorIn, 0.05)).fired);
    }
}
