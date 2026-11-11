# CDL 最終精査

2026-11-11。コードは変更せず、読んだ箇所と、実際に走らせた入力で確かめた。判定は「確かめた / 確かめられなかった / 問題あり」の 3 種類。
「再現」は実際に実行して起きたこと、「推測」はコードを読んだだけのこと。問題は直していない。

実行環境: `target/debug/cdl` と `target/release/cdl`（`cargo build -p cdl-cli`、`--release`）。メモリ上限は `systemd-run --user --scope -p MemoryMax=… -p MemorySwapMax=0`。スタックは `ulimit -s` で変えた（Linux の主スレッドの既定は 8 MiB）。再現用の入力の生成は、本文の「再現入力」の式のとおり（一時ファイルは残していない）。

## 結論（修正前。修正後は「対処状況」）

| 主張 | 判定 |
| --- | --- |
| A. 公開 API から到達できるパニックがない | **問題あり**（スタックオーバーフローによる abort を 3 経路で再現。通常のパニックは見つからず） |
| B. 必ず停止する | **問題あり**（実用上の非停止 2 経路を再現。循環検査の抜けは見つからず） |
| C. 決定的である | 確かめた（ただし精査の範囲に限る。下記） |
| D. 資源上限が迂回できない | **問題あり**（総量・中間バッファ・型の大きさに上限がない） |
| E. 境界が守られている | 確かめた |
| F. 仕様と実装の食い違い | 問題あり（軽微な差が 4 件。SPEC 全文の突き合わせはしていない） |

## 対処状況（2026-11-11 追記）

P1〜P5 は、同日、先に `conformance/` のテストを足して修正した。以下は修正後に、元の再現入力を release ビルド（スタック 2 MiB・メモリ上限 1 GiB）で再実行した結果。すべて診断で止まる。上の本文は、修正前の状態の記録として残す。

| 問題 | 修正 | 再実行の結果 | 適合テスト |
| --- | --- | --- | --- |
| P1 | 前走査の連鎖に `keyof` `readonly` `unique` `infer` `asserts` を足した（DECISIONS C19） | `keyof` × 50,000 は E1008 | `7/nesting-keyof-chain` |
| P2 | 型の深さ 128・ノード数 4,096・別名の参照の深さ 128 の上限（E3013。D12）。`const` の型にも適用 | 倍々の型 k=22・別名 10,000 / 40,000 個・`const` の倍々は E3013 | `4/type-too-large` |
| P3 | 評価の再帰の深さ 128（E5012。D15） | 関数 64 個 × `${` 58 段・`-(` 28 段は E5012 | `7/eval-depth` |
| P4 | 値木の深さの検査が、たどった値ごとに 1 ステップを使う（D13） | 倍々の共有 k=62・`n16.map((x) => [n16])` は E5002（25 ms） | `7/shared-value-steps` |
| P5 | 文字列の生成に 256 バイトごと 1 ステップ、テンプレートの途中でも長さを検査（D14） | 2 GiB の入力は E5002、テンプレートは E5004（1 GiB の上限内） | `7/string-total-steps` `7/string-template-limit` |

- 修正後の `scripts/check-all.sh`: rules・fmt・clippy・test・conformance・spec-examples が OK、kani は未実行（使わない方針）。`cargo test --workspace` は 125 件で失敗 0。突然変異ファジング 3,000 件を追加で実行し、異常は 0 件。
- 副作用: 型の深さ・ノード数の上限は `Limits` に出さない固定値。E5002 が、共有された値を大量にたどる入力でも出るようになった（`dag20` のように 2^20 回たどる入力は、既定の 100 万ステップでは通らない）。`cdl-eval/tests/eval.rs` の `value_depth_default_bounds_const_chains` は、5,000 個の連鎖が型検査の E3013 で止まる形に直した。
- 残りの対処（同日、2 回目）:
  - P6: `Limits::max_modules`（既定 256。E5013。DECISIONS D16）。境界テスト `module_count_limit`（cdl-host/tests/limits.rs）。
  - P7: E4008 のメッセージに値の `Debug` を入れず、種類だけにした。
  - `Limits` の極端な値: 全項目を 0 にした場合、最大にした場合、1 項目ずつ 0 にした場合で、パニックしないことを `extreme_limits_never_panic` で確かめた。フック呼び出し（`max_hook_steps` を 0 にして `call_hook`）は、このテストの対象外（未確認）。
  - `check.rs`・`convert.rs` の全行精読は**していない**。代わりに、再帰する関数を列挙し（check.rs: `resolve_expr` `resolve_stmt` `resolve_children` `resolve_type_names` `ty` `stmt` `expr` `element` `children` `facts`、types.rs: `assignable` `widen` `union` `fmt` と派生の `Clone`/`Drop`、convert.rs の `expr` `ty` `stmt` `children`）、再帰の深さが構文の入れ子（上限 64）と型の深さ（上限 128）で抑えられることを確かめた。確かめ方: 22 種類の構文（括弧・単項・配列・オブジェクト・テンプレート・三項・アロー・二項・`satisfies`・メンバー・呼び出し・JSX・各種の型・`if` の連鎖）を、入れ子の上限ぎりぎりまで深くした入力を、debug ビルド・スタック 2 MiB で走らせ、abort が 0 件。ただし、前走査（`lex.rs`）が数えない構文が、ほかにあるかもしれない点は残る（P1 のような漏れの可能性。推測）。
  - 既定値の妥当性: `examples/contrast/` の設定は、ステップ 140、評価の再帰 10、ノード 22 で通る（既定は 100 万・128・100 万）。回帰を `cdl-cli/tests/headroom.rs` が守る。実際の設定ファイルの規模での実測は、していない（例が小さい）。
  - oxc が内部で再帰する点: 前走査が数える構文については、1 MiB 以下の入力で abort しない（上の実験）。oxc 自体のソースは精査していない。

## 問題の一覧（重大度の高い順）

### P1（高）構文の前置キーワードの連鎖で abort（A・B）— 再現

- 場所: `crates/cdl-syntax/src/lex.rs:143`（`NESTING_KEYWORDS`）。入れ子の前走査が数えるキーワードは `if typeof void delete await new yield` だけで、型の前置演算子 `keyof` `readonly` `unique`（と、おそらく `infer` `asserts`。**推測**）を数えない。oxc が再帰で読む。
- 再現入力: `"use cdl 1";\ntype T = ` + `keyof ` × N + `string;`（`readonly ` + `string[]`、`unique ` + `symbol` でも同じ）。
  - release/debug とも、主スレッド 8 MiB で N=50,000（約 300 KB）が `stack overflow, aborting`（終了コード 134）。N=30,000 は debug では正常（E1005）、release では abort。release は N=15,000 まで正常。
  - スタック 2 MiB では、debug が N=10,000（約 60 KB）で abort（N=6,000 は正常）、release が N=6,000 で abort（N=4,000 は正常）。
- 影響: 1 MiB 以下の入力で、ホストのプロセスごと落ちる。SPEC §10「パニックも abort もしない」と §7「構文ネスト深さ 64」に反する。
- 対照: `satisfies` の連鎖（N=50,000）、呼び出し・添字・`!`・`Array<…>`・`T[]`・`{a:…}`・関数型の連鎖（N=100,000）は E1008 で正常。

### P2（高）型の大きさに上限がない（A・D）— 再現

- 場所: `crates/cdl-sema/src/types.rs:9`（`Ty` は所有する木）、`check.rs:905`（型エイリアスの参照のたびに `self.ty(&a.ty)` で展開して複製）。
- 再現 1（メモリ）: `type T0 = {a:number;b:number}; type T1 = {a:T0;b:T0}; … type T20 = {a:T19;b:T19};`（21 行・約 1 KB）。k=16 は 0.26 秒で通り、k=20 と 22 は 3 GiB の上限で OOM kill（終了コード 137）。倍々なので k=62 で事実上終わらない。
- 再現 2（スタック）: `type T0 = number; type T1 = T0[]; … type T10000 = T9999[];` と `function f(x: T10000): number {…}`（約 218 KB）。release/debug とも 8 MiB で abort。N=2,000 は正常。
- 影響: 小さな入力でメモリ枯渇または abort。評価の上限（SPEC §7）より前の段階なので、`Limits` で防げない。
- 注: 値の深さは E5011 で止まる（`const c{i} = [c{i-1}]` を 20,000 個並べても E5011。再現）。型にだけ上限がない。

### P3（高）評価器の再帰がスタックを使い切る（A）— 再現

- 場所: `crates/cdl-eval/src/eval.rs:437`（`expr`）、`:659`（`call_value`）。呼び出しの深さ上限（64）と構文の入れ子の上限（64）は別々に効くので、再帰の深さは両者の積（約 64 × 60 フレーム）になるが、スタックを守るものがない。
- 再現入力: 関数 `f0 … f63`。`f0(x)=x`、`fi(x)` は ``return `${`${ … `${f{i-1}(x)}` … }`}`;``（`${` を 58 段）。`const r = f63("a");` と `export default (<Contrast />);`（約 21 KB）。
  - release: スタック 2 MiB で abort、8 MiB は正常。関数の数を 48 以下にすると 2 MiB でも正常。三項演算子の連鎖（`true ? 1 : ` を 58 段）の同形の入力は、release の 2 MiB で正常。
  - debug: `-(` を 16 段ずつ重ねた版（`f0 … f63`）、あるいは `${` を 20 段重ねた版で、8 MiB の主スレッドが abort。
  - release で `-(` を 28 段 × 64 関数は 2 MiB で通り、1 MiB で abort。
- 影響: 評価は `Engine::load` から到達する。ホストが 2 MiB 程度のスレッドで呼ぶ、あるいはデバッグビルドで動かすと、20 KB の入力で abort する。型検査・構文解析までは通る（`--stage sema` は正常）。
- 補足: `cargo test` の評価系のテストはこの深さを試していない（HANDOFF の「未確認」と一致）。

### P4（高）共有された値の深さ検査が指数時間で、ステップに数えられない（B）— 再現

- 場所: `crates/cdl-eval/src/value.rs:175`（`depth_exceeds`）。配列・オブジェクト・要素を作るたびに `eval.rs:403`（`bounded`）が呼ぶ。値は `Arc` で共有されるが、検査は共有を木として何度もたどり、作業をステップに数えない。`count_nodes`（`value.rs:207`）の内側のループも、上限で打ち切らない。
- 再現入力: `const a0 = [1, 1]; const a1 = [a0, a0]; … const a{k-1} = [a{k-2}, a{k-2}];` と `export default (<Contrast />);`。release で k=22: 51 ms、26: 530 ms、28: 1.96 秒、30: 8.0 秒。k=62 の 62 行（約 3 KB）で、評価ステップは数百なのに、計算量は 2^62。
- 変種（再現）: `const r = n16.map((x) => [n16]);`（`n16` は `concat` の倍々で作った 65,536 要素の配列。ステップは約 33 万）が、release で 14.6 秒。k=13: 0.23 秒、14: 0.8 秒、15: 3.3 秒で、要素数の 2 乗で増える。DECISIONS D11 の「既知の限界」は、これを「1 MiB のソースで抑えられる」としているが、時間は抑えられていない。
- 影響: `Limits` の `max_steps` では止まらない。SPEC §2 の停止性・§7「評価ステップ上限で保証」に反する（実用上の非停止）。

### P5（中〜高）メモリの総量・途中のバッファに上限がない（D）— 再現

- 総量: 個々の上限は 1 値あたり（文字列 64 KiB、配列 65,536 要素）。`const s12`（32 KiB、`${s}${s}` の 12 回の倍化）と `const n16`（65,536 要素、`concat` の倍化）を作り、`n16.map((x) => `${s12}${x}`)` を評価すると、異なる文字列 65,536 個（約 2 GiB）ができる。入力は約 1.5 KB、release で 0.8 秒、評価ステップは 100 万未満。3 GiB の上限では通り、1 GiB の上限では OOM kill（137）。上限を使えば 12 GiB 以上も作れる（**推測**。文字列を 20 万個作れる計算）。
- 途中のバッファ: `eval.rs:445`（テンプレート）は、組み立て中の `String` に長さの検査がなく、完成してから `string()`（`:422`）で検査する。`${s12}` を 40,000 回書いたテンプレート（約 240 KB）は、3 GiB の上限では E5004 になるが、1 GiB の上限では組み立ての途中で OOM kill（137）。1 MiB のソースなら約 5.5 GiB に達する（**推測**）。
- 影響: SPEC §7 の表の上限を守っても、ホストのメモリを使い切れる。SPEC に総量の上限がない（食い違いではなく、仕様の穴）。

### P6（低）ホストモジュール数・総ソース量に上限がない（D）— 推測

- `crates/cdl-host/src/load.rs` の `collect` は、リゾルバが返すファイルを無制限にたどる。上限は 1 ファイルあたりのソースサイズ（`max_source_bytes`）だけ。リゾルバが実在のファイルを返す限り有限。実行していない。

### P7（低）E4008 のメッセージが値の `Debug` を丸ごと出す（D）— 推測

- `crates/cdl-host/src/convert.rs` の `optional_prop` は `{v:?}` を入れる。大きな値や P4 の共有された値では、メッセージが巨大になる。スキーマと変換コードが食い違うときだけ通る経路。実行していない。

## A. パニック・abort

確かめた:
- 全クレートが `[lints] workspace = true` と `#![forbid(unsafe_code)]` を持つ（`Cargo.toml` と各 `lib.rs`/`main.rs`）。`src/` に `#[allow(` はない（grep）。`unwrap`/`expect`/`panic`/添字/`as`/算術は clippy で deny（`Cargo.toml` の `[workspace.lints.clippy]`）。この結果は、このセッションでは clippy を実行していないので、**lint 設定を読んだだけ**。
- `scripts/check-rules.sh` の静的検査（`.unwrap()` など）を `src/` に対して読んだ。ただしこのスクリプトは各ファイルの最初の `#[cfg(test)]`/`#[cfg(kani)]` 以降を走査しない。
- パニックしうる標準関数の grep（`split_at`・`repeat`・`Vec::insert`・`pow`・`abs`・シフト・`with_capacity` など）: `num.rs:66,71` の `split_at` は `min` で抑えている（`js_number_string`）。`render.rs` の `locate` は `get` で範囲外・文字境界外の span を `None` にする。`with_capacity` の引数は既存の配列の長さ。
- 突然変異ファジング（このセッションで作った一時スクリプト。conformance と examples の `.cdl` を、トークンの挿入・削除・反復・任意の文字で 1〜4 回変えた入力）: debug ビルドの `cdl check --json` に 3,000 件 + 12,000 件、パニック・異常終了・20 秒超過は 0 件。debug ビルドなので算術の溢れも検出される。入力の種類は限られる（深い入れ子・巨大な入力は含まない）。
- 評価器の `Err(internal(..))` 経路（`eval.rs`）は、型検査を通った式が評価で型の誤りになったときの E5000 で、パニックしない。

問題あり: P1・P2・P3（abort）。

確かめられなかった:
- `cdl-sema/src/check.rs`（3,002 行）と `cdl-syntax/src/convert.rs`（1,516 行）の全行の精読。再帰の深さは、構文ネストの上限（P1 のとおり前走査は近似）に頼っている。
- `Limits` に 0 や極端な値を入れたときの挙動（`max_call_depth: 0`、`max_value_depth: 0` など）。
- `cdl-cli` の `FsResolver`（シンボリックリンク・巨大ファイル）。
- oxc 自体の内部の挙動（`keyof` 以外にも、前走査が数えない構文があるかもしれない）。

## B. 停止性

確かめた:
- 循環の検査: 参照グラフは `check.rs:274-346` で、`graph::sccs`（`graph.rs:16`。Tarjan の方法。明示のスタックで、再帰しない）に渡す。次の入力はすべて診断で止まった（再現）: 直接再帰 E2005、相互再帰 E2005、`const f = (n) => f(n)` E2006、関数と const の循環 E2006、オブジェクトのメンバーの自己参照 E2006、関数の中のアロー関数からの再帰 E2005、コンポーネントが自分を子に置く E2005、`type A = {a: A | undefined}` E2007、型の相互循環 E2007、import の循環 E2010。高階関数（`ap(f, x)` に関数を渡す）は通る。
- 反復は `map`/`filter`/`reduce`/`includes`/`concat` だけ。各要素の処理で `step()` を呼ぶ（`eval.rs:737,746,761,771`）。`concat` と、配列・オブジェクト・属性のスプレッドは、要素数ぶんを `charge` する（`:467,491,784,863`）。`expr`（`:438`）と `stmt`（`:811`）は 1 ステップを使う。ユーザーのコンポーネントと関数の呼び出しは `call_value`（`:659`）で深さを数える。ステップの予算は減るだけ（`budget.rs:23`。単調性の Kani ハーネスは**未実行**）。
- フックは別の予算（`lib.rs:278-282`）。呼び出しごとに `reset_budget` で深さも 0 に戻る。

問題あり: P4（上限に数えられない作業が指数時間・2 乗時間になる）。P3 の abort も停止しないという意味では同じ。

確かめられなかった:
- 型検査（`check.rs`）の計算量の上限。P2 のとおり、型の展開は上限がない。ほかに指数時間になる経路（`assignable` など）があるかは見ていない。

## C. 決定性

確かめた:
- `HashMap`/`HashSet`/`SystemTime`/`Instant`/`std::env`/`std::fs`/乱数は、`crates/*/src` の grep で、`cdl-cli` 以外に出ない（`cdl-cli/src/main.rs:8` が `env::args`、`lib.rs:79` が `fs::read_to_string`）。`clippy.toml` の `disallowed-types`/`disallowed-methods` も読んだ（clippy は未実行）。
- 値のマップは `BTreeMap`（`value.rs`）。アロー関数の番号は、ポインタをキーにした `BTreeMap`（`eval.rs:54`）だが、引くだけで、ポインタの順序は観測できない（番号は走査順）。
- f64 の文字列化は自前の `js_number_string`（`num.rs:48`。`format!("{:e}")` を整形。ロケールに依存しない）。JSON の数値は `serde_json`（`cdl-cli/src/json.rs`）。`-0` は `normalize`（`num.rs:9`）で `+0` に直す。
- 診断の並びは `cdl_diag::sort`（`cdl-diag/src/lib.rs:88`）。(ファイル, 開始, 終了, コード) で安定ソート。キーが同じものは出した順で、出す順は BTree と走査順で決まる。
- ファジングの 15,000 件では、2 回目の出力との比較はしていない。適合テストのランナーが 2 回評価する（`crates/cdl-cli/tests/conformance.rs`。実行はステップ 5 の `cargo test`）。

確かめられなかった: スレッド数やアロケータによる差。メモリ枯渇（P5）の近くでの結果の一致。

## D. 資源上限

確かめた:
- 組み込みメソッド・スプレッド・`concat` の要素数は `check_len`（`eval.rs:408`）で、結果の長さを検査する。`map` の結果は元の配列と同じ長さ。
- テンプレートの結果・文字列・JSX の属性文字列は、完成した値を `string()` で検査する。
- import 経由: 同じファイルは 1 回だけ評価し、予算は全ファイルで共有（`cdl-eval/src/lib.rs:199-203`）。
- 値木の深さ・ノード数は、作ったときと文書の確定時に検査する。ホストのフック引数にも深さの検査がある（`lib.rs:255`）。

問題あり: P2・P4・P5。P6・P7 は推測。

## E. 境界

確かめた:
- `oxc` への参照は `crates/cdl-syntax/` の外にない（`grep -rn oxc crates` を `cdl-syntax` 以外で実行。`cdl-cli/tests/architecture.rs` の文字列を除く）。
- `std::fs`/`std::env`/`std::time`/`std::process`/`std::net` の使用は、`cdl-cli/src/{main,lib}.rs` だけ。
- 各クレートの `Cargo.toml` の依存を読んだ（`cdl-host` は `cdl-diag` と `cdl-eval` だけ）。`cdl-cli` の依存の検査は `scripts/check-rules.sh` の [6] が行う（実行はしていない）。
- `cdl-host` の公開物（`lib.rs`）は、下位クレートの型を `pub use` で再公開する（`Diagnostic`・`Schema`・`Ty`・`Value`・`Element` など）。「公開 API は cdl-host だけ」は、利用者が `cdl-host` だけに依存すれば足りるという意味で成り立つ。ただし再公開された型（`Value`・`Element`・`Ty`・`Field`）の定義は下位クレートにあり、互換性の約束の範囲に入るかは SPEC に書かれていない（HANDOFF の確認事項）。

## F. 仕様と実装の食い違い

SPEC 全文と実装の突き合わせはしていない（確かめられなかった）。確かめた範囲:

1. 構文エラーからの回復（SPEC §10）: 実装は最初の構文エラーで止まる。SPEC に実装の現状を注記した（今回の SPEC 変更）。
2. 評価ステップの数え方（SPEC §7「式を 1 つ評価するごと」）: `eval.rs:940` は、JSX の子の要素を `element()` で直接評価し、`step()` を数えない（`expr` を通る要素は数える）。属性文字列（`:856`）と JSX の子の `Fragment` も同様。ソースの大きさで有界なので、停止性には影響しない（**読んだだけ**）。
3. 診断の代表位置（SPEC §7「位置は…トップレベルの宣言」）: 評価の誤りの位置は宣言ではなく演算の位置になるものがある（DECISIONS D8 のとおり。SPEC に反映済み）。食い違いではない。
4. SPEC §12「clippy の `disallowed_methods` で乱数を禁止」: `clippy.toml` に乱数の項目はない（乱数のクレートが依存に入れないことで守る）。
5. `docs/PROTOTYPE-REPORT.md` §5 と、README の旧記述は、Kani を使うように読めた（README は今回直した。PROTOTYPE-REPORT は今回の変更範囲外で、更新していない）。

## 実行していないこと

Kani（使わない）、cargo-fuzz（使わない）、`clippy`/`fmt`（コードを変えていないので不要と判断）、`scripts/check-all.sh` 全体、ベンチ。
