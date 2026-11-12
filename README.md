# Monitoaster

画面の一区画を見張り、条件を満たしたら画面の演出・音・通知で知らせる Windows 用の常駐ツールです。
ジョブの「実行中→完了」のような小さな表示の変化に、別の作業をしていても気付けるようにします。
単一の exe(Monitoaster.exe)で動き、インストールと管理者権限は要りません。対象は Windows 10 22H2 / 11(x64)です。

**開発中です。** いまは M0(土台と技術検証)の段階で、使える版はまだありません。

## 安全の方針

次の4点を、仕様と CI の自動検査の両方で守ります。詳細と検査の方法は [docs/spec.md の7章](docs/spec.md#7-セキュリティ要件)にあります。

- **通信しない**: ネットワークを使う機能を持たず、ネットワーク API も呼びません。更新確認や利用状況の送信もしません。
- **残さない**: 取り込んだ画面はメモリ上だけで扱い、ファイル・クリップボード・ログに書きません。書き込むのは exe と同じフォルダーの設定ファイル monitoaster.json だけです。
- **触らない**: 画面は読むだけです。入力の送出、キーボード・マウスのフック、他のプロセスへのアクセスをしません。
- **実行しない**: 外部のプログラムを起動しません。

## ビルドとテスト

Rust は [rustup](https://rustup.rs/) で入れてください。版は `rust-toolchain.toml` で 1.97.0 に固定してあり、rustup が自動で入れます。

```bash
cargo test                                                   # OSに依存しない部分の単体テスト(Linux でも動く)
cargo clippy --all-targets -- -D warnings
cargo clippy --target x86_64-pc-windows-gnu -- -D warnings   # Windows 向けコードの型検査
cargo fmt --check
cargo build --release                                        # Windows(MSVC)で target/release/Monitoaster.exe を作る
```

配布用の exe は Windows の MSVC ツールチェーン(Visual Studio Build Tools の C++)でビルドします。GitHub Actions の Windows ランナーが push ごとにビルドし、Monitoaster.exe と SHA256SUMS.txt を成果物に置きます。

安全の方針の検査は、CI(`.github/workflows/ci.yml`)と同じものを手元でも流せます。

```bash
bash tools/check_source.sh                      # ソースの検査(禁止APIなど)
bash tools/check_exe.sh <exeのパス> <msvc|gnu>   # exe のインポート表、PE ヘッダー、マニフェストの検査
cargo deny --locked check                       # 依存クレートの許可リスト、ライセンス、取得元、脆弱性(deny.toml)
cargo audit                                     # 脆弱性
```

cargo-deny と cargo-audit は CI と同じ版を入れてください(`cargo install cargo-deny --locked --version 0.20.2`、`cargo install cargo-audit --locked --version 0.23.0`)。

## 文書

| ファイル | 内容 |
|---|---|
| [docs/spec.md](docs/spec.md) | 要件と設計。要件ID(F / E / S)の出どころ |
| [docs/ux.md](docs/ux.md) | ユースケース、画面、状態、文言、時間の決まり |

## 使う前に(M3 で書きます)

### SmartScreen の警告と SHA-256 の確認

### Windows 11 でトレイアイコンをタスクバーに出す

## ライセンス

[MIT](LICENSE)
