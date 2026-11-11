//! リンカー設定。S-12 を守るため、マニフェストはクレートを使わずリンカー引数で埋め込む。
//!
//! - S-02: asInvoker / uiAccess=false のマニフェストを埋め込む。
//! - S-09: /DEPENDENTLOADFLAG:0x800 で、静的インポートしたDLLの検索先を System32 に限定する。
//!   起動後に呼ぶ SetDefaultDllDirectories は、起動前に読み込まれるDLLには効かないため。
//!
//! MSVC のリンカー専用。windows-gnu(Linux上の cargo check 用)では何もしない。

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/Monitoaster.exe.manifest");

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let abi = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" || abi != "msvc" {
        return;
    }
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    for arg in [
        "/MANIFEST:EMBED".to_string(),
        format!("/MANIFESTINPUT:{dir}\\assets\\Monitoaster.exe.manifest"),
        "/MANIFESTUAC:NO".to_string(), // UAC の指定は自前のマニフェストに書く
        "/DEPENDENTLOADFLAG:0x800".to_string(), // LOAD_LIBRARY_SEARCH_SYSTEM32
    ] {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
