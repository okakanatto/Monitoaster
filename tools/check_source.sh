#!/usr/bin/env bash
# ソースの検査(docs/m0.md 作業3-a)。リポジトリのルートで実行する。
#
#   tools/check_source.sh
#
# 検査ごとに「OK / NG 要件ID 内容」を1行で出し、NG なら原因の行(ファイル:行)を続けて出す。
# 1つでも NG なら終了コード 1。リポジトリのルートでないなど、検査を始められないときは 2。
#
# 位置付け: 主な守りはコンパイラの lint(Cargo.toml の unsafe_code = "deny" と、各ファイルの
# #![forbid(unsafe_code)])と、exe のインポート表の検査(tools/check_exe.sh)。このスクリプトは
# 二重の網で、lint では拾えないもの(std::fs の書き込み、println! など)も受け持つ。
#
# 対象: src/ 以下と build.rs(Cargo.toml の build = で別名なら、そのファイル)、
#       あれば tests/ benches/ examples/ の .rs。
#
# 調べ方:
# - 各ファイルを字句の単位で読み、コメント(//、///、//!、/* */、/** */、入れ子も)を除いてから調べる。
#   文字列リテラル(普通の文字列、b"" c""、生文字列 r#"..."#)と文字リテラル('"' や '\'' も)を
#   読み分けるので、doc コメントの「unsafe は platform だけ」「SendInput は使わない」で誤検知せず、
#   文字列の中の "//" や "/*" のせいで後ろのコードを読み落とすこともない。
# - 文字列リテラルの中身の扱い(どちらを選んだか):
#   * 構文を調べる検査(unsafe、属性、use、パス、マクロ呼び出し)は、文字列の中身を消してから調べる。
#     文字列の中の語は効力を持たないので、消しても見逃しにはならず、誤検知だけが減る。
#   * 名前を調べる検査(S-01 の DLL 名、S-04・S-05 の禁止API、S-03・S-06 の Win32 書き込みAPI、
#     OutputDebugString)は、文字列の中身を残して調べる。GetProcAddress(h, s!("SendInput")) や
#     LoadLibraryW(w!("ws2_32.dll")) のように、名前を文字列で渡して動的に呼ぶ抜け道を塞ぐため。
#     画面の文言(日本語、text.rs に集約)にこれらの英字の名前が入ることはまずなく、誤検知の心配は小さい。
# - 複数行にまたがる use と属性は、1つにつなぎ、識別子どうしの間以外の空白を詰めてから調べる
#   (rustfmt は use std::{...} を複数行に折る)。
# - 語の区切りは [A-Za-z0-9_] で判定する。LC_ALL=C にして、ロケールで文字の扱いが変わらないようにする
#   (Windows の Git Bash でも同じ結果になるように。grep -P、\b、\w は使わない)。
# - 行末の CR は除いてから調べる(CRLF で取り出されたソースでも同じ結果になる)。
#
# 既知の限界(塞げないもの。コンパイラの lint、exe のインポート表の検査、レビューが受け持つ):
# - 名前を文字列の連結で組み立てる動的呼び出し(concat!("Send", "Input") を GetProcAddress に渡す等)。
#   GetProcAddress を呼ぶには unsafe が要るので、書ける場所は src/platform/ に限られる。
# - macro_rules! の本体は rustfmt が整形しないので、本体の中でパスを行の途中で折る
#   (std::<改行>net::TcpStream)と見逃す。use 以外のパスは1行の中で調べるため。
#   std::net と std::process::Command は exe の検査(ws2_32、CreateProcessW のインポート)でも拾える。
# - マクロにパスの部品を別々に渡して組み立てる書き方(macro_rules! m { ($a:ident, $b:ident) =>
#   { $a::$b::TcpStream::connect(..) } } を m!(std, net) で呼ぶ等)。exe の検査で拾える。
#   use の別名(use std::process as p)、glob(use std::fs::*)、std 自体の別名は拾う。
# - 自前の識別子が禁止API名で始まると誤検知する(例: mouse_event_count)。名前を変えて避ける。
#   自前の型に Command::new があっても S-05 で誤検知する。
# - 依存クレート(windows、serde、serde_json)の中身は調べない。S-12(cargo-deny)が受け持つ。
# - doc コメントの中のコード例(doctest)は調べない。製品の exe には入らない。
# - 環境変数 RUSTFLAGS などで lint を緩められる(.cargo/config.toml も無視される)。CI で使わない(CLAUDE.md)。

# shellcheck disable=SC2016 # awk のプログラムは単一引用符で書く(シェルに展開させない)
set -euo pipefail
export LC_ALL=C

# ---- 一覧(足すときは理由をコメントに書く) -------------------------------------------

# S-04、S-05: 禁止API名。識別子の前方一致で調べる(CreateProcessW、ShellExecuteExW なども当たる)。
# tools/check_exe.sh の BANNED_API と同じ一覧に保つ。
BANNED_API=(
  SendInput keybd_event mouse_event SetWindowsHookEx
  OpenProcess WriteProcessMemory ReadProcessMemory CreateRemoteThread
  CreateProcess ShellExecute WinExec
)

# S-03、S-06: Win32 のファイル・レジストリ・クリップボードへの書き込みAPI。どこでも禁止(前方一致)。
# 設定の保存は src/config.rs が std::fs で行うので、Win32 のファイルAPIは要らない。
WIN32_WRITE_API=(
  CreateFile WriteFile DeleteFile MoveFile CopyFile ReplaceFile
  RegSetValue RegSetKeyValue RegCreateKey SHSetValue
  OpenClipboard SetClipboardData
  WritePrivateProfile # INI ファイルへの書き込み
  SHFileOperation IFileOperation # シェルのファイル操作(コピー、移動、削除)
)

# S-03、S-06: std::fs で書き込む関数(fs:: の後ろの前方一致)。src/config.rs の外で禁止。
# create_dir は create_dir_all、remove_dir は remove_dir_all にも当たる。
FS_WRITE_FN=(
  write rename remove_file remove_dir create_dir copy hard_link set_permissions
  soft_link symlink # std::os::windows::fs::symlink_file なども fs::symlink で当たる
)

# S-01: 通信に使うDLL(大文字小文字を区別しない前方一致)。tools/check_exe.sh の NET_DLLS と同じ。
NET_DLLS=(ws2_32 mswsock winhttp wininet urlmon iphlpapi dnsapi)

# S-01: windows クレートの通信まわりの feature(Cargo.toml)とモジュール(ソース)。
NET_FEATURES='"(Win32_Networking|Win32_NetworkManagement|Win32_Web|Win32_System_Com_Urlmon|Networking|Web)(_[A-Za-z0-9_]*)?"'
NET_MODULES='(Networking|NetworkManagement|Urlmon)'

# ---- 準備 ------------------------------------------------------------------------------

if [[ ! -f Cargo.toml || ! -d src ]]; then
  echo "check_source.sh: リポジトリのルート(Cargo.toml と src/ のある所)で実行してください" >&2
  exit 2
fi

TAB=$'\t'
B='(^|[^A-Za-z0-9_])' # 語の始まり
E='([^A-Za-z0-9_]|$)' # 語の終わり

join_alt() { # 配列を正規表現の選択 (a|b|c) にする
  local IFS='|'
  printf '(%s)' "$*"
}

# ビルドスクリプト。Cargo.toml の build = "..." があればそれ、なければ build.rs。
build_script=build.rs
custom_build=$(tr -d '\r' <Cargo.toml | sed -n 's/^[[:space:]]*build[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | sed -n '1p')
[[ -n $custom_build ]] && build_script=${custom_build#./}

files=()
roots=()
for d in src tests benches examples; do
  [[ -d $d ]] && roots+=("$d")
done
while IFS= read -r -d '' f; do
  files+=("${f#./}")
done < <(find "${roots[@]}" -type f -name '*.rs' -print0 | sort -z)
if [[ -f $build_script ]]; then
  files+=("$build_script")
fi
if [[ ${#files[@]} -eq 0 ]]; then
  echo "check_source.sh: 調べる .rs ファイルがありません" >&2
  exit 2
fi

T=$(mktemp -d "${TMPDIR:-/tmp}/check_source.XXXXXX")
trap 'rm -rf "$T"' EXIT
: >"$T/orig.tsv"
: >"$T/code.tsv"
: >"$T/str.tsv"
: >"$T/units.tsv"

# ---- 字句の読み分け ----------------------------------------------------------------------
# 出力(各行「パス<TAB>行番号<TAB>本文」。行番号は元のファイルと同じ):
#   orig.tsv  元の行(行末の CR を除き、タブを空白に)
#   code.tsv  コメントを除き、文字列リテラルを "" に、文字リテラルを ' ' にした行
#   str.tsv   コメントだけを除いた行(文字列の中身を残す)
#   units.tsv 「パス<TAB>開始行<TAB>波かっこの深さ<TAB>本文」。use 文と属性(#[...]、#![...])を
#             1つにつなぎ、識別子どうしの間以外の空白を詰めたもの(code.tsv から作る)。
LEX_AWK='
function lex(line,    n, i, j, k, h, c, c2, rest) {
  oc = ""; os = ""; n = length(line); i = 1
  while (i <= n) {
    if (st == "block") {                      # ブロックコメントの中(入れ子あり)
      rest = substr(line, i)
      j = index(rest, "*/"); k = index(rest, "/*")
      if (j == 0 && k == 0) return
      if (k > 0 && (j == 0 || k < j)) { cdepth++; i += k + 1; continue }
      cdepth--; i += j + 1
      if (cdepth == 0) st = "code"
      continue
    }
    if (st == "str") {                        # 文字列の中。\ の次の1文字は読み飛ばす
      c = substr(line, i, 1)
      if (c == "\\") { os = os substr(line, i, 2); i += 2; continue }
      if (c == "\"") { oc = oc "\""; os = os "\""; st = "code"; i++; continue }
      os = os c; i++; continue
    }
    if (st == "raw") {                        # 生文字列の中。"### で閉じる
      rest = substr(line, i); j = index(rest, rawend)
      if (j == 0) { os = os rest; return }
      os = os substr(rest, 1, j - 1 + length(rawend)); oc = oc "\""
      i += j - 1 + length(rawend); st = "code"; continue
    }
    c = substr(line, i, 1); c2 = substr(line, i, 2)
    if (c2 == "//") return
    if (c2 == "/*") { st = "block"; cdepth = 1; i += 2; continue }
    if (c == "\"") { oc = oc "\""; os = os "\""; st = "str"; i++; continue }
    if ((c == "r" || c == "b" || c == "c") && (i == 1 || substr(line, i - 1, 1) !~ /[A-Za-z0-9_]/)) {
      j = i
      if (c != "r" && substr(line, i + 1, 1) == "r") j = i + 1
      if (substr(line, j, 1) == "r") {
        k = j + 1
        while (substr(line, k, 1) == "#") k++
        if (substr(line, k, 1) == "\"") {     # r"  r#"  br#"  cr#"
          rawend = "\""
          for (h = j + 1; h < k; h++) rawend = rawend "#"
          os = os substr(line, i, k - i + 1); oc = oc "\""
          st = "raw"; i = k + 1; continue
        }
      }
    }
    if (c == "\047") {                        # 文字リテラルか、ライフタイム・ラベルか
      if (substr(line, i + 1, 1) == "\\") {   # 例: \047\\n\047 \047\\\047\047 \047\\u{1F600}\047
        k = i + 3
        while (k <= n && substr(line, k, 1) != "\047") k++
        os = os substr(line, i, k - i + 1); oc = oc "\047 \047"; i = k + 1; continue
      }
      if (substr(line, i + 2, 1) == "\047") { # 例: \047"\047 \047a\047
        os = os substr(line, i, 3); oc = oc "\047 \047"; i += 3; continue
      }
      if (substr(line, i + 1, 1) !~ /[ -~]/) { # ASCII 以外の1文字(UTF-8 で2〜4バイト)
        for (k = i + 2; k <= i + 5 && k <= n; k++) {
          if (substr(line, k, 1) == "\047") break
          if (substr(line, k, 1) ~ /[ -~]/) { k = n + 1; break }
        }
        if (k <= i + 5 && k <= n) {
          os = os substr(line, i, k - i + 1); oc = oc "\047 \047"; i = k + 1; continue
        }
      }
    }
    oc = oc c; os = os c; i++
  }
}
function squeeze(s,    out, n, i, ch, prev) {
  gsub(/ +/, " ", s); out = ""; n = length(s)
  for (i = 1; i <= n; i++) {
    ch = substr(s, i, 1)
    if (ch == " ") {
      prev = substr(out, length(out), 1)
      if (prev ~ /[A-Za-z0-9_]/ && substr(s, i + 1, 1) ~ /[A-Za-z0-9_]/) out = out " "
      continue
    }
    out = out ch
  }
  return out
}
function end_unit() {
  print path "\t" ustart "\t" udepth "\t" squeeze(ubuf) > (OUT "/units.tsv")
  ukind = ""; ubuf = ""
}
function track(s,    n, i, ch, rest) {
  n = length(s)
  for (i = 1; i <= n; i++) {
    ch = substr(s, i, 1)
    if (ch == "{") bdepth++
    else if (ch == "}") bdepth--
    if (ukind == "use") { ubuf = ubuf ch; if (ch == ";") end_unit(); continue }
    if (ukind == "attr") {
      ubuf = ubuf ch
      if (ch == "[") abr++
      else if (ch == "]") { abr--; if (abr == 0) end_unit() }
      continue
    }
    if (ch == "#") {
      rest = substr(s, i + 1)
      if (rest ~ /^ *!? *\[/) { ukind = "attr"; ustart = FNR; udepth = bdepth; ubuf = "#"; abr = 0; continue }
    }
    if (ch == "u" && substr(s, i, 3) == "use" && (i == 1 || substr(s, i - 1, 1) !~ /[A-Za-z0-9_]/) \
        && substr(s, i + 3, 1) !~ /[A-Za-z0-9_]/ && substr(s, i + 3) !~ /^ *</) {
      ukind = "use"; ustart = FNR; udepth = bdepth; ubuf = "use"; i += 2; continue
    }
  }
  if (ukind != "") ubuf = ubuf " "
}
function end_file() {
  if (ukind != "") end_unit()
}
FNR == 1 {
  if (path != "") end_file()
  path = FILENAME; sub(/^\.\//, "", path)
  st = "code"; cdepth = 0; bdepth = 0; ukind = ""; ubuf = ""
}
{
  line = $0
  sub(/\r$/, "", line)
  gsub(/\t/, " ", line)
  print path "\t" FNR "\t" line > (OUT "/orig.tsv")
  lex(line)
  print path "\t" FNR "\t" oc > (OUT "/code.tsv")
  print path "\t" FNR "\t" os > (OUT "/str.tsv")
  track(oc)
}
END { if (path != "") end_file() }
'
awk -v OUT="$T" "$LEX_AWK" "${files[@]/#/./}"

# ---- 照合の道具 --------------------------------------------------------------------------

# scan VIEW SKIP ONLY RE1 [RE2 ...]
#   VIEW(code / str / units)の各行のうち、RE1 以降のすべてに合う行の「パス<TAB>行」を出す。
#   SKIP に合うパスは除き、ONLY が空でなければ ONLY に合うパスだけを見る。
#   名前に icase: を付けた正規表現は、本文を小文字にしてから照合する(正規表現は小文字で書く)。
scan() {
  local view=$1
  local skip=$2
  local only=$3
  shift 3
  local i=0
  local -a vars=()
  local re
  for re in "$@"; do
    i=$((i + 1))
    vars+=("RE$i=$re")
  done
  env "${vars[@]}" NRE="$i" SKIP="$skip" ONLY="$only" VIEW="$view" awk -F'\t' '
    BEGIN { nre = ENVIRON["NRE"] + 0; col = (ENVIRON["VIEW"] == "units") ? 4 : 3 }
    ENVIRON["SKIP"] != "" && $1 ~ ENVIRON["SKIP"] { next }
    ENVIRON["ONLY"] != "" && $1 !~ ENVIRON["ONLY"] { next }
    {
      t = $col
      for (i = 1; i <= nre; i++) {
        re = ENVIRON["RE" i]
        if (substr(re, 1, 6) == "icase:") { if (tolower(t) !~ substr(re, 7)) next }
        else if (t !~ re) next
      }
      if (col == 4) print $1 "\t" $2 "\t" t # use と属性はつないだ形も渡す
      else print $1 "\t" $2
    }' "$T/$view.tsv"
}

total=0
ng=0

# show_hits: 「パス<TAB>行<TAB>補足」を受け取り、元の行を添えて出す。行 0 は補足だけを出す。
# 元の行がない(TOML)ときは補足を、複数行にまたがる use・属性は補足(つないだ形)を出す。
show_hits() {
  sort -t "$TAB" -k1,1 -k2,2n -u >"$T/hits"
  awk -F'\t' -v max=20 '
    phase == 1 {
      k = $1 SUBSEP $2
      if (!(k in want)) { want[k] = 1; n++; key[n] = k; p[n] = $1; l[n] = $2; note[k] = $3 }
      next
    }
    phase == 2 { k = $1 SUBSEP $2; if (k in want) { t = $3; sub(/^ +/, "", t); text[k] = t }; next }
    END {
      for (i = 1; i <= n && i <= max; i++) {
        k = key[i]
        if (l[i] == 0) printf "      %s: %s\n", p[i], note[k]
        else {
          t = (k in text) ? text[k] : note[k]
          if ((k in text) && note[k] != "") { # 複数行の use・属性は、つないだ形を出す
            a = t; b = note[k]; gsub(/ /, "", a); gsub(/ /, "", b)
            if (a != b) t = note[k]
          }
          printf "      %s:%s: %s\n", p[i], l[i], t
        }
      }
      if (n > max) printf "      ...ほか %d 件\n", n - max
    }' phase=1 "$T/hits" phase=2 "$T/orig.tsv"
}

# report 要件ID 内容 当たり(「パス<TAB>行[<TAB>補足]」の行の並び。空なら OK)
report() {
  local id=$1
  local what=$2
  local hits=$3
  total=$((total + 1))
  if [[ -z ${hits//[[:space:]]/} ]]; then
    printf 'OK  %s %s\n' "$id" "$what"
    return
  fi
  ng=$((ng + 1))
  printf 'NG  %s %s\n' "$id" "$what"
  printf '%s\n' "$hits" | awk 'NF > 0' | show_hits
}

# TOML のコメント(文字列の外の #)を除いて「パス<TAB>行<TAB>本文」で出す。
toml_lines() {
  awk '
    {
      s = $0; sub(/\r$/, "", s); out = ""; q = ""; n = length(s)
      for (i = 1; i <= n; i++) {
        c = substr(s, i, 1)
        if (q == "") {
          if (c == "#") break
          if (c == "\"" || c == "\047") q = c
        } else if (q == "\"" && c == "\\") { out = out c substr(s, i + 1, 1); i++; continue }
        else if (c == q) q = ""
        out = out c
      }
      gsub(/\t/, " ", out)
      print FILENAME "\t" FNR "\t" out
    }' "$@"
}

PLATFORM='^src/platform/'
CONFIG='^src/config\.rs$'
BIN_OR_BUILD="^(src/bin/|$(printf '%s' "$build_script" | sed 's/[][\.^$*+?(){}|/]/\\&/g')\$)"
USE='^use[^A-Za-z0-9_]'
STD="${B}std${E}"
# モジュールの glob import と別名(use std::process::*、use std::fs as f、use std::fs::{self as f})
glob_or_alias() { printf '%s' "${B}$1( as |::[*]|::[{]([^{}]*,)?([*]|self as ))"; }

echo "ソースの検査: ${#files[@]} ファイル(${roots[*]} $([[ -f $build_script ]] && printf '%s' "$build_script"))"

# ---- S-11 unsafe の隔離 ----------------------------------------------------------------

declare -A has_forbid=()
while IFS= read -r f; do
  [[ -n $f ]] && has_forbid[$f]=1
done < <(awk -F'\t' '$3 == 0 && $4 ~ /^#!\[forbid\(([^()]*,)?unsafe_code[,)]/ { print $1 }' "$T/units.tsv")
hits=""
for f in "${files[@]}"; do
  case $f in
  src/lib.rs | src/platform/*) continue ;; # lib.rs に forbid を置くと platform で allow できない
  "$build_script") continue ;;              # build.rs は Cargo.toml の [lints] の deny が効く
  esac
  [[ -n ${has_forbid[$f]:-} ]] || hits+="$f${TAB}0${TAB}先頭に #![forbid(unsafe_code)] がない"$'\n'
done
report "S-11" "src/lib.rs と src/platform/ を除くすべての .rs に #![forbid(unsafe_code)] がある" "$hits"

hits=$(scan units "$PLATFORM" "" "^#" "${B}(allow|warn|expect)[(]([^()]*,)?unsafe_code[,)]")
report "S-11" "unsafe_code を緩める属性(allow / warn / expect、cfg_attr 経由も)が src/platform/ の外にない" "$hits"

hits=$(scan code "$PLATFORM" "" "${B}unsafe${E}|${B}(global_asm|naked_asm|asm) *!")
report "S-11" "unsafe(ブロック、fn、impl、extern、#[unsafe(...)])と asm 系マクロが src/platform/ の外にない" "$hits"

hits=$(toml_lines Cargo.toml | awk -F'\t' '
  $3 ~ /^ *\[/ { sec = $3; gsub(/[ "]/, "", sec) }
  sec == "[lints.rust]" && $3 ~ /^ *unsafe_code *= *("(deny|forbid)"|\{.*level *= *"(deny|forbid)")/ { ok = 1 }
  sec == "[lints]" && $3 ~ /^ *rust\.unsafe_code *= *("(deny|forbid)"|\{.*level *= *"(deny|forbid)")/ { ok = 1 }
  END { if (!ok) print "Cargo.toml\t0\t[lints.rust] に unsafe_code = \"deny\" がない" }')
report "S-11" 'Cargo.toml の [lints.rust] に unsafe_code = "deny"(または forbid)がある' "$hits"

cargo_configs=()
for c in .cargo/config.toml .cargo/config; do
  [[ -f $c ]] && cargo_configs+=("$c")
done
hits=""
if [[ ${#cargo_configs[@]} -gt 0 ]]; then
  hits=$(toml_lines "${cargo_configs[@]}" | awk -F'\t' '$3 ~ /unsafe[-_]code|cap-lints/ { print $1 "\t" $2 "\t" $3 }')
fi
report "S-11" ".cargo/config.toml に unsafe_code を緩める指定(-A unsafe_code、--cap-lints など)がない" "$hits"

hits=$(
  scan units "" "" "^#!?[[]path="
  scan code "" "" "${B}include *!"
)
report "S-11" "検査の外のファイルをモジュールとして取り込む #[path = ...] と include! がない" "$hits"

# ---- S-01 通信しない -------------------------------------------------------------------

hits=$(
  scan code "" "" "${B}std *:: *net${E}"
  scan units "" "" "$USE" "$STD" "${B}net${E}"
)
report "S-01" "std::net を使っていない(use std::{io, net} のような書き方も)" "$hits"

hits=$(
  scan str "" "" "icase:${B}$(join_alt "${NET_DLLS[@]}")"
  scan code "" "" "${B}${NET_MODULES} *::"
  toml_lines Cargo.toml | RE="$NET_FEATURES" awk -F'\t' '$3 ~ ENVIRON["RE"] { print $1 "\t" $2 "\t" $3 }'
)
report "S-01" "通信用のDLL名(ws2_32 など)と windows クレートの通信まわりの feature・モジュールを使っていない" "$hits"

# ---- S-04、S-05 他のプロセスに触れない、起動しない ---------------------------------------

hits=$(
  scan code "" "" "${B}process *:: *Command${E}"
  scan code "" "" "${B}Command *:: *new${E}"
  scan units "" "" "$USE" "$STD" "${B}process${E}" "${B}Command${E}"
  scan units "" "" "$USE" "$STD" "$(glob_or_alias process)"
)
report "S-04 S-05" "std::process::Command を使っていない(use の書き方の違い、Command::new も)" "$hits"

hits=$(scan str "" "" "${B}$(join_alt "${BANNED_API[@]}")")
report "S-04 S-05" "禁止API名($(join_alt "${BANNED_API[@]}" | tr -d '()' | sed 's/|/、/g'))がない" "$hits"

# ---- S-03、S-06 ファイルに書くのは config.rs だけ ----------------------------------------

hits=$(
  scan code "$CONFIG" "" "${B}fs *:: *$(join_alt "${FS_WRITE_FN[@]}")"
  scan code "$CONFIG" "" "${B}File *:: *(create|options)"
  scan code "$CONFIG" "" "${B}OpenOptions"
  scan units "$CONFIG" "" "$USE" "$STD" "${B}fs${E}" "${B}($(join_alt "${FS_WRITE_FN[@]}")[A-Za-z0-9_]*|OpenOptions)${E}"
  scan units "$CONFIG" "" "$USE" "$STD" "$(glob_or_alias fs)"
)
report "S-03 S-06" "std::fs の書き込み(fs::write、File::create、OpenOptions など)が src/config.rs の外にない" "$hits"

hits=$(scan str "" "" "${B}$(join_alt "${WIN32_WRITE_API[@]}")")
report "S-03 S-06" "Win32 のファイル・レジストリ・クリップボードへの書き込みAPI(CreateFile、RegSetValue、SetClipboardData など)がない" "$hits"

# ---- std の別名と glob import(S-01、S-03、S-05 の検査を逃れられるため) --------------------

hits=$(
  scan units "" "" "$USE" "$(glob_or_alias std)"
  scan code "" "" "${B}extern +crate +std +as${E}"
)
report "S-01 S-03 S-05" "std に別名を付けたり、std を glob import したりしていない(use std as s、use std::* など)" "$hits"

# ---- S-15 ログを出さない ---------------------------------------------------------------

hits=$(
  scan code "$BIN_OR_BUILD" "" "${B}(println|print|eprintln|eprint|dbg) *!|${B}(stdout|stderr) *[(]"
  scan units "$BIN_OR_BUILD" "" "$USE" "$STD" "${B}(stdout|stderr)${E}"
  scan code "" "$BIN_OR_BUILD" "${B}(eprintln|eprint|dbg) *!|${B}stderr *[(]"
  scan units "" "$BIN_OR_BUILD" "$USE" "$STD" "${B}stderr${E}"
  scan str "" "" "${B}(OutputDebugString|WriteConsole)"
)
report "S-15" "println! / print! / eprintln! / eprint! / dbg! と OutputDebugString が src/bin/ の外にない(src/bin/ と ${build_script} は標準出力だけ可)" "$hits"

# ---- まとめ ----------------------------------------------------------------------------

echo "まとめ: ${total} 件中 OK $((total - ng)) 件、NG ${ng} 件"
if [[ $ng -gt 0 ]]; then
  exit 1
fi
