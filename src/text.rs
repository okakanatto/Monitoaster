//! 画面に出す文言。文言はこのモジュールに集約する(spec.md 6章、docs/ux.md 11章)。
//!
//! 初期値は docs/ux.md の11章。体言止めか「〜できません」「〜してください」の短い文にし、
//! 句点は付けない。
//!
//! OS に依存しない。

#![forbid(unsafe_code)]

/// アプリの名前。メインウィンドウのタイトルに使う。
pub const APP_NAME: &str = "Monitoaster";
