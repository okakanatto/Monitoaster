//! Monitoaster - 画面の一区画を見張り、条件を満たしたら演出・音・通知で知らせる。
//!
//! モジュール構成と unsafe の規則は docs/spec.md の9章を参照。
//! OS非依存のモジュール(detect / config / effects / text)は Linux 上でも単体テストできる。
//! Windows に依存するモジュールは `#[cfg(windows)]` を付けて宣言する。
//!
//! S-11: ここ(クレートの根)には `forbid(unsafe_code)` を置かない。置くと platform で許可
//! できなくなるため。クレート全体の禁止は Cargo.toml の lints(`unsafe_code = "deny"`)が
//! 受け持ち、platform 以外のモジュールは各ファイルの先頭で `forbid` する。

pub mod config;
pub mod detect;
pub mod effects;
pub mod text;

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod audio;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod platform;
#[cfg(windows)]
pub mod render;
#[cfg(windows)]
pub mod ui;
