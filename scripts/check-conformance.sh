#!/usr/bin/env bash
# scripts/check-conformance.sh — conformance/ のケースについて、次を確かめる。
#   1. すべての .cdl に、期待結果（<名前>.expected.json）がある
#   2. 同じ入力を 2 回（順方向・逆方向）評価して、出力が完全に一致する。パニック・異常終了がない
#   3. 禁止構文の各ケース conformance/forbidden/<id>.cdl があり、診断コード付きで弾かれる
#   4. 診断コードの各帯（E0xxx〜E5xxx）に、期待結果のケースが 1 つ以上ある
#
# 期待結果と出力の一致そのものは、Rust 側のテストハーネスの責務。ここでは見ない。
# 読み取り専用（cargo build でバイナリを作る以外は、何も書かない）。
#
# 前提とする CLI の約束（cdl-cli）:
#   cdl check --json <file>   診断を JSON で出す。誤りがあれば終了コード 0 以外
#   cdl eval  --json <file>   値木または診断を JSON で出す
# 環境変数: CONF_DIR（既定 conformance）, CDL_BIN（既定 target/debug/cdl）
# 終了コード: 0 = OK / 1 = NG あり / 2 = 未実行（前提が足りない）
set -u
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || exit 1

CONF=${CONF_DIR:-conformance}
CDL_BIN=${CDL_BIN:-target/debug/cdl}
fail=0

ok() { printf '  OK   %s\n' "$1"; }
ng() { printf '  NG   %s\n' "$1"; fail=1; }

if [ ! -d "$CONF" ]; then
  echo "未実行: $CONF がない"
  exit 2
fi

if [ ! -x "$CDL_BIN" ]; then
  echo "$CDL_BIN がないので、cargo build -p cdl-cli を実行します"
  cargo build -q -p cdl-cli || { echo "未実行: cdl-cli がビルドできない"; exit 2; }
fi
if [ ! -x "$CDL_BIN" ]; then
  echo "未実行: $CDL_BIN が見つからない（CDL_BIN で指定できます）"
  exit 2
fi

TO=()
command -v timeout >/dev/null 2>&1 && TO=(timeout 10)

mapfile -t files < <(find "$CONF" -name '*.cdl' -type f | LC_ALL=C sort)
if [ "${#files[@]}" -eq 0 ]; then
  echo "未実行: $CONF に .cdl のケースがない"
  exit 2
fi
echo "ケース数: ${#files[@]}"

# run_eval <file>: 出力と終了コードをひとまとめに標準出力へ
run_eval() {
  local out rc
  out=$("${TO[@]}" "$CDL_BIN" eval --json "$1" 2>&1)
  rc=$?
  printf '%s\nexit=%s' "$out" "$rc"
}

echo "[1] 期待結果ファイル"
missing=0
# 複数ファイルのテストは、ディレクトリに置いた expected.json が 1 つの期待結果になる。
# その配下の .cdl（main.cdl とモジュール）には、個別の期待結果を要求しない。
in_multi_file_test() {
  local d
  d=$(dirname "$1")
  while [ "$d" != "$CONF" ] && [ "$d" != "." ] && [ "$d" != "/" ]; do
    [ -f "$d/expected.json" ] && return 0
    d=$(dirname "$d")
  done
  return 1
}
for f in "${files[@]}"; do
  if [ ! -f "${f%.cdl}.expected.json" ] && ! in_multi_file_test "$f"; then
    case $f in */forbidden/*) ;; *) ng "期待結果がない: $f"; missing=1 ;; esac
  fi
done
[ "$missing" = 0 ] && ok "すべてのケースに期待結果がある（forbidden/ は診断の有無を [3] で確かめる）"

echo "[2] 決定性（2 回評価・順方向と逆方向）と、パニックがないこと"
declare -A first
bad=0
for f in "${files[@]}"; do
  first[$f]=$(run_eval "$f")
done
for ((i = ${#files[@]} - 1; i >= 0; i--)); do
  f=${files[$i]}
  second=$(run_eval "$f")
  if [ "${first[$f]}" != "$second" ]; then
    ng "出力が一致しない: $f"; bad=1
  fi
  rc=${second##*exit=}
  if [ "$rc" = 101 ] || [ "$rc" -ge 124 ] 2>/dev/null; then
    ng "パニックまたは異常終了（exit=$rc）: $f"; bad=1
  fi
done
[ "$bad" = 0 ] && ok "全ケースで出力が一致し、パニックもない"

echo "[3] 禁止構文が、診断コード付きで弾かれること"
forbidden_ids="let var class new this null for while try async enum as hooks event-handler ref destructuring rest-param and-nonboolean"
bad=0
for id in $forbidden_ids; do
  f="$CONF/forbidden/$id.cdl"
  if [ ! -f "$f" ]; then ng "ケースがない: $f"; bad=1; continue; fi
  out=$("${TO[@]}" "$CDL_BIN" check --json "$f" 2>&1)
  rc=$?
  if [ "$rc" = 0 ]; then ng "$id: 弾かれていない（終了コード 0）"; bad=1
  elif [ "$rc" = 101 ] || [ "$rc" -ge 124 ]; then ng "$id: パニックまたは異常終了（exit=$rc）"; bad=1
  elif ! printf '%s' "$out" | grep -Eq 'E[0-9]{4}'; then ng "$id: 診断コード（E0000 形式）が出ていない"; bad=1
  fi
done
[ "$bad" = 0 ] && ok "禁止構文 $(echo $forbidden_ids | wc -w) 件すべて、診断コード付きで弾かれる"

echo "[4] 診断コードの各帯に、期待結果のケースがあること"
bad=0
for b in 0 1 2 3 4 5; do
  if grep -rEq "\"code\"[ ]*:[ ]*\"E${b}[0-9]{3}\"" "$CONF" --include='*expected.json'; then
    ok "E${b}xxx"
  else
    ng "E${b}xxx のケースがない"; bad=1
  fi
done

echo
if [ "$fail" = 0 ]; then echo "check-conformance: すべて OK"; else echo "check-conformance: NG あり"; fi
exit "$fail"
