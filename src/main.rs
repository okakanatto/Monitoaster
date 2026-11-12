//! 入口。処理はライブラリ `monitoaster` の app にある。
//!
//! Windows 以外では何もしない(Linux 上で cargo test と clippy を通すため)。

// exe名を Monitoaster.exe にするため、バイナリ名だけ大文字で始める。
#![allow(non_snake_case)]
#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    monitoaster::app::run();
}
