#!/usr/bin/env bash
# exe の検査(docs/m0.md 作業3-b)。
#
#   tools/check_exe.sh <exe> <msvc|gnu>
#
#   msvc: CI の Windows ランナー(MSVC)でビルドした配布用の exe。すべての検査を行う。
#   gnu : Linux 上で x86_64-pc-windows-gnu 向けにリンクした exe(手元の試験用)。mingw のリンカーでは
#         入らない硬化(CFG、DependentLoadFlags、マニフェスト)と、CRT の静的リンクは調べない。
#
# 検査ごとに「OK / NG 要件ID 内容」を1行で出し、NG なら原因(該当するインポートなど)を続けて出す。
# gnu で調べない検査は「--」で出す。1つでも NG なら終了コード 1。引数の誤りや道具の不足は 2。
#
# llvm-readobj(rust-toolchain.toml の llvm-tools)の --coff-imports / --file-headers /
# --coff-load-config の出力を読む。環境変数 LLVM_READOBJ があればそれを使い、なければ
# rustc の sysroot とホストの三つ組(rustc -vV の host:)から探す。
#
# 出力の書式は llvm-readobj(LLVM 22、llvm/tools/llvm-readobj/COFFDumper.cpp)のもの:
#   Import { / DelayImport {   行頭から。その中の "Name: <DLL>"、"Symbol: <名前> (<序数>)"
#   ImageOptionalHeader { ... Characteristics [ (0x...) IMAGE_DLL_CHARACTERISTICS_* (0x..) ... ]
#   LoadConfig [ ... DependentLoadFlags: 0x800 ... ]   (printHex なので 0x と大文字の16進)
# ツールチェーンを上げて書式が変わったら、ここと下の照合を直す。
#
# Windows の Git Bash でも動く(GNU grep / sed / awk の基本機能だけ。パスは cygpath があれば変換する)。

set -euo pipefail
export LC_ALL=C

# ---- 一覧(足すときは理由をコメントに書く) -------------------------------------------

# S-09: インポートしてよいDLL(小文字。bash のパターン)。システムDLLだけにする。
ALLOWED_DLLS=(
  kernel32.dll            # 標準ライブラリ、Win32 の基本
  ntdll.dll               # 標準ライブラリ(NtCreateFile など)
  user32.dll              # ウィンドウ、メッセージ、ホットキー
  gdi32.dll               # BitBlt による取り込み
  'api-ms-win-core-*.dll' # API セット。標準ライブラリが使う(WaitOnAddress など)
  bcryptprimitives.dll    # 標準ライブラリの乱数(ProcessPrng)。System32 のシステムDLL
)
# gnu のときだけ足す。mingw の CRT。
ALLOWED_DLLS_GNU=(
  msvcrt.dll
)
# 注意: api-ms-win-crt-*.dll は許可しない。これがあると CRT が静的リンクされていない(S-09)。

# S-09: CRT のDLL(msvc のとき、あれば「CRT が静的リンクされていない」)。
CRT_DLLS=('vcruntime*' 'msvcp*' 'ucrtbase*' 'api-ms-win-crt-*')

# S-01: 通信に使うDLL(拡張子を除いた名前。大文字小文字を区別しない)。check_source.sh の NET_DLLS と同じ。
NET_DLLS=(ws2_32 mswsock winhttp wininet urlmon iphlpapi dnsapi)

# S-04、S-05: 禁止API(前方一致。CreateProcessW、ShellExecuteExW なども当たる)。
# check_source.sh の BANNED_API と同じ一覧に保つ。
BANNED_API=(
  SendInput keybd_event mouse_event SetWindowsHookEx
  OpenProcess WriteProcessMemory ReadProcessMemory CreateRemoteThread
  CreateProcess ShellExecute WinExec
)

# S-03、S-06: レジストリ・クリップボード・INI・メタファイルなどに書くAPI(前方一致)。標準ライブラリが
# 決してインポートしないものだけを並べるので、インポートにあれば自前のコードが呼んでいる。
# 標準ライブラリが std::fs のために使うファイルのAPI(CreateFileW、MoveFileExW、CreateDirectoryW、
# RemoveDirectoryW、CreateHardLinkW、CreateSymbolicLinkW、SetFileAttributesW など)は、
# ここでは調べない(check_source.sh が受け持つ)。
WRITE_API=(
  RegSetValue RegSetKeyValue RegCreateKey SHSetValue
  OpenClipboard SetClipboardData OleSetClipboard
  WriteProfile WritePrivateProfile # INI ファイル(win.ini も。WriteProfileString、WriteProfileSection)
  SHCreateDirectory SHFileOperation      # シェルのフォルダー作成、ファイル操作
  # メタファイル。そのDCに画面から BitBlt すると、画面の内容がファイルに残る
  CreateEnhMetaFile CreateMetaFile CopyEnhMetaFile CopyMetaFile
)

# S-15: ログの出力先になるAPI(前方一致)。
# shellcheck disable=SC2034 # sym_check が配列の名前で参照する
LOG_API=(OutputDebugString)

# 8章: exe サイズの上限(3MiB)。
MAX_SIZE=3145728

# ---- 引数と道具 ------------------------------------------------------------------------

usage() {
  echo "使い方: tools/check_exe.sh <exe> <msvc|gnu>" >&2
  exit 2
}
[[ $# -eq 2 ]] || usage
MODE=$2
[[ $MODE == msvc || $MODE == gnu ]] || usage

have_cygpath=0
command -v cygpath >/dev/null 2>&1 && have_cygpath=1
to_unix() { if [[ $have_cygpath -eq 1 ]]; then cygpath -u "$1"; else printf '%s\n' "$1"; fi; }
to_native() { if [[ $have_cygpath -eq 1 ]]; then cygpath -w "$1"; else printf '%s\n' "$1"; fi; }

EXE=$(to_unix "$1")
if [[ ! -f $EXE ]]; then
  echo "check_exe.sh: exe が見つかりません: $1" >&2
  exit 2
fi
EXE_NATIVE=$(to_native "$EXE") # llvm-readobj(Windows のネイティブのプログラム)に渡す形

find_readobj() {
  local sysroot host dir c
  if [[ -n ${LLVM_READOBJ:-} ]]; then # パスでも、PATH にあるコマンド名でもよい
    if [[ -f $LLVM_READOBJ ]]; then to_unix "$LLVM_READOBJ"; else command -v "$LLVM_READOBJ"; fi
    return
  fi
  command -v rustc >/dev/null 2>&1 || return 1
  sysroot=$(rustc --print sysroot | tr -d '\r')
  sysroot=$(to_unix "$sysroot")
  host=$(rustc -vV | tr -d '\r' | sed -n 's/^host: //p')
  dir="$sysroot/lib/rustlib/$host/bin"
  for c in "$dir/llvm-readobj" "$dir/llvm-readobj.exe"; do
    if [[ -f $c ]]; then
      printf '%s\n' "$c"
      return
    fi
  done
  return 1
}
if ! READOBJ=$(find_readobj) || [[ ! -f $READOBJ ]]; then
  echo "check_exe.sh: llvm-readobj が見つかりません。rust-toolchain.toml の llvm-tools を入れるか、LLVM_READOBJ で場所を指定してください" >&2
  exit 2
fi

readobj() { # readobj <オプション>: 出力(CR を除く)。失敗したら止める
  local out
  if ! out=$("$READOBJ" "$1" "$EXE_NATIVE" 2>&1); then
    printf 'check_exe.sh: llvm-readobj %s が失敗しました:\n%s\n' "$1" "$out" >&2
    exit 2
  fi
  printf '%s\n' "${out//$'\r'/}"
}

IMPORTS=$(readobj --coff-imports)
HEADERS=$(readobj --file-headers)
LOADCFG=$(readobj --coff-load-config)

# ---- 結果の出し方 ----------------------------------------------------------------------

total=0
ng=0
skipped=0
pass() {
  total=$((total + 1))
  printf 'OK  %s %s\n' "$1" "$2"
}
fail() { # fail 要件ID 内容 原因(複数行可)
  total=$((total + 1))
  ng=$((ng + 1))
  printf 'NG  %s %s\n' "$1" "$2"
  printf '%s\n' "$3" | awk 'NF > 0 { print "      " $0 }'
}
skip() {
  total=$((total + 1))
  skipped=$((skipped + 1))
  printf -- '--  %s %s(%s では調べない)\n' "$1" "$2" "$MODE"
}
result() { # result 要件ID 内容 原因: 原因が空なら OK
  if [[ -z $3 ]]; then pass "$1" "$2"; else fail "$1" "$2" "$3"; fi
}

# ---- インポート表 ----------------------------------------------------------------------
# 行頭の "Import {" と "DelayImport {" で区切り、DLL と関数を「種類<TAB>DLL<TAB>関数」にする。
# 遅延読み込みの関数は DelayImport の中の入れ子の "Import {" にある(Name の行はない)。
TABLE=$(printf '%s\n' "$IMPORTS" | awk '
  /^Import \{/ { kind = "通常"; dll = ""; next }
  /^DelayImport \{/ { kind = "遅延読み込み"; dll = ""; next }
  /^[ ]+Name: / && kind != "" { dll = $0; sub(/^[ ]+Name: /, "", dll); print kind "\t" dll "\t"; next }
  /^[ ]+Symbol: / && kind != "" {
    s = $0; sub(/^[ ]+Symbol: /, "", s); sub(/ *\([0-9]+\)$/, "", s)
    if (s == "") { s = $0; sub(/^.*\(/, "#", s); sub(/\).*$/, "", s) } # 序数だけのインポートは #序数
    print kind "\t" dll "\t" s
  }')

dlls=()      # 「DLL(種類)」の表示用
dll_names=() # DLL 名
dll_kinds=()
syms=() # 「DLL<TAB>関数<TAB>種類」
while IFS=$'\t' read -r kind dll sym; do
  [[ -n $dll ]] || continue
  if [[ -z $sym ]]; then
    dll_names+=("$dll")
    dll_kinds+=("$kind")
    if [[ $kind == 通常 ]]; then dlls+=("$dll"); else dlls+=("$dll(遅延読み込み)"); fi
  else
    syms+=("$dll"$'\t'"$sym"$'\t'"$kind")
  fi
done <<<"$TABLE"

size=$(wc -c <"$EXE")
size=${size//[!0-9]/}
echo "exe の検査: $1($MODE、${size} バイト)"
if [[ ${#dlls[@]} -gt 0 ]]; then
  echo "インポートしているDLL: $(printf '%s, ' "${dlls[@]}" | sed 's/, $//')"
else
  echo "インポートしているDLL: なし"
fi

dll_label() { if [[ $2 == 通常 ]]; then printf '%s' "$1"; else printf '%s(%s)' "$1" "$2"; fi; }

# S-01
why=""
for i in "${!dll_names[@]}"; do
  base=${dll_names[$i],,}
  base=${base%.dll}
  for n in "${NET_DLLS[@]}"; do
    [[ $base == "$n" ]] && why+="$(dll_label "${dll_names[$i]}" "${dll_kinds[$i]}")"$'\n'
  done
done
result "S-01" "通信用のDLL($(printf '%s ' "${NET_DLLS[@]}" | sed 's/ $//; s/ /、/g'))をインポートしていない" "$why"

# S-09 許可リスト
allowed=("${ALLOWED_DLLS[@]}")
[[ $MODE == gnu ]] && allowed+=("${ALLOWED_DLLS_GNU[@]}")
why=""
for i in "${!dll_names[@]}"; do
  lc=${dll_names[$i],,}
  ok=0
  for pat in "${allowed[@]}"; do
    # shellcheck disable=SC2053 # 右辺はパターンとして照合する
    [[ $lc == $pat ]] && ok=1 && break
  done
  [[ $ok -eq 1 ]] || why+="$(dll_label "${dll_names[$i]}" "${dll_kinds[$i]}") は許可リストにない"$'\n'
done
result "S-09" "インポートするDLLが許可リスト(tools/check_exe.sh の ALLOWED_DLLS)に収まる" "$why"

# S-09 CRT の静的リンク
if [[ $MODE == msvc ]]; then
  why=""
  for i in "${!dll_names[@]}"; do
    lc=${dll_names[$i],,}
    for pat in "${CRT_DLLS[@]}"; do
      # shellcheck disable=SC2053
      [[ $lc == $pat ]] && why+="$(dll_label "${dll_names[$i]}" "${dll_kinds[$i]}")"$'\n' && break
    done
  done
  result "S-09" "CRT が静的リンクされている(vcruntime、msvcp、ucrtbase、api-ms-win-crt-* をインポートしていない)" "$why"
else
  skip "S-09" "CRT が静的リンクされている"
fi

# 関数の前方一致: sym_check 配列名 → 当たった「DLL: 関数(種類)」
sym_check() {
  local -n list=$1
  local entry dll sym kind name out=""
  for entry in "${syms[@]+"${syms[@]}"}"; do
    IFS=$'\t' read -r dll sym kind <<<"$entry"
    for name in "${list[@]}"; do
      if [[ $sym == "$name"* ]]; then
        out+="$(dll_label "$dll" "$kind"): $sym"$'\n'
        break
      fi
    done
  done
  printf '%s' "$out"
}

result "S-04 S-05" "禁止API($(printf '%s ' "${BANNED_API[@]}" | sed 's/ $//; s/ /、/g'))をインポートしていない" "$(sym_check BANNED_API)"
result "S-03 S-06" "レジストリ・クリップボード・INI・メタファイルに書くAPI($(printf '%s ' "${WRITE_API[@]}" | sed 's/ $//; s/ /、/g'))をインポートしていない" "$(sym_check WRITE_API)"
result "S-15" "OutputDebugString をインポートしていない" "$(sym_check LOG_API)"

# ---- PE ヘッダー(S-10) -------------------------------------------------------------------
# ImageOptionalHeader の中の Characteristics が DllCharacteristics(IMAGE_DLL_CHARACTERISTICS_*)。
flags=$(printf '%s\n' "$HEADERS" | sed -n '/^ImageOptionalHeader {/,/^}/p' | grep -o 'IMAGE_DLL_CHARACTERISTICS_[A-Z_]*' || true)
has_flag() { grep -qx "IMAGE_DLL_CHARACTERISTICS_$1" <<<"$flags"; }
flag_list=$(printf '%s\n' "$flags" | sed 's/^IMAGE_DLL_CHARACTERISTICS_//' | awk 'NF > 0' | paste -sd ' ' -)
why=""
for f in DYNAMIC_BASE HIGH_ENTROPY_VA NX_COMPAT; do
  has_flag "$f" || why+="IMAGE_DLL_CHARACTERISTICS_$f がない"$'\n'
done
[[ -z $why ]] || why+="DllCharacteristics にあるもの: ${flag_list:-なし}"
result "S-10" "DllCharacteristics に DYNAMIC_BASE(ASLR)、HIGH_ENTROPY_VA、NX_COMPAT(DEP)がある" "$why"

if [[ $MODE == msvc ]]; then
  why=""
  has_flag GUARD_CF || why="IMAGE_DLL_CHARACTERISTICS_GUARD_CF がない(DllCharacteristics にあるもの: ${flag_list:-なし})"
  result "S-10" "DllCharacteristics に GUARD_CF(CFG)がある" "$why"
else
  skip "S-10" "DllCharacteristics に GUARD_CF(CFG)がある"
fi

# ---- ロード構成(S-09 DependentLoadFlags) -------------------------------------------------
# 書式の揺れ(0x800 / 0x0800 / 2048、後ろに名前が付く形)を許す。読めなければ該当部分をそのまま出して NG。
if [[ $MODE == msvc ]]; then
  block=$(printf '%s\n' "$LOADCFG" | sed -n '/^LoadConfig \[/,/^\]/p')
  line=$(printf '%s\n' "$block" | grep -E '^[[:space:]]*DependentLoadFlags[[:space:]]*:' || true)
  why=""
  if [[ -z $block ]]; then
    why="ロード構成(LoadConfig)がない。llvm-readobj --coff-load-config の出力:"$'\n'"$(printf '%s\n' "$LOADCFG" | head -n 30)"
  elif [[ -z $line ]]; then
    why="DependentLoadFlags の行が読めない。llvm-readobj --coff-load-config の LoadConfig:"$'\n'"$(printf '%s\n' "$block" | head -n 30)"
  elif ! grep -qE ':[[:space:]]*(.*[^0-9A-Fa-fXx])?(0[xX]0*800|2048)([^0-9A-Fa-f]|$)' <<<"$line"; then
    why="$(printf '%s' "$line" | sed 's/^[[:space:]]*//')(0x800 でない)"
  fi
  result "S-09" "ロード構成の DependentLoadFlags が 0x800(LOAD_LIBRARY_SEARCH_SYSTEM32)" "$why"
else
  skip "S-09" "ロード構成の DependentLoadFlags が 0x800"
fi

# ---- マニフェスト(S-02) ----------------------------------------------------------------
# exe の中の文字列で調べる。リンカーがマニフェストを書き直すと引用符が ' になることがあるので両方を許す。
if [[ $MODE == msvc ]]; then
  why=""
  grep -a -q -E "level[[:space:]]*=[[:space:]]*[\"']asInvoker[\"']" "$EXE" || why+="level=\"asInvoker\" がない"$'\n'
  grep -a -q -E "uiAccess[[:space:]]*=[[:space:]]*[\"']false[\"']" "$EXE" || why+="uiAccess=\"false\" がない"$'\n'
  grep -a -q -E "requireAdministrator" "$EXE" && why+="requireAdministrator がある"$'\n'
  grep -a -q -E "highestAvailable" "$EXE" && why+="highestAvailable がある"$'\n'
  if [[ -n $why ]]; then
    found=$(grep -a -o -E 'requestedExecutionLevel[^>]{0,120}' "$EXE" || true)
    if [[ -n $found ]]; then why+="exe の中の記述: $found"; else why+="requestedExecutionLevel が見つからない(マニフェストが埋め込まれていない)"; fi
  fi
  result "S-02" "マニフェストが level=\"asInvoker\" と uiAccess=\"false\" で、requireAdministrator と highestAvailable がない" "$why"
else
  skip "S-02" "マニフェストが asInvoker / uiAccess=false"
fi

# ---- サイズ(8章) -----------------------------------------------------------------------
why=""
[[ $size -le $MAX_SIZE ]] || why="${size} バイト(上限 ${MAX_SIZE} バイト)"
result "8章" "サイズが 3MiB(${MAX_SIZE} バイト)以下(${size} バイト)" "$why"

# ---- まとめ ----------------------------------------------------------------------------
summary="まとめ: ${total} 件中 OK $((total - ng - skipped)) 件、NG ${ng} 件"
[[ $skipped -eq 0 ]] || summary+="、調べない ${skipped} 件"
echo "$summary"
if [[ $ng -gt 0 ]]; then
  exit 1
fi
