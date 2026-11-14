# CDL (Contrast Document Language)

CDL は、Wayland コンポジタ Contrast の設定とポリシーを宣言するための、TypeScript 風構文・JSX 風ドキュメントの静的型付き言語です。設定は要素のツリー（ドキュメント）として書き、評価は純粋・決定的で、必ず停止します。

このリポジトリは Contrast から独立していて、Contrast には外部ライブラリ（`cdl-host`）として組み込まれます。ホストを限定しないので、Contrast 以外にも埋め込めます。

> **状態: v0.1（2026-11-12。edition 1 は不安定で、`0.x` のあいだは互換性を約束しない）。**
> 実装は、仕様の主張（決定性・停止性・型安全）が成り立つかを確かめるためのプロトタイプ（`oxc_parser` を内部で使う）です。検証の範囲と、確かめていないこと（Kani を v0.1 の変更後に再実行していないことを含む）は「[v0.1 の現状](#v01-の現状)」にあります。最終精査の結果は [`docs/AUDIT.md`](docs/AUDIT.md)、Contrast への組み込みを担当する人への引き継ぎは [`docs/HANDOFF.md`](docs/HANDOFF.md)（どちらも 2026-11-11 時点の記録で、v0.1 の仕様変更は DECISIONS の R1〜R4・U1〜U5 が正）にあります。
> この README は仕様の要約です。**正典は [`docs/SPEC.md`](docs/SPEC.md)** で、仕様が沈黙している点の決定は [`docs/DECISIONS.md`](docs/DECISIONS.md) にあります。食い違いがあれば、そちらに従ってください。

## 目次

- [例](#例)
- [目的と非目標](#目的と非目標)
- [設計原則](#設計原則)
- [字句とファイル形式](#字句とファイル形式)
- [型システム](#型システム)
- [構文と意味論](#構文と意味論)
- [ドキュメントモデルとスキーマ](#ドキュメントモデルとスキーマ)
- [評価モデル](#評価モデル)
- [ポリシー](#ポリシー)
- [モジュール](#モジュール)
- [診断](#診断)
- [使い方](#使い方)
- [ホストへの組み込み](#ホストへの組み込み)
- [リポジトリ構成](#リポジトリ構成)
- [検証](#検証)
- [バージョニング](#バージョニング)
- [v0.1 の現状](#v01-の現状)

## 例

```tsx
"use cdl 1";
import { Contrast, Appearance, Animation, Keybind } from "host:contrast";

const BASE = px(8);
const focus: Color | undefined = undefined;

function Bind(props: { keys: string; action: string }): Element<Keybind> {
  return <Keybind keys={`mod+${props.keys}`} action={props.action} />;
}

export default (
  <Contrast>
    <Appearance gap={BASE * 2} radius={BASE} focusColor={focus ?? color("#89b4fa")} />
    <Animation duration={ms(120)} />
    <Bind keys="Return" action="spawn terminal" />
    <Bind keys="q" action="close" />
  </Contrast>
);
```

ルートは、ホストの環境 `Env` を受け取る関数でも書けます（UI = f(env)）。出力（モニタ）ごとの scale で gap を変える例:

```tsx
"use cdl 1";
import { Contrast, Appearance, Keybind } from "host:contrast";
import type { Env } from "host:contrast";

const BASE = px(8);

export default function App({ output }: Env): Element<Contrast> {
  return (
    <Contrast>
      <Appearance gap={BASE * output.scale} radius={BASE} />
      <Keybind keys={`output-${output.name}`} action={`width ${output.width}`} />
    </Contrast>
  );
}
```

テキスト子・条件描画・`key`・スロット・引数の分割代入を使った例（`Label` はテキストを、`Group` はスロットと要素・テキストの混在を受け付けるホストのコンポーネント）:

```tsx
"use cdl 1";
import { Contrast, Group, Keybind, Label } from "host:contrast";

const binds = [
  { id: "term", keys: "mod+Return", action: "spawn terminal" },
  { id: "close", keys: "mod+q", action: "close" },
];

function Bind({ keys, action = "noop" }: { keys: string; action?: string }): Element<Keybind> {
  return <Keybind keys={keys} action={action} />;
}

const verbose = true;

export default (
  <Contrast>
    <Group header={<Label>Keybinds</Label>}>
      {binds.map((b) => <Bind key={b.id} keys={b.keys} action={b.action} />)}
    </Group>
    {verbose && <Label>{"2 binds"}</Label>}
  </Contrast>
);
```

複数ファイルとポリシーを含む例は [`examples/contrast/`](examples/contrast/)（出力ごとの設定は `per-output.cdl`）にあります。

## 目的と非目標

目的:

- 設定を、型付きの要素ツリーとして宣言できる
- TypeScript と React（JSX）に慣れた人が、そのまま読み書きできる
- 繰り返しや共通部分を、純粋なコンポーネント（関数）にまとめられる
- 定数計算・条件・配列変換・ポリシー関数を、副作用なしで書ける
- 誤りを、位置つきの診断で確実に報告する

非目標:

- 汎用プログラミング言語ではない（チューリング完全でない）
- TypeScript の再実装ではない（書き方と型の表記を借りるだけ）
- I/O・時計・乱数・ネットワーク・環境変数を持たない
- 動的コード評価やメタプログラミングを持たない

### 用語

| 用語 | 意味 |
| --- | --- |
| Document | `.cdl` のエントリファイルを評価して得られる、型付きの要素ツリー |
| Element | `<Name prop={…}>…</Name>` と書く、ツリーの 1 ノード |
| Component | props を受け取り Element を返す純関数（ホストの組み込みと、CDL 内で書くもの） |
| Schema | ホストが提供する組み込みコンポーネントの型契約 |
| Host | CDL を組み込むアプリケーション（例: Contrast） |
| Policy | ホストから呼ばれる、純粋関数としての判定ロジック |
| Edition | 言語の互換性世代。ファイル先頭で宣言する |

## 設計原則

1. **決定性**: 同じ入力（ソースとホスト値）は、常に同じ結果と同じ診断を返す。
2. **純粋性**: 評価は外界を読み書きしない。副作用はホストが結果を受け取って行う。
3. **停止性**: 再帰と無限ループを構文・名前解決の段階で排除し、さらにステップ上限で保証する。
4. **静的型付け**: すべての式は評価前に型が決まる。`any` と暗黙変換はない。
5. **閉じた世界**: オブジェクト型は閉じていて、未知のキーは誤り。
6. **ホスト非依存**: コアはファイルシステムにも Contrast にも依存しない。
7. **診断優先**: 最初の誤りで止めず、回復して複数の誤りを位置つきで報告する。
8. **小さな言語面**: 迷ったら入れない。追加は edition 単位で行う。

## 字句とファイル形式

- 拡張子は `.cdl`。UTF-8（BOM なし）。改行は LF / CRLF（内部で LF に正規化）。BOM・単独の CR・U+2028・U+2029 は字句エラー。
- 構文は TSX のサブセット。文末の `;` は必須（自動セミコロン挿入なし）。
- 診断の位置（span）は元のソースのバイトオフセット。

| 種類 | 規則 |
| --- | --- |
| 識別子 | XID\_Start / `$` / `_` で始まる。コンポーネント名は ASCII の `A`–`Z` で始める |
| 数値 | `number`。10 進・`0x`・`0b`、`_` 区切り、小数・指数可。`0o`・先頭 0 の 8 進・BigInt は不可。有限にならないリテラル（`1e400`）は字句エラー。`NaN` / `Infinity` は存在しない |
| 文字列 | `"…"` / `'…'`。エスケープは `\n \t \\ \" \' \u{…}`（Unicode スカラー値のみ） |
| テンプレート文字列 | `` `…${式}…` ``。補間できるのは `string` と `number` だけ。タグ付きは不可 |
| 真偽・不在 | `true` / `false` / `undefined`（`null` はない） |
| JSX | `<A />`、`<A>…</A>`、フラグメント `<>…</>`。属性値は `"…"` か `{式}`、子は要素・テキスト・`{式}` |

- JSX のテキスト子（`<Label>text</Label>`）は、子に string を受け付けると宣言したコンポーネントにだけ置けます（それ以外は E3014）。JSX の標準（Babel / TypeScript）と同じ空白規則です（各行の前後の空白を除く。最初の行の先頭と最後の行の末尾は残す。空行を捨て、残りを 1 つの空白でつなぐ。`<Label> hi </Label>` は `" hi "`）。標準と違うのは、空白だけのテキストを常に捨てる点だけです。`{"text"}` は加工しません。文字参照（`&amp;`）は使えず、テキストの `&` は E1012 です。
- JSX の属性文字列（`name="…"`）はエスケープを解釈しません。文字参照（`&amp;`）も使えず、`&` を書くと字句エラーです。必要なら `name={"a & b"}` と書きます。
- 単位つきの値は専用リテラルではなく、組み込み関数で作ります: `px(8)` `ms(120)` `sec(1)` `color("#89b4fa")`。
- コメントは `//` と `/* */`（入れ子不可）。`/** */` は直後の宣言に紐づくドキュメントコメント。

### 予約語

- 使える: `const` `function` `return` `if` `else` `switch` `case` `default` `type` `interface` `import` `export` `from` `satisfies` `true` `false` `undefined`
- 使うと診断: `let` `var` `class` `new` `this` `null` `for` `while` `do` `break` `continue` `async` `await` `yield` `try` `catch` `throw` `enum` `namespace` `as`
- React の機能も存在せず、専用の診断になります: `use[A-Z0-9_]` で始まる名前（Hooks。`user` は可）、`on[A-Z]` で始まる属性（イベントハンドラ）、`ref` 属性（`key` 属性は書けます）。

キーワードはプロパティ名としては使えます（`{ default: 1 }`、`a.type`）。束縛の名前には使えません。

## 型システム

型は構造的で、評価前にすべて決まります。注釈は TypeScript と同じ書き方です。推論は `const` の初期化式に限った局所推論で、`function`・トップレベルの `const f = (…) => …`・ポリシーのメンバーは引数と戻り値の注釈が必須です（組み込みメソッドに直接渡すアロー関数だけ、文脈から推論）。

| 型 | 表記 | 備考 |
| --- | --- | --- |
| 真偽 | `boolean` | |
| 数値 | `number` | IEEE 754 binary64 のみ。`NaN` / `±Infinity` を生む演算は評価エラー |
| 文字列 | `string` | UTF-8、長さ上限あり |
| 長さ | `Length` | `px(8)`。`L ± L`、`L * n`、`n * L`、`L / n`（`L / L` は不可） |
| 時間 | `Duration` | `ms(120)` / `sec(1)`。内部は整数ミリ秒。結果が整数ミリ秒にならなければ評価エラー |
| 色 | `Color` | `color("#rgb" / "#rgba" / "#rrggbb" / "#rrggbbaa")`。演算は `===` / `!==` だけ |
| 配列 | `T[]` / `Array<T>` | 空配列 `[]` は型注釈が必須 |
| タプル | `[A, B]` | 注釈か文脈型があるときだけ |
| レコード | `Record<string, T>` | 参照は `r["k"]` / `r[k]` だけ（型は `T \| undefined`）。キー順は UTF-8 のバイト順 |
| オブジェクト | `{ k: T }` / `interface` | 閉じた構造的型。余分なキーは誤り |
| 不在 | `T \| undefined`、`k?: T` | プロパティがないことと `undefined` は区別しない |
| リテラル型 | `"a" \| "b"` | 文字列・数値・真偽 |
| 要素 | `Element<C>` | `C` はホストの組み込みコンポーネント名かそのユニオン |

規則:

- **部分型**: ユニオンの包含と、不変コンテナ（配列・タプル・`Record`・フィールド・`Element<…>`）の共変性だけ。関数型は不変。幅部分型はなく、過剰プロパティは常に誤り。
- **ユニオン**: リテラル型どうし、`T | undefined`、`Element<A | B>`、子の型 `Element<A> | string`（要素とテキストの混在）、`c && B` の型 `false | B` だけ。オブジェクト型どうしのユニオンや `string | number` は書けない。
- **絞り込み**: `undefined` を含む型は `??`・`?.`・`=== undefined` / `!== undefined` で絞ってから使う。対象は識別子と `a.b.c` の形のパスで、`if`・`?:`・`&&`・`||` の中で効く。`switch` の `case` 内では、判定する値がそのリテラル型に絞られる。
- **`const` の推論**: プリミティブのリテラルで初期化するとリテラル型になる。オブジェクト・配列の中のリテラルは基本型に広がる。
- `any`・`unknown`・`as`・`!` はない。使える型の演算子は `satisfies` だけ。
- 再帰型は書けない。ユーザーはジェネリクスを定義できない（型引数を取るのは組み込みの `Array`・`Record`・`Element` だけ）。
- 組み込みの `px` `ms` `sec` `color` と型 `Length` `Duration` `Color` `Element` `Array` `Record` は、import せずに使える（シャドーイング不可）。

## 構文と意味論

式指向で、再代入・ループ・クラスはなく、束縛はすべて不変（`const`）です。

### 宣言

| 宣言 | 役割 |
| --- | --- |
| `"use cdl 1";` | edition 宣言。全ファイルの最初の文として必須 |
| `import { … } from "…";` / `import type { … } from "…";` | 他ファイル・ホストの名前を取り込む |
| `const name: T = expr;` | 不変の束縛（注釈は省略可） |
| `type` / `interface` | 型宣言 |
| `function name(a: A): R { … }` | 純関数 |
| `function Name(props: P): Element<…> { return <…/>; }` | コンポーネント。ASCII 大文字で始まり、props を 1 つだけ受け取り、JSX からだけ使える |
| `export default <…/>;` / `export default function App(env: Env): Element<…> { … }` | ルート。要素（静的な形）か、ホストの `Env` を受け取る関数（アロー関数も可）|
| `export const policy = { … } satisfies Policy;` | ポリシー |

- トップレベルの宣言は順序を問いません。値は依存関係の順に評価され、循環はエラーです。ブロック内の `const` は、前に宣言した名前だけを参照できます。
- 名前の隠蔽（prelude・import・外側のスコープと同じ名前を内側で宣言すること）はできません。

### 式

- 算術 `+ - * / %`、単項 `-`、比較 `=== !== < <= > >=`（`==` `!=` `**` とビット演算はない）、論理 `&& || !`、`??`
  - `||` `!` と `if` / `?:` の条件は `boolean` だけ（真とみなす判定はない）。`&&` の左辺も `boolean` だけで、右辺は何でもよい（式の型は `false | B`）
  - 文字列の連結はテンプレート文字列で書く
  - `===` は、プリミティブ・リテラル・単位つきの値・`undefined` に使う。オブジェクト・配列・要素や、重なりのない型どうしは比べられない
- 関数本体では `const` と、全経路が `return` で終わる `if` / `else` / `switch` が書ける。`else` は省略できない。
- `switch`: 1 つの `case` に文は 1 つ。フォールスルーなし。リテラルのユニオン型は網羅性を検査（`default` は省略可）、それ以外は `default` が必須。
- 配列の操作は有界な組み込みだけ: `map` `filter` `reduce`（初期値必須）`includes` `concat`、`length`。
- プロパティ参照 `a.b` `a?.b`、添字 `a[i]`（範囲外・負・整数でないときは `undefined`）、スプレッド（後に書いたものが勝つ）。
- 0 除算、`NaN`/`±Infinity` を生む演算は評価エラー。演算結果の `-0` は `+0` に正規化。

### JSX

- 属性は `name={expr}`、`name="text"`、真偽値の省略形 `flag`、スプレッド `{...props}`。
- 条件は `c ? <A /> : undefined` か `c && <A />`。`false` と `undefined` の子は、どの子の位置でも評価結果から取り除かれます（`true` は置けません）。
- `key="a"` / `key={expr}`（string だけ。number は E3017 で、`` key={`${n}`} `` と書く）。props ではなくノードの `key` として残り、同じ親の子で重複すると E4011 です（文字列リテラルどうしは評価の前に、動的なものは評価後に、すべて報告）。ユーザー定義のコンポーネントに付けた `key` は、展開後のルート要素の `key` になります。
- 繰り返しは `xs.map((x) => <Item … />)`。配列の子は自動で平坦化されます。
- `{…}` の中は純粋な式だけ。イベントハンドラ、`ref`、Hooks、状態、Context、子としての関数はありません。

### 引数の分割代入

関数・コンポーネントの引数に、1 階層のオブジェクトパターン `{ a, b = 既定値 }: P` を書けます（型注釈は必須）。既定値は `P` で省略可能（`?`）な prop にだけ付けられ（E3015）、本体では `undefined` が除かれた型になります。既定値の式は、prop が `undefined` のときだけ、呼び出しごとに評価されてステップを消費します（同じ関数の引数は参照できません）。

```tsx
function Bind({ keys, action = "noop" }: { keys: string; action?: string }): Element<Keybind> {
  return <Keybind keys={keys} action={action} />;
}
```

### 書けない TypeScript の構文

`const` への分割代入と、引数パターンの別名・入れ子・rest、`enum`、import の改名、既定引数・省略可能引数（引数パターンの `{ a = 既定値 }` は書けます）、計算プロパティ名、メソッド構文、`interface … extends`、交差型、`keyof` などの型演算子、正規表現、BigInt、カンマ演算子、`export { … }` と再エクスポート、ブロック内の関数宣言、`f(...xs)`。

## ドキュメントモデルとスキーマ

エントリファイルはちょうど 1 つの `export default`（ルート）を持ちます。ルートは要素（静的な形）か、引数 1 つの関数（下記）です。評価すると、コンポーネントがすべて展開された不変の要素ツリーになります。各ノードは、その要素を作った JSX の位置と、展開の途中で通ったコンポーネント呼び出しの位置の列を持つので、ホストは「どの行の設定か」を診断に使えます。

**スキーマ**はホストが Rust のビルダー API で登録し、`import { Contrast } from "host:contrast"` のように取り込みます（CDL の中に `declare` は書けません）。各コンポーネントは props の型と、子に許す要素の型を持ち、ルート要素の種類もホストが指定します。

- props は閉じた型。省略できるのは `?` 付きだけで、欠落・余分・型違いは診断される。
- 子は、平坦化して `false` と `undefined` を除いた配列として渡される。型の違う子は診断される。`children` 属性と JSX の子は同時に書けない。
- 評価結果の子ノードは、要素かテキスト（string）。テキストを受け付けるコンポーネントは、ホストの `ComponentDef::text_children()` で宣言する（ユーザー定義のコンポーネントは `children: string[]`）。
- `key` は props ではなく、ノードの `key` として残る（`cdl-host` の `Node::key()`）。ホストの同一性は、key → ホストが宣言した識別 prop → 親・位置の順。
- ホストが一意と宣言した子（例: `Appearance`）の重複はエラー（後勝ちにしない）。`map` が作る子は、評価後のツリーで検査する。
- 値が `undefined` の props はツリーに含めない。関数の props はツリーに置けない。
- **スロット**: 要素を props に渡せる（`<Group header={<Label />}>`）。props の型は `Element<Label>`（ユニオン・配列も可）で、スロットの要素は子とは別に props の値として残る。型違いの要素は E3016。
- **子の型**: `(Element<A> | string)[]` で、要素とテキストの混在する子を受け取れる（テキストだけなら `string[]`）。
- ツリーは評価後に不変で、ホストは旧ツリーとの差分でリロードできる。

### ルートの関数の形（UI = f(env)）

`export default` に、ホストの環境 `Env`（出力の名前・解像度・scale など）を受け取る関数を書けます。引数は名前つきか 1 階層のオブジェクトパターンで、型注釈は必須。型はホストが宣言した `Env`（`import type { Env } from "host:contrast"`）と完全に一致しなければならず（E4012）、戻り値の注釈（ルート要素）も必須です。`Env` を宣言していないホストでは書けません（E4013）。

評価は純粋なままです。ホストは 1 回読み込んだ `Program` から、`Program::render(&env)` で何度でも（複数のスレッドからでも）要素ツリーを作ります。render ごとに独立したステップ予算を使い、同じ `env` からは必ず同じ木になります。`env` の値が `Env` の型に合わなければ E4014。静的な形のルートでは `env` は無視されます。関数の形のルートの本体は render のときに評価されるので、評価の誤りもそのとき報告されます（`cdl check` は型検査まで）。

参照ホスト `host:contrast`（適合テストと CLI が使う）のスキーマは [`conformance/HOST.md`](conformance/HOST.md) にあります。

## 評価モデル

評価は **パース → 名前解決 → 型検査 → 評価** の 4 段階です。前段に誤りがあれば後段へは進みません（診断は蓄積されます。`warning` では止まりません）。複数ファイルでも、段階はプログラム全体で進めます。評価器は I/O を持たず、入力はソースとホスト提供の読み取り専用の値だけです。

### 停止性

- トップレベルの宣言の参照グラフは DAG（呼び出しに限らず、名前が現れるすべての箇所が辺）。再帰・相互再帰はエラー。
- 再帰型を書けないので、関数を値として渡しても評価は必ず停止する。
- ループ構文はなく、反復は `map` / `filter` / `reduce` などの有界な組み込みだけ。
- さらに実行時にリソース上限を強制する（多重防御）。

### リソース上限

超過すると評価はその場で打ち切られ、`E5xxx` の診断になります。ホストが `Limits` で上書きでき、既定値は暫定です。

| 項目 | 既定値 |
| --- | --- |
| 評価ステップ | 1,000,000（Document 全体。値の深さの検査と、256 バイトごとの文字列生成も数える） |
| 呼び出し深さ | 64 |
| 評価の再帰の深さ | 128（式と要素の入れ子。呼び出しをまたいで数える） |
| 文字列長 | 64 KiB（1 値あたり） |
| 配列・マップ要素数 | 65,536（1 値あたり） |
| 値木ノード数 | 1,000,000 |
| ソースサイズ | 1 MiB（1 ファイル） |
| ファイル数 | 256（エントリを含む） |
| 構文の入れ子の深さ | 64 |
| 値木の深さ | 64 |
| 型の大きさ | 深さ 128・ノード数 4,096（固定。超過は E3013） |
| フック 1 回のステップ | 10,000（別枠） |

### 決定性

`NaN`/`±Infinity` はエラー、`-0` は `+0` に正規化。`Record` は UTF-8 のバイト順で保持し、ハッシュ順に依存しない。組み込み関数は環境・時刻・ロケールに依存しない。

## ポリシー

ポリシーは、ホストが「判断が必要な瞬間」に呼ぶ純関数です。CDL が決めるのは判定の結果だけで、実際の操作はホストが行います。

```ts
export const policy = {
  windowOpen: (w: WindowInfo): Placement => {
    switch (w.appId) {
      case "firefox":
        return { workspace: 2, floating: false };
      default:
        return { workspace: w.workspace, floating: false };
    }
  },
} satisfies Policy;
```

- フックの引数型・戻り値型は、ホストの `Policy` が宣言したものと完全一致する必要がある（`satisfies` で検査。関数型は不変）。フックはすべて省略でき、`Policy` にないフックは診断される。
- `export const policy` はエントリファイルに 1 つだけで、`satisfies Policy` が必須。
- ポリシーは状態を持たない。呼び出しごとに専用のステップ上限が適用される。
- 上限超過・評価エラー時は、そのフックのホスト既定の動作に戻る（診断はホストが受け取る）。
- ホットリロード: ホストは新しい版の評価が成功したときだけ差し替える。ドキュメントとポリシーは 1 つの不変な値（`Program`）で、世代番号と差し替えはホストが管理する。
- フックの一覧と型は CDL の仕様ではなく、各ホストのスキーマの責任。

## モジュール

```tsx
import { BASE, Panel } from "./theme.cdl";
import { Contrast } from "host:contrast";
```

| 構文 | 解決先 |
| --- | --- |
| `from "./path.cdl"` | 他の `.cdl` ファイル。`./` か `../` で始まり `.cdl` で終わる相対パスのみ |
| `from "host:name"` | ホストが公開するモジュール（型・組み込みコンポーネント・読み取り専用の定数） |

- import できるのは `export` 付きの `const` / `type` / `interface` / `function` だけ。`export default` は import できない。
- ライブラリとして読まれるファイルは `export default` と `export const policy` を持てない。
- import の循環は、全ファイルをまとめてエラーにする。同じファイルは 1 回だけ評価して共有する。
- パッケージ管理と外部レジストリはない。
- コアは `ModuleResolver` トレイトを通してのみソースを読む。同じファイルかどうかは正準 ID で判断する。ファイルシステムの実装はホスト（または CLI）が提供するので、コアはサンドボックスや WASM でも動く。

## 診断

すべての誤りは、安定したコード・重大度（`error` / `warning` / `hint`）・位置・メッセージを持つ診断として値で返ります。コアはパニックも `abort` もしません。

| 帯 | 段階 | 例 |
| --- | --- | --- |
| E0xxx | 字句 | 閉じていない文字列、不正なエスケープ |
| E1xxx | 構文 | 期待したトークンがない、禁止構文、edition の不備 |
| E2xxx | 名前解決 | 未定義の名前、循環参照、再帰呼び出し、import の誤り |
| E3xxx | 型検査 | 型不一致、未知のキー、`switch` の網羅性不足 |
| E4xxx | スキーマ適合 | ホストの組み込みコンポーネントと `Policy` についての検査 |
| E5xxx | 評価 | リソース上限超過、0 除算、オーバーフロー |

- E3 と E4 の境界: ホストの組み込みコンポーネントと `Policy` についての検査は E4、それ以外（CDL 内のコンポーネントや型を含む）は E3。例外として、テキスト子を受け付けない位置のテキスト（E3014）とスロットの型違い（E3016）は、ホストの組み込みに対するものも E3。
- 診断は (ファイル, 開始, 終了, コード) の順に並べて返す。意味を変えるときは新しいコードを振る。
- 人間向け（ソース抜粋と下線つき）と機械向け（JSON）で出力できる。JSON は互換性の対象（フィールドの追加は可、削除と意味変更は edition を上げない限りしない）。
- コード一覧: [`docs/DIAGNOSTICS.md`](docs/DIAGNOSTICS.md)

```json
{ "code": "E3001", "severity": "error",
  "span": { "file": "main.cdl", "start": 120, "end": 128 },
  "message": "…", "labels": [ { "span": { … }, "message": "…" } ], "help": "…" }
```

## 使い方

```sh
cargo run -p cdl-cli -- check examples/contrast/main.cdl          # 検査（評価まで行い、診断だけを出す）
cargo run -p cdl-cli -- eval  examples/contrast/main.cdl          # 評価して値木を JSON で出す
cargo run -p cdl-cli -- check --json file.cdl                     # 診断を JSON で出す
cargo run -p cdl-cli -- check --stage syntax file.cdl             # syntax / sema / eval の段階で止める
cargo run -p cdl-cli -- eval --env env.json examples/contrast/per-output.cdl   # 関数の形のルートに Env（JSON）を渡す
```

- 終了コード: `0` 成功、`1` 診断あり、`2` 使い方の誤り。
- CLI は参照ホスト `host:contrast`（`--preset contrast`）のスキーマを組み込んでいます。ほかのホストの設定は CLI では検査できません。

## ホストへの組み込み

Contrast が依存するのは公開 API を持つ `cdl-host` だけです。

```rust
// 1. スキーマを Rust のビルダーで登録する
let schema = Schema::builder()
    .root("host:contrast", "Contrast")
    .component("host:contrast", ComponentDef::new("Appearance").prop("gap", Ty::Length) /* … */)
    .build()?;

// 2. ModuleResolver を用意し、読む（ファイルを読むのはホストの仕事）
let engine = Engine::new().with_schema(schema).with_limits(Limits::default());
let program = engine.load(&resolver, "main.cdl")?;      // Result<Program, Vec<Diagnostic>>

// 3. 要素ツリーを作り（ルートが関数の形なら `Env` を渡す。UI = f(env)）、Rust の型へ読み出す（失敗は E4008）
let tree = program.render(&env)?;                         // Result<Arc<Element>, Vec<Diagnostic>>
let config: Config = Node::new(&tree).read()?;            // Config: FromNode（手書き。derive はない）

// 4. ポリシーのフックを呼ぶ（失敗したらホストの既定動作に戻す）
let placement = program.call_hook("windowOpen", arg)?;
```

動く例: `cargo run -p cdl-host --example contrast`（スキーマの登録・import・`FromNode` の手書き・出力ごとの `Env` での render・フック呼び出し）。公開 API の互換性（semver）を約束するのは `cdl-host` だけで、ほかのクレートは内部実装です。

### 安定させる面（`cdl-host`）

互換性（semver）を約束する面。仕様が `0.x` のあいだは、次の形を「最終形の候補」として固め、変えるときは破壊的変更として DECISIONS に記録する。

| 名前 | 署名・役割 |
| --- | --- |
| `Engine` | `new() / with_schema(Schema) / with_limits(Limits)`、`load(&dyn ModuleResolver, &str) -> Result<Program, Vec<Diagnostic>>`、`check`、`check_until(…, Stage)` |
| `Program` | `render(&Value) -> Result<Arc<Element>, Vec<Diagnostic>>`（`Env` から木を作る。何度でも・複数スレッドから。静的な形のルートでは `Env` を無視）、`is_function_root() -> bool`、`root() -> Option<Node>` / `document() -> Option<&Element>`（静的な形のルートだけ）、`files()`、`call_hook(&str, Value) -> Result<Option<Value>, Diagnostic>`。`Send + Sync` |
| `Schema` / `SchemaBuilder` / `ComponentDef` | `root(module, name)`、`env(module, Ty)`（`Env` の宣言）、`component(module, def)`、`type_def(module, name, Ty)`、`build()`。`ComponentDef`: `prop` / `optional_prop` / `children` / `unique_child` / `text_children` |
| `Value` | 値（`Undefined / Bool / Number / String / Length / Duration / Color / Array / Object / Element / Function`） |
| `Element` | 評価結果のノードの実体: `name`・`props`・`key: Option<String>`・`children: Vec<ChildNode>`・`span`・`expanded_from` |
| `ChildNode` | `Element(Arc<Element>)` / `Text(Arc<str>)`（`as_element()` / `as_text()`） |
| `Node<'a>`（読み出しの入口） | `component() -> &str`、`key() -> Option<&str>`、`props() -> &BTreeMap<String, Value>`、`span() -> &Span`、`expanded_from()`、`children()`（要素だけ）、`child_nodes() -> Child`（`Child::Element(Node)` / `Child::Text(&str)`）、`children_named(&str)`、`children_of::<T: FromNode>()`、`unique_child::<T>()` / `required_child::<T>()`、`read::<T>()`、`prop::<T: FromValue>(&str)`、`slot(&str)` / `slots(&str)` / `slot_as::<T>(&str)`、`expect_component(&str)`、`error(msg)` |
| `FromValue` | props の値から読む trait（`expected() -> String`、`from_value(&Value) -> Option<Self>`）。標準で `String` / `f64` / `bool` / `Px` / `Millis` / `Rgba` / `Option<T>` / `Vec<T>` |
| `FromNode` | 1 ノードから読む trait（`const COMPONENT`、`from_node(Node) -> Result<Self, Diagnostic>`）。derive はなく、手で書く（`examples/contrast.rs`） |
| `ModuleResolver` / `Source` / `Limits` / `Stage` / `Ty` / `Field` / `Diagnostic` / `diag` | 従来どおり |

読み出しの失敗は、要素の位置つきの診断（E4008）。省略できる prop は `Option<T>` で読む。


## リポジトリ構成

```
crates/
  cdl-diag/    診断型（コード・重大度・span・メッセージ）とレンダリング
  cdl-syntax/  ソース → CDL AST（プロトタイプでは oxc_parser を内部で使用）
  cdl-sema/    名前解決・型検査・スキーマ適合
  cdl-eval/    評価器・リソース上限
  cdl-host/    組み込み API（Engine / ModuleResolver / Limits / Schema）
  cdl-cli/     check / eval
conformance/   .cdl と期待結果（値木 or 診断の JSON）。実装非依存
docs/          SPEC.md（正典）, DECISIONS.md（決定。R1〜R4・U1〜U5 が v0.1 の仕様変更）, DIAGNOSTICS.md（診断コード表）,
               PROTOTYPE-REPORT.md・AUDIT.md・HANDOFF.md（2026-11-11 時点の記録）
examples/      Contrast 向けの設定例とホスト組み込みの例
scripts/       check-all.sh ほか
```

依存は一方向です: `diag ← syntax ← sema ← eval ← host ← cli`（`cdl-diag` へは各クレートが直接依存してよい）。

実装ルール:

- すべてのクレートに `#![forbid(unsafe_code)]`。
- clippy で deny: `unwrap_used` `expect_used` `panic` `indexing_slicing` `as_conversions` `arithmetic_side_effects`。
- 公開 API は `Result` を返し、誤りは診断として値で返す。
- `cdl-syntax` / `cdl-sema` / `cdl-eval` は `std::fs` `std::env` `std::time` に触れない。
- 数値演算は checked。値木のマップは `BTreeMap`（`HashMap` を使わない）。
- 依存は最小限。`oxc_parser` は `cdl-syntax` の内側だけに閉じ込め、`oxc_*` の型を外に出さない（本実装で自前パーサに置き換える）。

## 検証

仕様の主張ごとに検証手段を置いています。

| 主張 | 検証手段 |
| --- | --- |
| 字句解析・パースは任意入力でパニックしない | clippy の deny（unwrap・添字・算術など）+ 構文ネスト深さの上限 + proptest。Kani（自前の字句検査の一部。9/10 個成功）。cargo-fuzz は使わない（SPEC 第 12 章） |
| 評価は必ず停止する | 参照グラフの DAG・再帰型の禁止 + ステップ上限 + 上限超過テスト（証明ではない。最終精査 `docs/AUDIT.md` で見つかった、上限をすり抜ける入力は修正済み） |
| 整数演算にラップ・未定義動作がない | `unsafe_code = "forbid"` + clippy の `arithmetic_side_effects` deny（checked 演算） |
| 型検査を通った式は評価で型エラーにならない | proptest（型付き式ジェネレータ） |
| 診断のコード・位置が回帰しない | ゴールデンテスト（`conformance/`） |
| 同じ入力は常に同じ出力 | 同じ入力を 2 回評価する差分テスト |

```sh
bash scripts/check-all.sh   # rules / fmt / clippy / test / conformance / spec-examples をまとめて実行し、OK / NG / 未実行 の表を出す
bash scripts/verify.sh      # 手動確認用: test / fmt / clippy（メモリ上限つき）と、目視用の grep
```

- `scripts/check-all.sh` は、`WITH_KANI=1` を付けないと kani の行を「未実行」として表に出し、終了コードは 2（NG なし・未実行あり）になります。`WITH_KANI=1` を付けると `cargo kani` も実行し、すべて OK なら 0 です。
- 個別のスクリプト: `check-rules.sh`（unsafe・unwrap・`as`・oxc の漏れ・依存の向き・fs/env/time・HashMap の静的検査。最初の `#[cfg(test)]` 以降は走査しない）、`check-conformance.sh`（期待結果の有無・2 回評価の一致・禁止構文・診断コードの各帯）、`check-spec-examples.sh`（SPEC の例が通ること）。
- 結果は OK / NG / 未実行 の 3 種類で、入っていないツールは「未実行」であり成功ではありません。

Kani の実行結果（2026-11-11）: 9 個成功。`lexer_never_panics` は時間切れで実行対象から外した。**v0.1 の変更（2026-11-12）のあと、Kani は再実行していない**（未実行。ハーネスの対象のコードは、字句検査の `,` `;` の扱いを除いて触れていない）。実行していないこと: cargo-fuzz（使わない方針）、ベンチ。最終精査は `docs/AUDIT.md`。

### 適合テスト（`conformance/`）

`conformance/<節>/<名前>.cdl` と `<名前>.expected.json` を並べます。禁止構文は `conformance/forbidden/<id>.cdl`、複数ファイルは `<節>/<名前>/main.cdl` と `expected.json` です。期待結果は `{"value": …}`（値木。span は比べない）か `{"diagnostics": […]}`（コード・重大度・span を比べる）で、フックの呼び出しは `calls.json` に書きます。値木の JSON は、要素 `{"element": "Name", "key": "k", "props": {…}, "children": […]}`（`key` はあるときだけ。子はテキストなら文字列）、単位つきの値 `{"length": 16}` `{"duration": 120}` `{"color": [r, g, b, a]}`、配列内の `undefined` `{"undefined": true}` です。実装に依存しない形式なので、別実装も同じスイートで検証できます。詳細は [`conformance/README.md`](conformance/README.md)。

## バージョニング

互換性の単位は edition で、全ファイルの最初の文の `"use cdl 1";` が必須です（`use cdl` と 10 進の正の整数のあいだに空白 1 つ。引用符はどちらでもよい）。同じ edition なら、古いファイルは新しい実装でも同じ意味で動きます。

- 新しい組み込み関数・型の追加は同一 edition で可。構文・意味の変更や削除は新しい edition でのみ。
- 予約語の追加は、事前に予約して使用を警告してから有効化する。診断コードの意味変更は禁止。
- スキーマのバージョンは言語の edition とは独立で、ホストが管理する。
- 仕様書が `0.x` のあいだ、edition 1 は不安定。

## v0.1 の現状

v0.1 は、仕様（`docs/SPEC.md`）の全体を実装し、`bash scripts/check-all.sh`（rules・fmt・clippy・test・conformance・spec-examples）がすべて OK の状態です（テスト 191 件、適合テスト 150 ケース。Kani は上記のとおり未実行）。`0.x` のあいだ、仕様・診断コードの追加・`cdl-host` の公開 API は、破壊的に変わりうります。

### v0.1 に至る主な仕様変更

`docs/DECISIONS.md` に、変更前・変更後・理由・Contrast への影響を書いています。

| 決定 | 内容 |
| --- | --- |
| R1 | テキスト子 `<Label>text</Label>`（子に string を受け付けるコンポーネントだけ）。文字参照は不可（E1012） |
| R2 | 条件描画 `c && <A />`（左辺は boolean だけ。型は `false \| B`）。`false` と `undefined` の子は取り除かれる |
| R3 | `key`（props ではなくノードに残る。重複は E4011） |
| R4 | 引数の 1 階層の分割代入と既定値 |
| U1 | ルートを `Env` の関数にできる。`Program::render(&env)` |
| U2 | `Element<A> \| string`（混在する子）、スロット（要素を props に渡す。型違いは E3016） |
| U3 | `key` は string だけ（E3017）。重複を、評価前・評価後ともすべて報告 |
| U4 | `cdl-host` の公開 API の整理（`Node` / `FromNode` / `FromValue`） |
| U5 | テキストの空白規則を JSX の標準に合わせた（空白だけのテキストは常に捨てる） |

**できること**
- パース（`oxc_parser` 経由）、名前解決、型検査、スキーマ適合、評価、ポリシーのフック、複数ファイルの import、CLI の `check` / `eval`、Rust のスキーマ・ビルダーと値木の読み出し（`cdl-host`）。

**対象外・未対応**
- `fmt`、`migrate`、LSP、パッケージ管理。ホストが公開する定数（DECISIONS X1）、edition の範囲の問い合わせ API（X2）。
- 構文の誤りからの回復（`oxc_parser` は最初の構文エラーで止まる。SPEC 第 10 章に注記）。`oxc_parser` はプロトタイプ限りで、本実装では自前パーサに置き換える。

**検証したこと**
- 最終精査（[`docs/AUDIT.md`](docs/AUDIT.md)）で、abort・資源枯渇・実用上の非停止を 5 件（P1〜P5）見つけて修正し、軽微な 2 件（P6・P7）も対処しました。各々に適合テストを足しています（型の大きさ E3013、評価の再帰 E5012、ファイル数 E5013 など）。
- Kani は 10 個のハーネスのうち 9 個が成功です（2026-11-11。v0.1 の変更のあとは未実行）。`lexer_never_panics` は入力を 2 バイトまで下げても時間切れで、実行対象から外しました（証明できていません。proptest と突然変異ファジングで代替）。

**確かめていないこと**
- `check.rs`・`convert.rs` の全行精読（再帰の深さの確認と極端な上限値のテストで代えました）。入れ子の前走査が数えない構文が、ほかにもあるかもしれません。
- cargo-fuzz、ベンチ、実際の設定ファイルの規模での性能と上限の既定値の妥当性（既定値は暫定）。oxc 自体のソースの精査。
- Kani と proptest は、仕様の主張を証明するものではありません（SPEC 第 12 章）。

DECISIONS の決定は SPEC に反映済みです。実装して分かったことは [`docs/PROTOTYPE-REPORT.md`](docs/PROTOTYPE-REPORT.md)（2026-11-11 時点）にあります。

## ライセンス

[`LICENSE`](LICENSE) を参照してください。
