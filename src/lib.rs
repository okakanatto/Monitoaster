//! Monitoaster - 画面の一区画を見張り、条件を満たしたら演出・音・通知で知らせる。
//!
//! モジュール構成と unsafe の規則は docs/spec.md の9章を参照。
//! OS非依存のモジュール(detect / config / effects)は Linux 上でも単体テストできる。
//! Windows に依存するモジュールは `#[cfg(windows)]` を付けて宣言する。

pub mod detect;
