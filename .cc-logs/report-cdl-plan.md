# C0 報告: CDL 破壊的アップデートの現状確認と計画

2026-11-14 作成。CDL・Contrast とも、コード・テスト・文書は変更していない（書いたのはこのファイルだけ）。
「確認した」は、ファイルを読んだ箇所（`ファイル:行`）を指す。**ビルド・テスト・CLI は 1 回も実行していない**（8 章）。

## 1. 解釈とゲートの結果

解釈（1 行）: リポジトリの場所はプロンプトの `~/RustroverProjects/*` ではなく、実在する `/home/user/cdl`（CDL）と `/home/user/Contrast`（Contrast、読むだけ）として読んだ。

| ゲート | 結果 |
| --- | --- |
| CDL の `git status` | clean（出力なし） |
| CDL の HEAD | `0d252db2bc7d477f2e2bf1d85180dd20b0e09a74`（2026-11-12 13:36 +0900、`docs: README を完成版にする…`）。タグなし |
| Contrast が参照する CDL の rev | `0d252db2bc7d477f2e2bf1d85180dd20b0e09a74`（`/home/user/Contrast/crates/contrast-policy-cdl/Cargo.toml:13`、git 指定）。**CDL の HEAD と同じ**。つまり「2026-11-12 の小さな変更」は、この rev にすでに含まれる。調査の rev は古くない |
| 規約 | `/home/user/cdl/CLAUDE.md` のみ（`AGENTS.md`・`CONTRIBUTING.md` は無い）。プロンプトと矛盾する点なし。ただし CLAUDE.md の「`scripts/check-all.sh` で OK/NG/未実行」は、今回コードを変えないので実行していない |
| `.cc-logs/` | CDL に元々無く、作成した。`.gitignore` は `/target` だけ（`/home/user/cdl/.gitignore`）なので、**`.cc-logs/` は ignore されず、`git status` に未追跡として出る**（`.gitignore` は触っていない） |
| Contrast の `git status` | 出力を最後まで確認していない（読むだけの作業のため）。HEAD は `2d03b02 変更` |

見つからなかったもの: プロンプトが出所とする `ui-spec.md`・`cdl-requests.md` は、ファイル名の検索（`find /`）で見つからなかった。Contrast 側で読めたのは `docs/cdl-contract.md`、`docs/ui-breaking-update.md`、`crates/contrast-policy-cdl/src/*`、`config/`・`examples/`。

## 2. 現行 rev での確認（5.1）

| 必要なもの | 調査の記述 | 現行（`0d252db`）の確認 |
| --- | --- | --- |
| 順序付き列挙 | `Ty::Literal` はあるが `Literal` を再公開していない | **成り立つ。** `cdl-host/src/lib.rs:21-22` は `ComponentDef, Schema, SchemaBuilder` と `Field, Ty` だけを `pub use`。`Literal`（`cdl-sema/src/types.rs:29-33`）は host から名前を書けず、`Ty::Literal(..)` を作れない。`Ty::union` は公開だが、リテラルのメンバーが作れない。Contrast は実際に `modKey` / `action` / `mode` / `animation` / `ipc` を `string` にして変換で検査している（`contrast-policy-cdl/src/schema.rs:1-3,50,102,128`、`cdl-contract.md:108`）。**「順序」は CDL に無い**: ユニオンは正規化された集合（`types.rs:15`、`Ty::union`）で、`<` などの比較もリテラルには使えない（SPEC 168）。順序は host が持つしかない |
| 範囲検査 | `number` / `Length` / `Duration` に範囲の宣言がない | **成り立つ。** `ComponentDef::prop(name, Ty)` / `optional_prop` は型だけを受ける（`cdl-sema/src/schema.rs:86-98`）。`Field` は `ty` と `optional` だけ（`types.rs:41-45`）。範囲を持つ型は SPEC 第 4 章にもない |
| 単一子 | `unique_child` は「1 つまで」、「ちょうど 1 つ」は `required_child`（評価後。E4008） | **成り立つ。** `schema.rs:114-118` は `unique: BTreeSet<String>`（1 つまで）。「ちょうど 1 つ」はスキーマで宣言できず、host の `Node::required_child`（`cdl-host/src/convert.rs:258-267`）が、無ければ**要素の位置**で E4008。**1 つの親に複数の `unique_child` を宣言できる**（`BTreeSet` なので何個でも）。Contrast は `Contrast` に 9 個宣言している（`schema.rs:51-57,66-74`）。制約は、`unique ⊆ children`（`schema.rs:221-227`） |
| 条件要素 | `c && <A />`、`c ? <A /> : undefined`。`false`・`undefined` の子は除かれる | **成り立つ（仕様と DECISIONS）。** SPEC 166・186、DECISIONS R2（`DECISIONS.md:551-`）。テストは `conformance/5/and-render.cdl`、`cdl-eval/tests/eval.rs`、`cdl-sema/tests/check.rs` に `&& <` がある。左辺は `boolean` だけ（それ以外は E3006）。**評価器の実装そのもの（`eval.rs`）は精読していない** |
| `Env` | `SchemaBuilder::env`、`Program::render(&env)`、`is_function_root()`、型違いは E4014 | **成り立つ。** `schema.rs:149-166`、`cdl-host/src/lib.rs:198-217`、`cdl-eval/src/lib.rs:190-205`（型は `conforms` で検査。E4014 の位置はルートの式で、メッセージは型全体を言うだけでフィールドを指さない）。**非対称**: `load` / `check` は関数の形のルートの**本体を評価しない**（SPEC 239、`cdl-cli/src/lib.rs:258-262` は `check` が `check_until` だけ）。本体の評価の誤り（0 除算・上限・key 重複・値の変換）は `render` のときに初めて出る |
| 診断の位置 | 値の診断は要素単位。props に位置が無い | **半分だけ成り立つ。** 値木の `Element` は `span` と `expanded_from` しか持たない（`cdl-eval/src/value.rs:35-47`）。`Node::error` は要素の位置で E4008（`convert.rs:276-278`）。**ただし評価前の静的検査は prop の位置を持つ**: E4003 は prop 名の位置（`cdl-sema/src/check.rs:3035-3053`）、E4004 は値の式の位置（`check.rs:3066-3076`）。Contrast の契約 4 節の記述は、値木の診断（Contrast の CT003 など）についてだけ正しい |

## 3. Q-C1〜C5 への回答

### Q-C1: `Duration` のリテラルと読み出し

- **リテラルはない。** `150ms` のような書式は存在せず、`ms(150)` / `sec(0.15)` の関数で作る（SPEC 80・106、`cdl-eval/src/eval.rs:1276-1284`）。構文を足さないのは G1 の決定済みの方針。
- `ms(n)`: `n` が整数で、`|n| ≤ MAX_SAFE` のとき成功。小数は E5009。`sec(n)`: `n * 1000.0` が整数になるときだけ成功（`eval.rs:1283`）。**浮動小数の罠（推測。未実行）**: `sec(1.1)` は `1.1 * 1000.0 = 1100.0000000000002` になり、E5009 になるはず。`sec(0.15)` が通るかも、丸め次第で未確認。検証の入力は 9 章 V4 に書いた。
- host の読み出し: `Millis(pub i64)`（`convert.rs:15-17`）。`prop::<Millis>("duration")` / `prop::<Option<Millis>>`。負の値も通る（範囲なし）。「host が使いやすいか」: 型としては十分（`std::time::Duration` への変換は 1 行）。**足りないのは範囲**（Contrast の INV-T1「Motion は 400ms 以下」は host の変換で検査）。これは Q-C1 というより負債 T3（5.3）。

### Q-C2: 未知の子・重複する子

- 未知の子（スキーマの `children` にない組み込み要素）: **E4005**（`check.rs:3124-3131`。位置はその子）。テキスト: E3014。
- 重複する一意な子: **E4006**。静的に数えられるものは意味解析で、2 つ目の位置にラベル「最初の子」つき（`check.rs:3136-3150`）。`map` が作る子は評価後に検査し、**最初の 1 件で打ち切る**（`eval.rs:1055-1073` の `return Err(e)`）。`key` の重複（E4011）は全件報告する（U3）ので、扱いが不揃い（負債 T7）。
- 1 つの親に複数の `unique_child`: **宣言できる**（上記）。一意かどうかは**コンポーネント名**で数える（`Tiers` の中の `FocusTier` のように、親ごとに独立）。
- 宣言できないもの: 「ちょうど 1 つ」「n 個まで」「0 個か 1 個」（Contrast の `Tier`）。いずれも host の変換が、要素の位置で検査している。

### Q-C3: `Env` ルート（関数の形）と import・複数ファイル・`when`

**実行して確かめていない。** 読んだ範囲で分かること／分からないことを分ける。

(a) `import` との両立:
- 関数の形のルートに課される制約は、エントリの `export default` だけ（`check.rs:1429-1489`）。引数は `Env` と完全一致（`*t != env` → E4012）、戻り値は `Element<ルート>`。import は `export default` を持てない（E2011。SPEC 440）ので、**ルートが 2 つになる衝突は起きない**。
- `Env` は host モジュールの型なので、どのファイルでも `import type { Env } from "host:contrast"` で取り込める（SPEC 235）。
- 読み込み時に評価されるのはトップレベルの `const`（import 先を含む）で、`Env` を知らない（SPEC 239）。`Env` 依存の値は、関数本体の中か、`Env` の一部を props で渡したコンポーネントの中でしか作れない。
- **テストの裏付けは無い**: `cdl-host/tests/render.rs` は 1 ファイル（`main.cdl`）だけで、`Memory` リゾルバは複数ファイルを扱えるのに import を試していない。

(b) 複数ファイルの設定（Contrast の `config/config.cdl` の形）:
- Contrast の入口は、`import { appearance, tiers, … } from "./appearance.cdl"` で**要素の定数**を取り込み、`<Contrast>{appearance}{tiers}…</Contrast>` に置く（`/home/user/Contrast/config/config.cdl:3-8,32-41`）。`export const policy`（フック）も同じ入口にある。
- 静的な形のルートのままなら、**何も壊れない**（SPEC 238: 静的な形では `Env` は無視される）。関数の形にするのは、`Env` が要る部分だけでよい（`const` の要素定数はそのまま使える）。`Env` を使う子ファイルの部品は、`function Part(props: { env: Env }): Element<…>` のように **props で受ける形になる**（推測。`Env` 型の props がユーザー定義コンポーネントで書けるかは未確認）。
- `export const policy` と関数の形の `export default` の同居は、SPEC 上は禁止されていない（第 8 章は `export default` の形に触れない）。**未確認**。

(c) `when`（条件）: CDL に `when` という名前の構文・コンポーネントは**無い**。一番近いのは `c && <A />` / `c ? <A /> : undefined`（R2）で、関数の形のルートでもふつうの式として書ける。`c` が `boolean` でなければ E3006（`undefined` を含む `Env` のフィールドは先に絞り込む）。関数の形のルートの中で `env.x === "…" && <A />` が通るかは、**テストされていない**。検証の入力は 9 章 V1〜V3。

**load と render の非対称（Contrast への影響が大きい）**: 関数の形のルートでは、`load` が成功しても本体の誤りが残る。Contrast の「リロード失敗なら旧版を保持」（`cdl-contract.md` 3 節）を守るには、`load` のあとに**代表の `Env` で 1 回 `render`** して、`to_settings` まで通す設計が要る（出力ごとに別の `Env` になるなら、全出力分）。これは CDL の変更なしでできるが、Contrast 側の設計事項（`loader.rs:100-107` は現状 `root()` が `None` なら内部エラー）。

### Q-C4: 配列・レコードの値をスキーマで表せるか

- 型としては**表せる**: `Ty::Array` / `Tuple` / `Record` / `Object`（`types.rs:8-26`）。host は `Ty` の公開バリアントで組み立てられる。props の型に使えない制約は「関数型を含む」だけ（`schema.rs:130-138,180-190`）。
- テストで裏付けがあるのは、`Array<string>`（`cdl-host/tests/node_api.rs:43`）、`Array<Element>`（`slots.rs:34`）、オブジェクト型の `Env` とフック引数（`render.rs:19-27`）。**オブジェクト・`Record` を host コンポーネントの prop の型にした例はテストにない**（未確認）。
- host の読み出し: `FromValue` は `bool` / `f64` / `String` / `Px` / `Millis` / `Rgba` / `Option<T>` / `Vec<T>` だけ（`convert.rs:33-137`）。オブジェクト・`Record`・タプルの実装はないので、必要なら host が自前で `FromValue` を実装する（trait は公開、`Value::Object(Arc<BTreeMap>)` を match する。`Value` は公開）。
- **表せないもの**: リテラルのユニオン（`Literal` 非公開）、範囲・非空・長さの制約、オブジェクト型どうしのユニオン（SPEC 119。異種の要素のリストは**型付きの子要素**で表す）、`Record` のキーの制約。
- シーン宣言の「アプリ起動リスト」: 同種の要素なら `Array<{command: string; …}>` で足りる。異種や分岐があるなら、要素（`<App … />`）の子にするほうが CDL の流儀に合い、現状の機能で書ける。

### Q-C5: `Env` の型としてのレコードの表現力

- `Env` は `Ty::Object`（それ以外は `SchemaBuilder::env` が拒否。`schema.rs:155-160`）。**入れ子のオブジェクト、配列、タプル、`Record`、省略可能なフィールド（`Field::optional`）、`Length` / `Duration` / `Color`** が使える（検査は `conforms`、`value.rs:164-195`。`render.rs:19-27,67-75` が入れ子のオブジェクトで動いている）。例: `{ workspaces: { name: string; kind: string; focused: boolean }[]; output: {…} }` は書ける。
- **使えない**: 関数型（`schema.rs:150-154`）、リテラルのユニオン（`kind: "main" | "scratch"` が書けず、`string` にして `switch` に `default` が要る）、オブジェクトでない `Env`。
- `Env` の値の深さは `max_value_depth`（既定 64）で制限（E5011。`cdl-eval/src/lib.rs:192-198`）。配列の長さの上限が `Env` に効くかは未確認。
- E4014 のメッセージが型全体を言うだけで、どのフィールドが合わないかを言わない（`lib.rs:202-205`）。`Env` が大きいと原因が探しにくい（負債 T6）。

## 4. 技術的負債の棚卸し（5.3）

実在を確認したものだけ。費用は 小・中・大。

| ID | 項目 | 場所 | 影響 | 破壊的か | 費用 | 直さないと残るもの |
| --- | --- | --- | --- | --- | --- | --- |
| T1 | `Literal` が host から使えない | `cdl-host/src/lib.rs:21-22` | host が列挙を `string` + 自前検査にする。Contrast は 5 つの prop（`modKey` `mode` `animation` `ipc` `action`）で回避中。CDL の診断（型のユニオンを示す E4004）の代わりに host 独自のコード（CT003/CT020）が出る。`Env` の `kind` のような列挙も書けない | 非破壊（再公開のみ） | 小 | 回避策と host 独自コードが増え続ける。`Env`（U4）の型が文字列になる |
| T2 | 値木に prop の位置がない | `cdl-eval/src/value.rs:35-47`、`cdl-host/src/convert.rs:276-278` | host の値の検査（範囲・形式・相互関係）の診断が要素単位。Contrast は「メッセージに prop 名を書く」で回避（`cdl-contract.md:239`）。`Tiers` / `Motion` の検査が増えるほど効く | `Element` にフィールドを足すので、構造体リテラルで `Element` を作る host だけ壊れる（Contrast は作っていない。未確認）。`Node` の API は追加のみ | 中 | 位置が要素単位のまま。診断の質が host の文面頼み |
| T3 | 範囲・列挙・個数の宣言口がスキーマにない | `cdl-sema/src/schema.rs:86-118` | Contrast は `borderWidth` 0〜256、`l` ∈ (0,1]、`Workspaces.count` 1〜32、Motion ≤ 400ms などを全部 host の変換で検査（CT003 / CT010 / CT060 など。`cdl-contract.md:219-231`）。動作はしている | 型システムの拡張なので、入れるなら SPEC 第 4 章の変更（edition 内の追加扱いも可） | 大 | host ごとに同じ検査を書く。ただし T2 が済めば、host の検査は prop の位置を指せるので痛みが減る |
| T4 | 「ちょうど 1 つ」「0 か 1 個」を宣言できない | `schema.rs:114-118`、`convert.rs:258-267` | `FocusTier`（ちょうど 1 つ）を CT061、`Tier`（0〜1）を CT064 として host が検査。位置は要素単位 | 非破壊（追加） | 小〜中 | host の検査が残る。動作に問題はない |
| T5 | `check` が関数の形のルートの本体を評価しない | `cdl-cli/src/lib.rs:258-262`、SPEC 239 | `cdl check` が通っても `render` で E5xxx / E4011 / 変換の誤りが出る。Contrast のリロード（旧版保持）は代表の `Env` での `render` が必要 | 非破壊（追加の API / CLI オプションなら） | 小 | 関数の形を使う設定（U4）で「check は OK なのに render が失敗」が起きる |
| T6 | E4014 のメッセージが、合わないフィールドを指さない | `cdl-eval/src/lib.rs:202-205` | `Env` が大きくなると、host 側の不具合の原因が探しにくい | 非破壊（メッセージだけ。コードの意味は変えない） | 小 | 診断の質 |
| T7 | 評価後の E4006 は 1 件で打ち切り、E4011 は全件 | `eval.rs:1055-1073` と U3（`DECISIONS.md:605-`） | 一度に直せる件数が不揃い | 非破壊 | 小 | 軽微な不揃い |
| T8 | `Schema::component(name)` がモジュールを無視して最初の一致を返す | `schema.rs:48-51`、`eval.rs:1055` が `self.schema.component(name)` を使用 | 2 つの host モジュールに同名のコンポーネントがあると、取り違える**疑い**。Contrast は 1 モジュールなので顕在化していない。**実害の再現は未確認** | 非破壊 | 小 | 複数モジュールの host で将来困る可能性 |
| T9 | 古い文書の数字 | `docs/PROTOTYPE-REPORT.md:3,10-24,129`: 「2026-11-05 時点」「83 項目」「63 コード」「109 件」。実際は DECISIONS 99 項目（`### ` の数）、コード 76（`DIAGNOSTICS.md` の行数 = `code.rs` の定義数 = 76、一致）、`conformance/**/*.expected.json` 140 件 | 報告書が日付つきなので誤りとは言い切れないが、HANDOFF / README と数字がずれる。6 章「SPEC への反映が未承認」の項目が今も残っているかは**未確認** | 非破壊 | 小 | 読む人が現状を取り違える |
| T10 | 検証の穴（未確認の主張） | `docs/HANDOFF.md:119,176,198,204-207,219,222`、`docs/AUDIT.md:36,68`。Kani の K1（字句検査の非パニック）は時間切れで未証明（`PROTOTYPE-REPORT.md:84-`）。`Env` + import の複数ファイルのテストなし（`render.rs`）。性能・実設定の規模は未検証 | 主張の一部が「テストで守られている」とまだ言えない | 非破壊 | テストの追加は小〜中 | 主張の信頼度が低いまま。K1 は CLAUDE.md の範囲外（今回は Kani を使わない） |

診断コードの欠番・重複: `DIAGNOSTICS.md`（76 行）と `code.rs`（76 定義）は件数が一致。欠番は E1009（廃止と明記）だけ。重複は見つけていない（コードの突き合わせは件数のみ。1 件ずつは未確認）。

Contrast 側の文書のずれ（CDL の負債ではない。参考）: Contrast の `schema.rs` には `Keybind` の `cooldown` / `focus`（`schema.rs:150-151`）があるが、`cdl-contract.md:68` の表にない。

## 5. 破壊的アップデートの案（5.4）

方針だけを根拠にした変更はしない。Contrast の `Motion` / `Tiers` / `Env` と `when` / シーン宣言が実際に必要とするものだけを残した。

### (A) prop の位置を `Node` から取れるようにする【推奨: やる】

1. **SPEC**: 第 6 章「ドキュメントモデル」に 1 文: 「各ノードは、props ごとの位置（属性の名前から値まで。スプレッドから来た prop はスプレッドの式）も保持する」。第 10 章の E4008 の説明に「host は prop の位置で報告できる」。
2. **cdl-host API**: `Element` に `prop_spans: BTreeMap<String, Span>`（案。ファイル名は要素の `span.file` と同じなので、`(start, end)` だけ持つ形が軽い）。`Node::prop_span(key) -> Option<Span>` と `Node::prop_error(key, message) -> Diagnostic`（E4008、prop の位置）を追加。**`Node::prop::<T>` が失敗したときの E4008 の位置を、要素から prop の位置に変える**（既存の挙動の変更）。旧 → 新: 位置が要素 → prop。
3. **診断コード**: 追加なし（E4008 の位置だけが変わる）。
4. **Contrast の移行**: `crates/contrast-policy-cdl/src/convert.rs` の診断ヘルパー（`:34-35` の `at(node.span(), …)`、`:39-50` の `prop` / `optional`）を prop の位置に切り替える。メッセージの prop 名はそのまま残せる。`cdl-contract.md:239` の記述を更新。
5. **テスト**: `cdl-host/tests/node_api.rs` に、E4008 の位置が prop の属性を指すテスト（直接書いた prop・スプレッド・コンポーネント展開後の 3 通り）。値木の JSON の比較は span を見ない（SPEC 543）ので、`conformance/` の既存の期待値は変わらない。先に追加してから実装（CLAUDE.md）。
6. **破壊的か**: `Element` の構造体リテラルを使う host だけ壊れる。実質ほぼ非破壊。0.x なので `0.2.0` に上げる（`Cargo.toml:6` の `workspace.package.version`）。
7. **推奨**: やる。理由: Contrast の `Tiers`（CT060〜CT065）・`Motion`（400ms 上限）・`Layout` の範囲検査がすべて要素単位の位置で、増える一方。範囲の宣言（T3）より小さい変更で、同じ痛みの大半を消せる。位置の持ち方（案の `prop_spans`）は、メモリ（要素ごとに prop 数ぶんの 2 整数）と、値木の深さ・ノード数の上限（`Limits`）との関係を実装前に確認する。

### (B) `Literal`（リテラルのユニオン）の再公開【推奨: やる】

1. **SPEC**: 変更なし（第 4 章にリテラル型はすでにある）。第 11 章に「host は `Ty::Literal` でリテラル型を宣言できる」を 1 文。
2. **cdl-host API**: `pub use cdl_eval::sema::types::{Field, Literal, Ty};`。旧 → 新: `Literal` が使える。（任意: `Ty::string_union(&["alt", "super"])` のような補助。質問 Q2）
3. **診断コード**: 追加なし。host が列挙にすると、値の誤りが E4004（型のユニオンを表示）になる。
4. **Contrast の移行**: `schema.rs` の `modKey` / `mode` / `animation` / `ipc`（と、望むなら `action`）を `Ty` のリテラルユニオンにする。`convert.rs` の該当検査と CT003 / CT020 の一部が CDL の E4004 に置き換わる（**診断コードが変わる**。`crates/contrast-policy-cdl/tests` と `cdl-contract.md` 4 節の更新が必要）。`action` の 60 個超をユニオンにするかは Contrast が決める（CT020 のメッセージの質は host のほうが良いかもしれない）。
5. **テスト**: `cdl-host/tests/` に、`Literal` で作った列挙を prop にして、範囲外の値で E4004、範囲内で通るテスト。`Env` のフィールドにしたときの `render` と E4014 のテスト。
6. **破壊的か**: 非破壊（追加のみ）。
7. **推奨**: やる。理由: 費用が 1 行で、Contrast が 5 つの回避策を持ち、`Env` の型（U4）にも要る。**「順序付き列挙」の「順序」は CDL に入れない**: ユニオンは集合で、順序の概念も比較演算子もない。順序が要るなら host が持つ（`Tiers` の単調性は、すでに host の変換で検査）。順序を CDL に入れるのは、方針だけを根拠にした変更になる。

### (C) `Duration` の扱い【推奨: やらない（試験だけ足す）】

1. SPEC / API / 診断コード: 変更なし。`150ms` のリテラルは構文の追加で、G1 の決定（関数で作る）に反する。
2. 残る負債: `sec(1.1)` が E5009 になる**疑い**（Q-C1）。9 章 V4 で確かめてから、挙動が不便なら新しい DECISIONS 項目として、`sec` の丸め規則を質問する（現段階では変更しない）。
3. Contrast の移行: なし。

### (D) 設定ファイルの `Env` ルート（関数の形）への移行【推奨: 先送り（U4 の着手時）。CDL 側の変更は不要】

1. **SPEC**: 変更なし。静的な形は引き続き有効（SPEC 238）。
2. **CDL の API**: 変更なし。ただし T5（`check` が本体を評価しない）に対し、`Engine::check_with_env(&Value)` のような追加は U4 の設計で必要になりうる（質問 Q3）。
3. **Contrast の移行**: 既存の `config/*.cdl`・`examples/*.cdl` は**書き直し不要**（静的な形は有効なまま）。`Env` を使う部分だけを関数の形にし、子ファイルの部品は `Env` の一部を props で受ける。`crates/contrast-policy-cdl/src/loader.rs:100-107`（`root()` が `None` を内部エラーにしている）は、`is_function_root()` のときに `render(&env)` する形に直す。リロード失敗時に旧版を保持するため、**`load` の後に代表の `Env` で `render` して `to_settings` まで通す**設計が必要（Q-C3 末尾）。
4. **前提となる確認（CDL 側、テストのみ）**: import + 関数の形のルート、`&&` の条件、`policy` の同居（9 章 V1〜V3）。
5. **破壊的か**: CDL としては非破壊。Contrast の設定ファイルとしては、`Env` を使うファイルだけが新しい書き方になる（既存の設定は壊れない）。
6. **推奨**: 先送り。DU-7 が `Env` と Profile を U4 まで保留している。ただし、**V1〜V3 のテストは U3 の前に C 側で足しておく価値がある**（コード変更なし）。

### (E) 配列・レコードの型【推奨: 先送り】

1. 型としては表せる（Q-C4）。足りないのは、`FromValue` のオブジェクト・`Record`・タプルの実装（`BTreeMap<String, T>` など）。追加は非破壊で小さいが、**使う側（シーン宣言）がまだ仕様にない**ので、今入れると先回りの抽象化になる（CLAUDE.md「迷ったら入れない」）。
2. 異種のリストは、型付きの子要素（`<App />`）で今の機能で書ける。
3. 残る負債: host が自前で `FromValue` を書く手間。U4 以降に、具体的な形が決まってから再検討。

### (F) 「React 式の宣言的 UI に近づける」方針からの変更【推奨: やらない】

方針だけを根拠にする変更は残さない。Contrast の仕様から実際に出た要求（`Motion` の範囲、`Tiers` の個数、`Env` と条件、シーン宣言）は、(A)〜(E) と T4 で覆われる。ほかに、この調査で「React に近づけないと Contrast が困る」と示せたものは見つからなかった。

### (G) 負債の解消（5.3 から）

- T4（ちょうど 1 つ・0 か 1 個の宣言）: **先送り**。host の検査が動いていて、(A) で位置が改善する。入れるなら `ComponentDef::required_child` / `max_children` のような追加で、非破壊。
- T3（範囲の宣言）: **先送り**。型システムの拡張で、`Motion` / `Tiers` の実装が (A) で足りるかを見てから。
- T5・T6: T5 は (D) とセット。T6（E4014 のメッセージにフィールドの経路）は、`Env` を大きくする U4 の前に直すとよい（非破壊・小）。
- T7・T8: 次の保守の機会に（非破壊・小）。T8 は先に再現テストを書く。
- T9: 文書の数字を現状に合わせる（非破壊・小）。
- T10: 9 章のテストを足す。K1 は今回の範囲外。

## 6. 段階案（5.5）

各段階は、CDL の `cargo test` / `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings`（CLAUDE.md は `scripts/check-all.sh`）が緑で終わる。メモリ上限つきの実行（プロンプトの `systemd-run … nix develop -c cargo … -j 2`）で行う。

| 段階 | 内容 | 破壊 | 止められる点・人間が確認する点 |
| --- | --- | --- | --- |
| **C1** | テストだけ（コード変更なし）: 9 章 V1〜V5。`Env` + import、`&&`、`policy` 同居、`sec` の小数、`Literal` の動作確認用の準備。**失敗したら、それ自体が新たな問題として報告** | なし | 結果を見て、(D) の前提と V4 の扱いを決める |
| **C2** | (B) `Literal` の再公開 + テスト + SPEC 第 11 章の 1 文 + DECISIONS に 1 項目 | 非破壊 | `Ty::string_union` を入れるか（Q2） |
| **C3** | (A) prop の位置: 先に `node_api.rs` のテスト、次に実装、SPEC 第 6 章・第 10 章、DECISIONS。バージョンを `0.2.0` に上げる | ほぼ非破壊 | 位置の表現（Q1）、メモリの確認 |
| **C4** | 文書: T9（数字）、T6（E4014 のメッセージ）、T7 / T8 の再現テスト（任意） | 非破壊 | いつやるか |

- **Contrast の固定**: Contrast はすでに `rev = 0d252db…` で CDL を固定している（`contrast-policy-cdl/Cargo.toml:13`）。着手前の固定はこれで足りる。CDL 側に `git tag`（例: `pre-c1`）を打つかは、リポジトリへの書き込みなので、承認が要る（Q4）。
- **Contrast の移行のタイミング**: **C3 まで終わってから 1 回**を推奨。C2・C3 は追加が中心で、Contrast は固定した rev のまま動く。途中で rev を上げると、`Literal` による診断コードの変更（CT003 → E4004）と、位置の変更が別々に Contrast のテストを壊し、確認が 2 回になる。ただし (B) の移行は診断コードの変更を伴うので、Contrast 側で独立した作業として扱う（`crates/contrast-policy-cdl/tests` と `cdl-contract.md` の更新込み）。
- **バージョン**: 0.x のうちは、API の変更（C3 の E4008 の位置の変更、`Element` のフィールド追加）で `0.2.0`、追加だけの C2 は `0.1.x` または `0.2.0` に含める。edition は変えない（`"use cdl 1"` のまま。SPEC 559: 0.x の edition 1 は不安定）。
- **各段階の流儀**: CLAUDE.md により、仕様を変える実装は先に `conformance/` かホストのテストを足す。区切りごとに動く状態でコミットする。Kani は使わない。

## 7. 見つけた問題（何が問題か / 再現入力 / 影響）

| # | 何が問題か | 再現入力（分かっていること） | 影響 |
| --- | --- | --- | --- |
| P1 | `check` が関数の形のルートの本体を評価しない | `cdl check` は `check_until` だけを呼ぶ（`cdl-cli/src/lib.rs:258-262`）。本体は `eval --env` / `render` でのみ評価（SPEC 239）。関数の本体に 0 除算を書いて `check` する、という入力で再現するはず（**実行していない**） | 関数の形のルートを使う設定で、「check OK」が検査の完了を意味しない |
| P2 | 評価後の E4006 は最初の 1 件で打ち切る | `eval.rs:1066-1073`（`return Err(e)`）。`map` が一意な子を 2 組以上重複させる入力（**実行していない**） | 1 回に直せる誤りが 1 件。E4011 は全件報告なので不揃い |
| P3 | `Schema::component(name)` がモジュールを区別しない | `schema.rs:48-51`。2 つの host モジュールに同名のコンポーネントを登録する入力（**未実行。実害は未確認**） | 複数モジュールの host で取り違える可能性 |
| P4 | Contrast の契約と CDL の API の食い違い（記述の範囲） | `cdl-contract.md:239` は「props に位置がない」だが、静的検査の E4003 / E4004 は prop の位置を指す（`check.rs:3035-3076`）。値木の診断について限定して書くべき | Contrast 側の文書の修正（CDL の仕様の問題ではない） |

CLAUDE.md が求める「仕様と実装の食い違い」は、今回の読みの範囲では見つかっていない（SPEC 第 6 章の `Env`・`render`・E4014 は実装と一致）。ただし SPEC 全文と実装の突き合わせはしていない（HANDOFF:176 と同じ限界）。

## 8. 未確認

- **ビルド・テスト・CLI・Kani を 1 回も実行していない**。この報告の「成り立つ」は、すべて読んだ結果。
- `Env` + import + 複数ファイル、`&&` の条件（関数の形のルート内）、`policy` と関数の形の同居、`Env` 型の props の受け渡し: テストなし・未実行。
- オブジェクト / `Record` を host コンポーネントの prop の型にした場合の静的検査・評価・読み出し: テストなし。
- `sec(0.15)` / `sec(1.1)` の挙動（推測のみ）。
- `eval.rs` の `&&` と子の除去の実装、`eval.rs:975-1029` の props の組み立て（C3 の実装の見積もり）は精読していない。
- `Element` を構造体リテラルで作る host が Contrast にあるか（Contrast のコードは `contrast-policy-cdl/src` の 一部しか読んでいない: `schema.rs`、`loader.rs`、`convert.rs` の grep）。`hook.rs`・`resolver.rs`・`tests/` は未読。
- `Schema::component` の取り違え（P3）の実害。
- 診断コード表と実装の 1 件ずつの一致（件数 76 = 76 のみ確認）。
- `docs/PROTOTYPE-REPORT.md` 6 章「SPEC への反映が未承認」の項目が現在も残っているか。
- Contrast の `git status` の全体、`ui-spec.md` / `cdl-requests.md` の中身（見つからず）、`config/` と `examples/` の全 `.cdl`（`config.cdl` と `keybinds.cdl` の冒頭のみ読んだ）。

## 9. 検証の入力案（C1 の素案。リポジトリには書いていない）

`render.rs` の `schema(true)`（`Root` / `Item` / `Env { output: {name, width, height, scale} }`）を前提にした素案。`Memory` リゾルバで 2 ファイルを返す。

```tsx
// V1: import + 関数の形のルート（main.cdl）。期待: load 成功、render 成功
"use cdl 1";
import { Root } from "host:t";
import type { Env } from "host:t";
import { Part, BASE } from "./parts.cdl";
export default function App(env: Env): Element<Root> {
  return <Root><Part scale={env.output.scale} /></Root>;
}
// parts.cdl
"use cdl 1";
import { Item } from "host:t";
export const BASE = 8;
export function Part(props: { scale: number }): Element<Item> {
  return <Item name={`s${props.scale * BASE}`} />;
}
```

```tsx
// V2: 関数の形のルートでの条件（&&）。期待: scale が 2 のとき Item が出て、1 のときは出ない
export default function App({ output }: Env): Element<Root> {
  return (<Root>{output.scale > 1 && <Item name="hidpi" />}</Root>);
}
```

```tsx
// V3: policy と関数の形のルートの同居（SPEC は禁止していない）。期待: 通る（通らなければ仕様の沈黙として質問）
"use cdl 1";
import { Root } from "host:t";
import type { Env, Policy, WindowInfo, Placement } from "host:t";
export const policy = { windowOpen: (w: WindowInfo): Placement => ({ floating: w.isDialog }) } satisfies Policy;
export default function App(env: Env): Element<Root> { return <Root />; }
```

```tsx
// V4: 小数の Duration。期待（推測）: sec(0.15) は通るか不明、sec(1.1) は E5009
const a = sec(0.15);
const b = sec(1.1);
const c = ms(150);
```

```rust
// V5: Literal の再公開後（C2）に使う形。期待: "alt" と "super" は通り、"meta" は E4004
let mod_key = Ty::union(vec![
    Ty::Literal(Literal::String("alt".into())),
    Ty::Literal(Literal::String("super".into())),
]);
```

## 10. 質問（選択肢と推奨つき）

**Q1. prop の位置の表現（C3）**
- (a) `Element` に `prop_spans: BTreeMap<String, Span>` を足し、`Node::prop_span` / `prop_error` を追加（**推奨**。最小で、値の型（`Value`）を変えない）
- (b) `props` の値を `Spanned<Value>` にする（`Value` を使う全箇所が壊れる。却下を推奨）
- (c) 位置は作らず、host のメッセージ頼みのままにする

**Q2. リテラルのユニオンの補助（C2）**
- (a) `Literal` の再公開だけ（**推奨**。1 行）
- (b) 加えて `Ty::string_union(&["alt", "super"])` のような補助（host の記述が短くなるが、API が 1 つ増える）
- (c) 範囲・列挙を宣言できる型を足す（T3。大きい。先送りを推奨）

**Q3. `Env` ルートの `check`（T5）**
- (a) 何もしない。Contrast が `load` のあとに代表の `Env` で `render` する（**推奨**。U4 の設計で決める）
- (b) `Engine::check_with_env(&Value)` や `cdl check --env` を足す（非破壊・小）

**Q4. C1 の前の固定と順序**
- (a) 既存の rev 固定のまま、C1（テストのみ）から始める（**推奨**）
- (b) CDL に `git tag pre-c1` を打つ（リポジトリへの書き込み。承認が要る）

**Q5. 見つからなかった資料**
`ui-spec.md` と `cdl-requests.md` の置き場所を教えてほしい。今回は `docs/cdl-contract.md` と `docs/ui-breaking-update.md` を出所とした。Motion / シーン宣言が CDL に要求するものは、この 2 つからは読み取れなかった（`Motion` の内容は §4 の INV-T1 と §6 の U3 に名前があるだけ）。要求が増えるなら、(A)〜(G) の範囲が変わりうる。
