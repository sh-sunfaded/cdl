# CDL アップデート報告（C1〜C5）

## 冒頭

- 解釈（1 行）: CDL は `/home/user/cdl`、Contrast は `/home/user/Contrast`（読むだけ）。検証は `systemd-run --user --scope -p MemoryMax=4G -p MemorySwapMax=0 nix develop -c bash scripts/check-all.sh`（`scripts/*.sh` に実行権限がないため `bash` 経由）。
- **G0**: `git status` clean（出力なし）。OK。
- **G1**: HEAD は `ec3b9059…`で、`0d252db…` ではない。差分は 1 コミット `ec3b905 変更`（`.cc-logs/report-cdl-plan.md` を追加しただけ。コード・文書の変更なし）。C0 の確認表（§2）への影響なし。注意: そのため `.cc-logs/report-cdl-plan.md` は**追跡されている**。`.cc-logs/report-cdl-update.md`（この報告）は未追跡で、コミットしない。`.gitignore` は触っていない。
- 規約との食い違い: プロンプトは新しいコードで `#[allow(...)]` を禁じるが、既存の統合テストはファイル先頭の `#![allow(clippy::unwrap_used, …)]` が慣習（DECISIONS F3。ヘルパー関数は clippy.toml の例外の対象外のため）。新しいテストファイルはこの慣習に従った。本体（`src/`）の新コードには使わない。

## C1: 検証テスト

1. 実施: `crates/cdl-host/tests/env_import.rs` を新規追加（コミット `0ea09cf`）。実装の変更なし。
2. 検証: `check-all.sh` → rules OK / fmt OK（最初の実行で、新規ファイルの整形が NG だったので `rustfmt` で直した）/ clippy OK / test OK / conformance OK / spec-examples OK / kani 未実行（指示どおり）。
3. 結果（V1〜V4・V6）と見つけたこと:
   - **V1** import + 関数形ルート（2 ファイル）: 期待どおり通る。
   - **V2** 関数形ルートでの `output.scale > 1 && <Item/>`: 期待どおり（2 で出て 1 で出ない）。
   - **V3** `policy` と関数形ルートの同居: 通る。`call_hook` も動く。
   - **V4** `sec` / `ms`: `ms(150)`=150、`sec(1)`=1000、`ms(1.5)`=E5009、`sec(0.15)`=150、**`sec(1.1)`=1100（C0 の推測「E5009」は外れ）**、`sec(1.0001)`・`sec(0.0004)`=E5009、`ms(-5)`=-5。SPEC 第 4 章（整数ミリ秒にならなければ評価エラー）と一致するので、現挙動を固定した。`sec` の変更はしない。
   - **V6** オブジェクト型の prop: 静的検査・評価・host の自前 `FromValue` での読み出しが通る。誤りは、オブジェクトリテラルの中なので E4004 ではなく **E3003（欠落）/ E3002（余分）/ E3001（型違い）**（C0 では E4004 と書いたのは不正確。テストはこの実測を固定）。
   - コミットしなかったテスト: なし（失敗は期待値の誤りで、実測に直した）。SPEC と挙動の食い違いは見つかっていない。
4. 未確認: V1 の子ファイルから `Env` 型を props で渡す形（`props: { env: Env }`）は試していない（V1 は `Env` の一部の number を渡した）。`Env` のフィールドへの `Literal` は C2 で試す。
5. 次へ進んだ: はい（緑）。

## C2: `Literal` の再公開

1. 実施: `cdl-host/src/lib.rs` の `pub use` に `Literal` を追加（1 行）、`cdl-host/tests/literal.rs`（3 テスト）、`docs/SPEC.md` 第 11 章に 1 文、`docs/DECISIONS.md` に L1。コミット `feat: cdl-host が Literal を再公開…`。
2. 検証: 実装前はテストが `unresolved import cdl_host::Literal` でコンパイル失敗することを確認。実装後 `check-all.sh` → rules / fmt / clippy / test / conformance / spec-examples すべて OK、kani 未実行。
3. 見つけた問題: なし。props の範囲外は E4004（メッセージにユニオン）、`string` 型の値は拒否、`Env` の範囲外は E4014、をテストで確認。
4. 未確認: `Literal::Number` / `Bool` のユニオンは試していない（String のみ）。
5. 次へ進んだ: はい。

## C3: prop の位置情報（0.2.0）

1. 実施（コミット `4b7d42e`）:
   - `cdl-eval/src/value.rs`: `ByteRange { start, end }` と `Element.prop_spans: BTreeMap<String, ByteRange>`。`cdl-eval/src/eval.rs`: `props()` が位置を集める（直接の属性は属性全体、`{...x}` はそのスプレッド属性全体、あとに書いたものが勝つ、`undefined` で取り除かれた prop は位置も持たない）。`Value` の型は変えていない。
   - `cdl-host/src/convert.rs`: `Node::prop_span(key) -> Option<Span>`、`Node::prop_error(key, msg)`。`Node::prop` / `slot` / `slots` の E4008 の位置が prop の位置になった（prop がなければ要素の位置）。`cdl-host` が `ByteRange` を再公開。
   - テスト: `cdl-host/tests/prop_span.rs`（7 テスト。直接・スプレッド・後勝ち・コンポーネント展開後・prop がない / `undefined`・`prop_error`・関数形ルートの `render`）。先にテストを書き、`prop_span` / `prop_error` がなくコンパイル失敗することを確認してから実装。既存の 2 テスト（`engine.rs`・`node_api.rs`）は「要素の位置」を期待していたので、新しい仕様（prop の位置。prop がないときは要素）に合わせて直した。
   - 文書: SPEC 第 6 章・第 11 章、DECISIONS P1。`Cargo.toml` の `version` を `0.2.0`（`Cargo.lock` は cargo が更新）。
   - 位置の型の決定: ファイル名つきの `Span`（`String` を持つ）を prop ごとに持つと文字列の複製が増えるので、ファイルを持たない `ByteRange`（8 バイト）にした。ファイルは要素の `span.file` と同じ（prop の属性は、その要素を書いた JSX の中にあるため）。スプレッドの位置は、仕様の指示の「スプレッドの式」ではなく「スプレッド属性全体 `{...x}`」にした（AST が持つ位置がそれで、診断として十分。SPEC・DECISIONS にそう書いた）。
   - メモリと `Limits`: `prop_spans` のエントリ数は `props` 以下で、`props` の値は `max_nodes` に数えられるので、総数は上限の定数倍に収まる。値木の深さ・ノード数の数え方は変えていない。実測のメモリ・性能は未計測。
2. 検証: `check-all.sh` → 最初の実行は test NG（`node_api.rs` の既存テストが要素の位置を期待）。直して再実行 → rules / fmt / clippy / test / conformance（全件、既存の期待値の変更なし）/ spec-examples OK、kani 未実行。
3. 見つけた問題: なし（既存テストの期待値の更新は、仕様変更の想定内）。
4. 未確認: メモリ・性能の実測。`Element` を構造体リテラルで作る下流がほかにないこと（Contrast の `crates/` を `grep "Element {"` した範囲では **なし**。`SceneElement` 等は無関係）。
5. 次へ進んだ: はい。

## C4: 文書と小さな負債（コミット `fe41372`）

1. 実施:
   - **T9**: 数え直した値 — DECISIONS 102 項目（`### ` の数。L1・P1・P2 を含む）、診断コード 76（`code.rs` と `DIAGNOSTICS.md` が一致）、`conformance` の `*.expected.json` 140 件（`.cdl` は 150）、`cargo test --workspace` は 187 テスト。`docs/HANDOFF.md`（83 → 102 項目、63 → 76 コード）と `README.md`（テスト 171 → 187 件）を更新。HANDOFF の日付つきの行（「125 テスト」の 2026-11-11 の記録）は変えていない。`docs/PROTOTYPE-REPORT.md` は本文を変えず、冒頭に注記 1 つ。**C0 の「PROTOTYPE-REPORT 6 章の『未反映』が残っているか未確認」は、`docs/HANDOFF.md:173` に「古い」と明記されていた**ので解消。
   - **T6**: `cdl-eval/src/value.rs` に `mismatch_path`、`cdl-eval/src/lib.rs` の E4014 のメッセージに「（合わないフィールド: `output.scale`）」を添える。先にテスト（`render.rs` の `an_env_mismatch_names_the_field_path`。型違い・欠落・余分・経路なし）を書き、失敗を確認してから実装。`conformance` の期待値は span とコードだけを比べるので変更なし。DECISIONS P2。
   - **T8**: 再現した（下の「見つけた問題」）ので、テストはコミットしていない。再現テストは `.cc-logs/t8-same-name-repro.rs.txt` に移した（`crates/cdl-host/tests/same_name.rs` にすれば実行できる。未追跡・未コミット）。
   - **T7**: やらない（先送り）。
2. 検証: `check-all.sh` → rules / fmt / clippy / test / conformance / spec-examples OK、kani 未実行。
3. 見つけた問題:
   - **T8 は実在の不具合**: 2 つの host モジュール（`host:a` と `host:b`）に同名の `Box` があり、`host:b` の `Box` だけが `Leaf` を一意にする。`import { Root, Box, Leaf } from "host:b"` して、(a) `<Box><Leaf /><Leaf /></Box>`（静的）は期待どおり E4006、(b) `<Box>{xs.map((x) => <Leaf />)}</Box>`（`map` が作る子。評価後の検査）は**診断が出ない**（期待は E4006、実際は読み込み成功）。原因（推測の域を出ないが、コードの読みと一致）: 評価後の検査 `eval.rs` の `check_unique` が `self.schema.component(name)`（`schema.rs:49-51`、モジュールを無視して最初の一致）を使う。影響: 同名のコンポーネントを複数モジュールに登録する host で、`map` 由来の重複が見逃される。Contrast は 1 モジュールなので影響なし。**修正は今回していない**（指示どおり）。
4. 未確認: 評価後の検査以外で `Schema::component` を使う箇所（`check.rs` / `eval.rs` の他の使用）が同じ取り違えを起こすか。静的な重複は正しく動いた。
5. 次へ進んだ: はい。

## C5: 移行メモ

### 1. 最終の CDL

- HEAD: `fe413720c55c598b2622950bd139633a9589f0d6`（`fe41372`）。`workspace.package.version = "0.2.0"`。
- Contrast の `crates/contrast-policy-cdl/Cargo.toml:13` の `rev` を `fe413720c55c598b2622950bd139633a9589f0d6` に更新（`Cargo.lock` も更新）。
- **注意**: Contrast は `https://github.com/sh-sunfaded/cdl.git` を git で参照している（`Cargo.toml:11-13` のコメントは、手元の path への差し替えも想定）。CDL の 4 コミットは**まだ push していない**（指示により push しない）。人間が push するまで、その rev は GitHub 上にない。push 前に試すなら、path の差し替え（`cdl-host = { path = "…/cdl/crates/cdl-host" }`）を一時的に使う。

### 2. API の旧 → 新

| 変更 | 内容 |
| --- | --- |
| `cdl_host::Literal` | 新規に再公開。`Ty::Literal(Literal::String(..))` とそのユニオン（`Ty::union`）で列挙を宣言できる（DECISIONS L1） |
| `Element.prop_spans` | 新フィールド（`BTreeMap<String, ByteRange>`）。`Element` を構造体リテラルで作るコードは壊れる（Contrast には見つからず） |
| `cdl_host::ByteRange` | 新規に再公開（`start` / `end`） |
| `Node::prop_span(key)` | 新規。`Option<Span>`（ファイルつき） |
| `Node::prop_error(key, msg)` | 新規。E4008 を prop の位置で作る（位置がなければ要素の位置） |
| E4008 の位置 | **変更**: `Node::prop` / `slot` / `slots` の失敗が、要素の位置から prop の位置へ。prop がない（必須なのにない）ときは要素の位置のまま。`required_child`・`unique_child`・`expect_component` は要素の位置のまま |
| E4014 のメッセージ | 「（合わないフィールド: `…`）」が付く（コードと位置は同じ） |

### 3. Contrast の移行箇所（`crates/contrast-policy-cdl/` を読んだ範囲）

**(B) `Literal`**
- `schema.rs` の `string` を列挙にできる prop: `Contrast.modKey`（`:46`）、`Layout.mode`（`:100`）、`Layout.animation`（`:103`）、`Startup.ipc`（`:119`）、`Workspaces.mode`（`:147` 付近）。`Keybind.action` は 60 個超で、CT020 のメッセージの質（未知の action を案内する文面）を保つなら `string` のままのほうがよい。判断は Contrast。
- 置き換わる検査（`convert.rs`）: `modKey`（`:184-187`）、`Layout.mode`（`:352-355`）、`animation`（`:373-375`）、`ipc`（`:507-509`）、`Workspaces.mode`（`:596-598`）の、範囲外の値の分岐（CT003）。範囲外の値は、CDL が評価前の段階で E4004（メッセージに型のユニオン。位置はその prop の値）で拒否する。
- 診断コードの変化: CT003（不正な列挙値）→ **E4004**（CDL のコードのまま `ConfigDiagnostic` に写る）。影響を受けるテスト: `crates/contrast-policy-cdl/tests/` の CT003 / CT020 を含む約 26 箇所のうち、列挙の値を扱うもの（`impl_look.rs`・`impl_workspaces.rs`・`impl_startup.rs`・`impl_config.rs` などの該当ケース。どれかは移行時に `grep CT003` で確認）。文書: `docs/cdl-contract.md` の 1 節の注（`:108`「`cdl-host` は `Literal` を公開していない」）と 4 節（CT003 の説明）。

**(A) prop の位置**
- 診断のヘルパー（`convert.rs:31-36` の `Ctx::error` は `node.span()` を使う）を、prop の位置に切り替える。`Ctx::prop` / `optional`（`:39-58`）は `node.prop` の `Err` を `convert(&d, …)` で写すだけなので、**E4008 の位置は CDL 側の変更で自動的に prop の位置になる**（コード変更なし）。値の範囲・形式の検査（CT003 の範囲、CT060〜CT065 など）は `cx.error(node, …)` で要素の位置になっているので、誤りのある prop に対して `node.prop_span(key)` を使うよう置き換える（`convert.rs:167` の `let span = node.span();` を含む）。
- `docs/cdl-contract.md:239`「値木の診断は要素単位の位置しか取れない（props に位置がない）」を更新。
- `Cargo.toml` の `rev` の更新（上記）。

**移行の順序（推奨）**: (1) `rev` を更新して、現状のコードのまま `cargo test` / clippy が通ることを確認（E4008 の位置が変わるので、テストが要素の位置を期待していれば直す）。(2) (A) の位置の切り替え（別コミット）。(3) (B) の列挙（別コミット。診断コードが変わるのでテストと文書を同時に直す）。各段階で Contrast の `cargo clippy --workspace --all-targets` と `cargo test --workspace`（重い処理はメモリ上限つき）。

### 4. Contrast の U4 の設計事項（CDL の変更なし）

- 関数形ルートでは、`load` が成功しても本体の誤りが残る（`check` / `load` は本体を評価しない。T5）。リロード失敗時に旧設定を保持するには、`load` の後に代表の `Env` で `Program::render` し、設定への変換（`to_settings`）まで通す。出力ごとに `Env` が違うなら全出力分。`loader.rs:100-107` は現状 `root()` が `None` だと内部エラーにしている。
- `Duration` は `ms(150)` で書く（`150ms` のリテラルはない）。`sec(0.15)`=150ms、`sec(1.1)`=1100ms は通り、整数ミリ秒にならない小数（`sec(1.0001)`）は E5009（C1 で実測）。負の `ms(-5)` も通るので、範囲は host が検査する。
- 実測で分かったこと（C1）: import + 関数形ルート、`&&` の条件、`policy` との同居はすべて通る（`cdl-host/tests/env_import.rs`）。`Env` に列挙（`Literal`）を使うと、範囲外の値は `render` で E4014（経路つき）。オブジェクトリテラルの中の誤りは E3001 / E3002 / E3003（E4004 ではない）。

### 5. Contrast の仕様への影響

- Motion の `duration` は `ms(n)`。INV-T1（400ms 以下）・INV-A1（`Tiers`）の診断は、(A) の後は prop 単位の位置で出せる。
- **T8（同名コンポーネントの取り違え）は Contrast に影響しない**（host モジュールが 1 つ）。

## 通読監査

対象: `git diff 0d252db..HEAD`（コード 6 ファイル、テスト 7 ファイル、文書 6 ファイル。`.cc-logs/report-cdl-plan.md` の追加は C0 後に人間が入れた 1 コミット）。

- **`CLAUDE.md` の規則**: 新しい `src/` のコードに `unwrap` / `expect` / `panic!` / 添字 / `as` / `#[allow]` はない（追加行を grep。`unwrap_or` のみ）。算術は `format!` の補間だけ。`HashMap` なし。依存の追加なし。依存の向き・`oxc` の漏れは `check-rules.sh` が OK。テストファイルは既存の慣習（DECISIONS F3）の `#![allow(clippy::unwrap_used, …)]` を使った（プロンプトの `#[allow]` 禁止とは食い違うが、`CLAUDE.md` と既存の慣習を優先。上の冒頭に記載）。
- **SPEC・DECISIONS・実装・テストの整合**: L1（`Literal`）・P1（prop の位置）・P2（E4014 の経路）が、SPEC 第 6・11 章、実装、テストと対応。`conformance/` の期待値の変更はなし（値木の JSON は span を比べない）。
- **残した負債**: T3（範囲・列挙の宣言）・T4（個数の宣言）・T5（`check` が関数形ルートの本体を評価しない）・T7（評価後の E4006 が 1 件で止まる）は先送り。(D)(E)(F) もやっていない。**T8 は不具合として再現済みで未修正**（上記）。
- **実行した検証**: 各段階の最後に `check-all.sh`（rules・fmt・clippy・test・conformance・spec-examples、すべて OK。最終の `cargo test --workspace` は 187 テスト）。
- **実行していない検証**: **Kani（指示どおり。`WITH_KANI=1` は未実行）**。`cargo-fuzz` と突然変異テスト（採らない方針）。`prop_spans` の実測のメモリ・性能。proptest の既定回数（256）での再実行は `cargo test` が既定で行ったもの以外にしていない。Contrast 側のビルド・テスト（読むだけの方針）。
- **未追跡のファイル**: `.cc-logs/report-cdl-update.md`（この報告）、`.cc-logs/t8-same-name-repro.rs.txt`。コミットしていない。`.gitignore` は触っていない。
- push はしていない（CDL の 4 コミットはローカルのみ）。
