# Monitoaster

画面の一区画を見張り、条件を満たしたら演出・音・通知で知らせるWindows用の常駐ツール。Rust + `windows` クレート。単一exe(Monitoaster.exe)、設定は exe と同じフォルダーの monitoaster.json。

## 文書

| ファイル | 内容 |
|---|---|
| `docs/spec.md` | 要件と設計。要件ID(F / E / S)の出どころ。変更したら14章の改訂履歴に残す |
| `docs/ux.md` | ユースケース、画面、状態、文言、時間の決まり。UIの実装はここを基準にする |
| `docs/m0.md` | 現在の段階(M0)の作業手順と完了条件 |

優先順位は「安全 > 軽量 > 遊び心」。衝突したら上位を取る。

## 守ること

CIが検査する。破る必要が出たら、実装する前に利用者に相談し、spec.md を改訂してから行う。

- **依存は3つだけ**(S-12): `windows`、`serde`、`serde_json`。build-dependencies も dev-dependencies も足さない。`windows` の features は使う分だけ足す。
- **unsafe は `src/platform/` 以下だけ**(S-11)。それ以外のファイルは先頭に `#![forbid(unsafe_code)]` を置く。`#[allow(unsafe_code)]` を書けるのは platform だけ。
- **通信しない**(S-01): `std::net` と、ws2_32 / winhttp / wininet / urlmon などを呼ぶAPIを使わない。
- **他のプロセスに触れない、起動しない**(S-04、S-05): `std::process::Command`、SendInput、SetWindowsHookEx、OpenProcess、CreateProcess、ShellExecute など。ホットキーは RegisterHotKey だけ。
- **ファイルに書くのは `src/config.rs` だけ**(S-03、S-06)。画面の内容をファイル・クリップボード・ログに出さない。
- **ログを出さない**(S-15)。`println!` / `eprintln!` / `dbg!` を製品コードに残さない。
- **モーダルダイアログを出さない**。エラーと状態は状態欄の1行で伝える。
- **文言は1つのモジュールに集約する**。初期値は ux.md の11章。
- 設定ファイルと利用者が選んだWAVは信頼しない(S-07、S-08)。

## 書き方

- コメント、doc、コミットメッセージ、画面の文言は日本語。識別子とテスト名は英語。
- コミットとテストのdocに、関係する要件IDを書く(例: `S-07: 64KBを超える設定は既定値に落とす`)。
- 判定・設定・演出の計算など、OSに依存しないロジックを厚くし、単体テストを付ける。`platform` は薄いラッパーに保つ。
- 時間はミリ秒の整数で持つ(spec.md 4章)。座標は物理ピクセル。

## クレートの構成

- ライブラリ `monitoaster`(`src/lib.rs`、全モジュール)と、実行ファイル `Monitoaster`(`src/main.rs`、入口だけ)。
- Windowsに依存するモジュールは `lib.rs` で `#[cfg(windows)]` を付けて宣言する。`windows` への依存も cfg(windows)。これにより Linux 上で `cargo test` が通る。
- ビルド硬化は `.cargo/config.toml`(CRT静的リンク、CFG)と `build.rs`(マニフェスト埋め込み、DEPENDENTLOADFLAG)にある。MSVCターゲット専用。CIで環境変数 `RUSTFLAGS` を設定すると `.cargo/config.toml` が無視されるので使わない。

## コマンド

```bash
cargo test                                             # OS非依存モジュールの単体テスト(Linux)
cargo clippy --all-targets -- -D warnings
cargo clippy --target x86_64-pc-windows-gnu -- -D warnings   # Windows向けコードの型検査
cargo fmt --check
```

クラウド環境(Linux)では画面まわりを動かせない。配布用exeは GitHub Actions の Windows ランナー(MSVC)がビルドし、実機確認はその成果物で利用者が行う。

exeの検査スクリプトを手元で試すときは、mingw のリンカーを入れて windows-gnu 向けにリンクする。

```bash
apt-get update && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```

`llvm-readobj` は `$(rustc --print sysroot)/lib/rustlib/<ホスト>/bin/` にある(rust-toolchain.toml の llvm-tools)。

## Git

- 作業ブランチに push する。プルリクエストは頼まれたときだけ作る。
- `Cargo.lock` はコミットする。
