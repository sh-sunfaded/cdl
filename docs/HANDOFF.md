# CDL 引き継ぎ書（Contrast への組み込みを担当する人へ）

2026-11-11 時点。確認したことだけを書く。推測は「推測」、確認できなかったことは「未確認」と書く。
細部は `docs/SPEC.md`（言語仕様）・`docs/DECISIONS.md`（実装の決定 83 項目）・`docs/DIAGNOSTICS.md`（診断コード表）・rustdoc を参照する。

## 1. 要約

- CDL は、Contrast（Rust + Smithay 製コンポジタ）の設定とポリシーを書くための、TSX のサブセットの静的型付き言語。設定は JSX 風の要素ツリー、判断は純関数（フック）で書く。
- 段階は**プロトタイプ**。目的は、決定性・停止性・型安全が実装で成り立つかの検証（CLAUDE.md）。
- できること: import を含む複数ファイルの読み込み、構文・名前解決・型検査・スキーマ適合・評価、値木の読み出し、ポリシーのフック呼び出し、`cdl check` / `cdl eval`。
- できないこと: `fmt`・`migrate`・LSP・パッケージ管理（対象外）。ホストの定数（DECISIONS X1）と edition 範囲の問い合わせ API（X2）。構文エラーからの回復（最初の構文エラーで止まる）。

## 2. 使い方（Contrast 側が知ること）

互換性を約束するのは `cdl-host` だけ（SPEC §11）。Contrast は `cdl-host` にだけ依存する。動く全体例は `examples/contrast.rs`（`cargo run -p cdl-host --example contrast`）。

### 2.1 公開 API（`crates/cdl-host/src/lib.rs`）

| 名前 | 役割 |
| --- | --- |
| `Engine` | 処理系。`Engine::new().with_schema(s).with_limits(l)`。`load(&dyn ModuleResolver, entry) -> Result<Program, Vec<Diagnostic>>`、`check`、`check_until(…, Stage)` |
| `Program` | 評価済みの不変な結果。`root() -> Node`、`document() -> &Element`、`files()`、`call_hook(hook, Value) -> Result<Option<Value>, Diagnostic>`。`Send + Sync`（テスト `program_is_send_and_sync`） |
| `ModuleResolver` | ソースを読む唯一の経路（trait） |
| `Source { id, text }` | リゾルバが返すソース |
| `Limits` | リソース上限（`Default` あり） |
| `Stage` | `Syntax` / `Sema` / `Eval` |
| `Schema` / `SchemaBuilder` / `ComponentDef` | スキーマ（cdl-sema からの再公開） |
| `Ty` / `Field` | 型の記述（props・ホストの型用） |
| `Value` / `Element` | 値木（cdl-eval からの再公開） |
| `Node` / `FromValue` / `Px` / `Millis` / `Rgba` | 値木を Rust の型で読む参照 API（`convert.rs`）。`derive` はない |
| `Diagnostic` / `diag` | 診断（`cdl-diag` の再公開） |

最小の使用例（`examples/contrast.rs` の `main` と `config` から。省略あり）:

```rust
let program = Engine::new().with_schema(schema).load(&Files, &entry)?; // Err は Vec<Diagnostic>
let root = program.root();
root.expect("Contrast")?;
for child in root.children() {
    if child.name() == "Keybind" {
        let keys: String = child.prop("keys")?;          // 必須。ないか型違いなら E4008
        let c: Option<Rgba> = child.optional_prop("focusColor")?;
    }
}
```

### 2.2 Schema ビルダー（`crates/cdl-sema/src/schema.rs`、DECISIONS E1）

スキーマは Rust 側のビルダーでだけ登録する（CDL 内の `declare` はない）。`examples/contrast.rs` の `schema()` が実例。

- `Schema::builder().root(module, "Contrast")` — ルート要素。`module` は `"host:contrast"` のように、CDL から `import … from "host:contrast"` する名前。
- `.component(module, ComponentDef::new("Name"))` — 組み込みコンポーネント。`ComponentDef` のメソッドは `prop(name, Ty)`（必須）・`optional_prop(name, Ty)`・`children(&["A","B"])`・`unique_child("A")` の 4 つ。
- `.type_def(module, "Policy", Ty)` — ホストの型。`Policy` はフックの集合で、フックは `Field::optional(Ty::Function(引数の型の Vec, Box<戻り値の型>))`。
- `.build() -> Result<Schema, Vec<String>>` — 誤りはここでまとめて返る。検査される内容（確認済み）: コンポーネント名は大文字で始める、props に関数型を使わない（B9）、名前の重複、子が同じモジュールにある、一意な子が子の一覧にある、ルートが存在する。
- 子に許す要素は、コンポーネント**名**の集合で指定する（同じモジュールのものだけ）。
- 一意子 `unique_child` は「その親の子の中に 1 つまで」。静的にわかる子は意味解析で、`map` などが作る子は評価後に E4006 になる（S3・E2）。
- 型を作る関数: `Ty::object([(name, Field::required(ty))…])`、`Ty::element(name)`、`Ty::union`、`Ty::Length` / `Duration` / `Color` / `String` / `Number` / `Boolean` など。
- **「識別 prop」に当たる機能はない**（`grep` で確認。DECISIONS C4: `key` は入れず、ホストは位置で差分を取る）。

### 2.3 ModuleResolver の実装

```rust
fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String>;
```

- `importer` はエントリを解決するときだけ `None`。`specifier` は `./x.cdl` / `../x.cdl` の形（検査済み。M1）。
- 同じファイルには**同じ `id`** を返すこと。`id` で同一性を判断し、同じ `id` は 1 回だけ構文解析・評価される。`id` は診断の `span.file` になる（E6）。
- `..` で範囲外へ出てよいかはリゾルバが決める（M1）。`examples/contrast.rs` の `Files` は `canonicalize` を使う。`cdl-cli` の `FsResolver` は字面の正規化だけで、シンボリックリンクは解決しない。
- 見つからなければ `Err(理由)` を返す。E2001 になる。
- import の書かれた場所ごとに呼ばれる。つまり同じファイルを何度も呼びうる。
- コアのクレートは `std::fs` を使わない。ファイルを読むのはリゾルバだけ。

### 2.4 Limits（既定値は暫定）

`Limits::default()`。上書きは `Limits { max_steps: …, ..Limits::default() }` と `Engine::with_limits`。

| フィールド | 既定 | 備考 |
| --- | --- | --- |
| `max_source_bytes` | 1 MiB | 1 ファイル。E5001 |
| `max_nesting` | 64 | 構文の入れ子。E1008 |
| `max_modules` | 256 | 開くファイルの数（エントリを含む）。E5013 |
| `max_value_depth` | 64 | 値木の深さ。E5011 |
| `max_steps` | 1,000,000 | Document 全体。E5002 |
| `max_call_depth` | 64 | E5003 |
| `max_eval_depth` | 128 | 式と要素の入れ子（呼び出しをまたぐ）。E5012 |
| `max_string_bytes` | 64 KiB | E5004 |
| `max_collection_len` | 65,536 | E5005 |
| `max_nodes` | 1,000,000 | 値木。E5006 |
| `max_hook_steps` | 10,000 | フック 1 回あたり（`cdl-eval/src/lib.rs` の `Default`） |

### 2.5 ポリシー（フック）

- CDL 側: `export const policy = { windowOpen: (w: WindowInfo): Placement => … } satisfies Policy;`（`examples/contrast/main.cdl`）。フックはすべて省略可能。`Policy` にないフック名は E4010。`policy` はエントリファイルにだけ置ける。
- Rust 側: `program.call_hook("windowOpen", Value::object([...]))`。
  - フックが未定義 → `Ok(None)`（ホストが既定の動作をとる）。
  - 引数がホストの型に合わない → `Err`（E4007）。
  - フック内の評価の誤り・上限超過（`max_hook_steps` など）→ `Err`（E5xxx）。位置は `policy` の名前（D8）。
  - `Err` のとき、ホストは既定の動作に戻す（`examples/contrast.rs` の `place`）。
- 戻り値の型は静的に検査されるが、実行時にも `conforms` で確かめているかは**未確認**。例は `Value` を自前でパターンマッチしている。
- `Program` は不変。リロードは、新しい `Program` の `load` が成功したら丸ごと差し替える（SPEC §8）。世代番号は Contrast 側（E5）。

### 2.6 診断

- コード帯（SPEC §10）: E0 字句 / E1 構文 / E2 名前解決・import / E3 型検査 / E4 スキーマ適合（ホストの組み込みと `Policy`）/ E5 評価。全 63 コードは `docs/DIAGNOSTICS.md`。コードの意味は変えない。
- `Diagnostic { code, severity, span: {file, start, end}, message, labels, help }`。`start`/`end` は元のソースのバイトオフセット。`Engine::load` の `Err` は (file, start, end, code) で整列済み。
- JSON: `cdl_host::diag::to_json(&[Diagnostic])` / `from_json`。形は SPEC §10 と DECISIONS F2。
- 人間向け: `cdl_host::diag::render_human(&diag, &dyn SourceMap)`。`SourceMap::source(file) -> Option<&str>` を実装する（`cdl-cli/src/lib.rs` の `Sources` が例）。ソースが見つからなければバイトオフセット表示にフォールバックする。
- 構文エラー（E1001）のメッセージは oxc 由来で英語（PROTOTYPE-REPORT §6）。

### 2.7 依存の仕方

- 全クレートが `version = "0.0.0"`、`publish = false`。git のタグは**ない**（`git tag` が空）。
- パス依存: `cdl-host = { path = "../cdl/crates/cdl-host" }`。
- git 依存を使うなら、タグがないので `rev = "<コミットハッシュ>"` で固定する。作業ツリーの状態は、この文書を書く前に未コミットの docs の変更（SPEC・README・AUDIT・本書）がある。固定するコミットは、それらをコミットしたあとにする。
- `rust-toolchain.toml` あり（rustc 1.99.1 で確認）。edition 2024。`oxc_*` は `=0.157.0` に固定されている。`Cargo.lock` はある。

## 3. 言語の要点（間違えやすい点）

- 先頭は `"use cdl 1";`。すべてのファイルに必須。形は厳密（C8）。末尾の `;` も必須（E1002）。
- 禁止（診断になる）: `let` `var` `class` `new` `this` `null` `for` `while` `try` `async` `enum` `as`、Hooks（`use` + 大文字）、`on` + 大文字のイベント属性、`ref`、`key`、分割代入、rest 引数、JSX のテキスト子、`c && <A />`（`conformance/forbidden/` に 19 件）。そのほか C1・C17 に一覧がある（関数呼び出しのスプレッド、ブロック内の関数宣言、`**`、ビット演算など）。
- **JSX のテキスト子は書けない**。`<Label>text</Label>` ではなく `<Label>{"text"}</Label>`（A1）。
- **`key` はない**（C4）。**条件描画は `c ? <A /> : undefined` だけ**（A2）。`if` / `?:` の条件は boolean だけ（S2）。`else` なしの `if` は書けない（A4）。
- 単位つきの値は関数で作る: `px(8)` `ms(120)` `sec(…)` `color("#89b4fa")`（`8px` のリテラルはない。G1）。Length ÷ Length はできない。比較は同じ単位どうし（B11）。`color()` の書式の誤りは評価時の E5010（B12）。
- 数値は `number`（binary64）だけ。`NaN` / `±Infinity` を生む演算と 0 除算は E5。`-0` は `+0` に正規化（D2）。
- 子の型付け: `Element<A | B>[]`。コンポーネントの戻り値は `Element<…>` に限る（フラグメント・`undefined` は返せない）。children は平坦化・`undefined` 除去済みの配列で渡る。`children` 属性と JSX の子の併記は E3012（B5・B6）。
- コンポーネント名は ASCII の大文字で始める。小文字のタグは E1011。コンポーネントを関数として呼べない（C5）。
- 関数・コンポーネントは、引数と戻り値の型注釈が必須。例外は、組み込みメソッドの引数に直接書くアロー関数（A3）。
- ユニオンは、リテラルどうし・`T | undefined`・`Element<A | B>` だけ（B2）。`undefined` を含む値は絞り込んでから使う（B14）。
- 再帰・相互再帰は E2005。`type` の再帰は E2007。参照グラフは DAG（B9）。名前の隠蔽（シャドーイング）は禁止（C6）。
- 配列のメソッドは `map` `filter` `reduce`（初期値必須）`includes` `concat` と `length` だけ。文字列と Record にはない。Record は `r["k"]` で参照し、結果は `T | undefined`（B15・B16）。
- テンプレート文字列に `boolean` は補間できない（PROTOTYPE-REPORT §2）。JSX の属性文字列はエスケープなし、`&` は E0008（C13・C18）。

## 4. 決まっていること・決まっていないこと

### 4.1 DECISIONS の要点

- 型: 不変コンテナは共変、関数型は不変（A6・B1）。型は構造的。ジェネリクスはユーザーに開放しない（B10）。
- E3 と E4 の境界: ホストの組み込みコンポーネントと `Policy` は E4、それ以外は E3（A5）。
- 段階: 全ファイルの構文 → import 解決 → 全ファイルの名前解決 → 依存先から型検査 → 評価。前の段階に誤りがあれば進まない（D5・M3）。評価の誤りは最初の 1 つで打ち切る（D9）。
- `cdl check` も評価まで行う（H3）。
- ホストの関数は扱わない。公開できるのは型・組み込みコンポーネント・（本実装で）定数（E4・X1）。
- 構文の入れ子の深さを `Limits` に足した（C10）。

### 4.2 SPEC の「付録 C」について

指示には「付録 C の未決事項」とあったが、`docs/SPEC.md` の付録は **A（設定例）と B（未決事項）だけ**で、付録 C はない。未決事項は付録 B にあり、`fmt` の規約と LSP の範囲・時期（プロトタイプ対象外）を除く 10 項目がすべて `[x]`（閉じた）。対応する決定: 子の型付け=B1・B5・B6、テキスト子=A1、`key`=C4、条件描画=A2、スキーマの定義元=E1、単位型=G1、TS 構文の範囲=C1、number=G2、edition 宣言=C8、診断コード表=F1。

### 4.3 仕様と実装・文書の食い違い（確認した範囲）

1. SPEC §10 は「パーサは同期点で回復し複数の誤りを報告」とするが、実装は最初の構文エラーで止まる（DECISIONS C11・C16。名前解決以降は複数報告する）。
2. SPEC §12 は構文解析の非パニックに cargo-fuzz を挙げるが、実装は proptest で代替（F4）。
3. SPEC §9・§13 の「ホストの定数」と「edition 範囲の問い合わせ API」は未実装（X1・X2）。
4. DECISIONS の決定は 2026-11-11 に SPEC.md へ反映した（H1・H3・S5・E4007/E4008・X1・X2・構文エラーの回復の現状・付録 B の扱い）。PROTOTYPE-REPORT §6 の「未反映」の記述は古い。
5. 参照ホストの定義が 2 か所で食い違う: `conformance/HOST.md` と `cdl-cli` の `contrast_schema()` は一意子が `Appearance` だけ。`examples/contrast.rs` の `schema()` は `Appearance` と `Animation`。
6. SPEC §11 の依存図に対し、実装は「どのクレートも `cdl-diag` に直接依存してよい」（DECISIONS H1。`architecture.rs` で検査）。
7. 上記以外は、確認した限りなし。ただし SPEC 全文と実装の突き合わせはしていない（**未確認**）。

## 5. 検証の状況

### 5.1 実行して確認した（`cargo test` は 2026-11-11、ほかは 2026-11-05）

| 検査 | 結果 |
| --- | --- |
| `scripts/check-all.sh`（kani 以外） | OK（2026-11-11。AUDIT 修正後）。`cargo test --workspace` は 125 テスト・失敗 0 |
| `cargo fmt --check` | OK |
| `cargo clippy --workspace --all-targets -- -D warnings` | OK |
| `scripts/check-rules.sh` | すべて OK |
| `scripts/check-conformance.sh` | すべて OK |
| `scripts/check-spec-examples.sh` | すべて OK |

適合テスト: `conformance/` 配下に `.cdl` が 115 ファイル（複数ファイルのテストの部品を含む）。ランナーは `crates/cdl-cli/tests/conformance.rs`。成功するケースを 2 回評価して一致を確かめる。

### 5.2 未実行

- **Kani（2026-11-11 に `cargo kani` を直接実行）**: 9 個のハーネスが成功し、`WITH_KANI=1 scripts/check-all.sh` もすべて OK。`lexer_never_panics`（`cdl-syntax/src/lex.rs`）は入力を 2 バイトに下げても時間切れで、実行対象から外した（証明できていない。proptest P1 とファジングで代替）。`nesting_reports_overflow` はハーネスの誤りを直して成功。詳細は PROTOTYPE-REPORT §5。
- **fuzz: 未実行**（使わない決定。F4）。
- `WITH_KANI=1 scripts/check-all.sh`: 2026-11-11 に実行し、すべて OK（終了コード 0）。`WITH_KANI` なしでは kani の行が「未実行」で、終了コードは 2。
- パフォーマンス（ベンチ）: 未実施。`benches/` はない。

### 5.3 機械で確かめていない主張と、支えるテスト

| 主張 | 支えるテスト（部分的） | 備考 |
| --- | --- | --- |
| パニックしない | `cdl-syntax/tests/proptest_syntax.rs` の P1（任意入力）。型検査・評価は P3（`cdl-eval/tests/props.rs`）の型付き式のみ | oxc の経路は proptest だけ。評価・型検査に任意バイト列を入れるテストは**未確認** |
| 停止する | `cdl-eval/tests/props.rs` の `small_limits_always_stop`、`cdl-eval/tests/eval.rs` と `cdl-host/tests/engine.rs` の上限超過テスト | 参照グラフの DAG と再帰型の禁止（E2005・E2007）が停止性の根拠だが、これを直接見るテストの所在は**未確認**。形式的な証明（Kani）は**ない** |
| 決定的 | `evaluation_is_deterministic`（props.rs）、`loading_is_deterministic`（engine.rs）、適合ランナーの 2 回評価 | |
| 資源上限が効く | 上記の上限テスト。入れ子の上限は `cdl-syntax/tests/parse.rs` の `nesting_at_the_limit_does_not_overflow_the_stack` ほか、`cdl-sema/tests/check.rs` の `deep_expressions_at_the_nesting_limit_do_not_overflow` | 評価の再帰（呼び出しの深さ）がスタックを使い切らないかを直接見るテストは**未確認** |
| 型検査を通れば評価で型エラーにならない | `typed_expressions_evaluate_to_values_of_their_type`（E5000 が出ない） | 生成できる式の範囲に限る |

### 5.4 最終精査

`docs/AUDIT.md`（2026-11-11）。精査で見つかった重大な問題 5 件（P1〜P5: 構文の前置キーワードの連鎖・型の大きさ・評価の再帰・共有値の深さ検査・メモリの総量）は、同日修正し、各々に適合テストを足した（対処状況の表）。P6・P7 も対処済み（`Limits::max_modules`＝既定 256・E5013、E4008 のメッセージ）。`check.rs`・`convert.rs` の全行精読はしておらず、再帰の深さの確認（22 種類の構文を入れ子の上限ぎりぎりで実行）と、極端な `Limits` のテストで代えた（フックの `max_hook_steps` を 0 にした呼び出しは未確認）。新しい上限: 型の深さ 128・ノード数 4,096（固定。E3013）、評価の再帰 128（`Limits::max_eval_depth`。E5012）。既定値は暫定。

## 6. 既知の制限とはまりやすい点

- **oxc_parser はプロトタイプ限り**（`=0.157.0` 固定）。`oxc_*` の型は `cdl-syntax` の外に出ない（`api_boundary.rs` で検査）。置き換えには、字句解析器・再帰下降パーサ・誤りからの回復・入れ子の深さの自前カウントが要る。見積もりは PROTOTYPE-REPORT §4（約 3,000〜3,500 行、2〜3 週間。見積もりであって実測ではない）。
- 構文エラーは 1 件で止まる。字句の誤りが 1 つでもあれば oxc に渡さない（C16）。E1001 のメッセージは英語で、span は oxc 依存（適合テストに入れていない）。
- 入れ子の深さの前走査は近似（C10 は 2 回改訂された）。
- 大きな入力・実際の設定ファイルの規模での性能は**未検証**。既定の上限が現実の設定に適切かも未検証（「暫定」）。
- `Program::call_hook` は、呼び出しごとに globals を複製する実装（`cdl-eval/src/lib.rs` の `call_hook`）。頻繁に呼ぶ場合のコストは**未計測**。
- `cdl-cli` は `host:contrast` のスキーマに固定（H2）。他のスキーマは Rust から使う。
- 診断の質で気づいた点: E1001 が英語、ソースが `SourceMap` にないと抜粋が出ない。それ以外の質の評価は**未確認**。
- 窮屈な点（仕様どおり）: `boolean` を補間できない、文字列に `length` がない、リテラル型どうしの比較 `1 === 2` が E3006、Length ÷ Length ができない。

## 7. 開発の作法（CLAUDE.md の要点）

```
crates/
  cdl-diag/    診断型・コード表・人間向け/JSON のレンダリング
  cdl-syntax/  ソース → CDL AST（oxc_parser は内部だけ。lex.rs が自前の字句検査）
  cdl-sema/    名前解決・型検査・スキーマ適合・Schema ビルダー
  cdl-eval/    評価器・リソース上限・フック呼び出し
  cdl-host/    公開 API（Engine / ModuleResolver / Limits / Program / Node）と import の解決
  cdl-cli/     cdl check / eval、FsResolver、参照ホスト
conformance/   節番号ごとの .cdl と .expected.json、forbidden/、HOST.md、README.md
docs/          SPEC.md, DECISIONS.md, DIAGNOSTICS.md, PROTOTYPE-REPORT.md, HANDOFF.md
examples/      contrast.rs（cdl-host の example）, contrast/main.cdl, contrast/theme.cdl
scripts/       check-all.sh, check-rules.sh, check-conformance.sh, check-spec-examples.sh, verify.sh
```

- 依存は `diag ← syntax ← sema ← eval ← host ← cli` の一方向。どのクレートも `cdl-diag` には直接依存してよい（H1）。`cdl-cli/tests/architecture.rs` が検査する。許可済みの外部依存は `oxc_*`（syntax のみ）、`serde` / `serde_json`、dev の `proptest`。足す前に確認する。
- `#![forbid(unsafe_code)]`。clippy で `unwrap_used` `expect_used` `panic` `indexing_slicing` `as_conversions` `arithmetic_side_effects` を deny（テストでは `clippy.toml` で緩める。F3）。公開 API は `Result` で診断を返す。
- 値木に `HashMap` を使わない（`BTreeMap`）。syntax / sema / eval は `std::fs` `std::env` `std::time` に触れない。
- conformance: `conformance/<節>/<名前>.cdl` + `.expected.json`。禁止構文は `forbidden/<id>.cdl`。仕様を変える実装は、先にテストを足す。
- 変更前: `scripts/check-all.sh`（`WITH_KANI=1` で Kani も）。結果は OK / NG / 未実行。未実行が残るあいだは「完成」と報告しない。
- 仕様が沈黙している点は、選択肢を挙げて質問し、決まったら DECISIONS に 1 項目で記録する。`rm`・`git reset --hard`・`git push --force`・上書きは 1 件ずつ承認を得る。説明・コミット本文・文書は日本語、識別子と `feat:` などの種別は英語。

## 8. 次にやるべきこと（最初に決める点の候補）

1. **スキーマの置き場所と形**: 現状はビルダーを Rust で呼ぶだけで、スキーマをファイルから読む形式はない（H2）。Contrast 側のコードに置くか、Contrast のリポジトリで管理する（SPEC §11 は Contrast 側の仕様を Contrast のリポジトリで管理すると定める）かを決める。参照ホストの 3 か所の食い違い（4.3 の 5）も解消する。
2. **識別 prop と一意子**: 一意子は `unique_child(名前)` として実装済みだが、識別 prop（要素の同一性を決める prop）に当たる API はなく、`key` も入れない決定（C4）。リロードで差分を取るなら、位置ベースで足りるか、識別 prop を SPEC の変更（修正）として入れるかを決める。入れるなら、先に `conformance/` のテストを足す。
3. **変換層**: いまは `Node::prop` で手書きに読む（E7 を E8 で改訂）。型付きの変換（`derive` など）は未実装。スキーマと変換コードの食い違いは E4008 になる（実行時）。スキーマから変換を作るかを決める。
4. **リロード**: `Program` は不変・`Send + Sync`。差し替え（世代番号・失敗時に旧設定を残す）は Contrast 側（E5）。`load` が失敗したときの診断の通知先（ログか画面か）を決める。ファイル監視と、import 先の変更検知は `Program::files()` が使えるが、監視の実装自体は**未確認**（CDL 側にはない）。
5. **フックの追加**: Contrast の `Policy` 型（`windowOpen` 以外）と、フックごとの `max_hook_steps` の妥当性。現状は全フック共通の 1 値。
6. **AUDIT.md の残り**（前走査が数えない構文がほかにないか、実際の設定ファイルの規模での既定値の確認）。Kani は使わない方針（SPEC 第 12 章）。
7. docs の変更のコミットと、固定するコミットの決定（2.7）。
