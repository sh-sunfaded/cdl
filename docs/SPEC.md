# CDL 仕様書 (Contrast Document Language)

Nov 3, 2026 · @K

## 1. 概要

CDL（Contrast Document Language）は、Contrast の設定とポリシーを「ドキュメント」として宣言するための、TypeScript 風構文の静的型付き言語です。設定は React の JSX に近い書き方で、要素のツリーとして宣言します。評価は純粋・決定的で、必ず停止します。

Contrast は Rust + Smithay 製の Wayland コンポジタです。CDL は独立リポジトリで開発し、Contrast には外部ツール（ライブラリ）として組み込みます。

### 目的

- 設定を、型付きの要素ツリー（ドキュメント）として宣言できる
- TypeScript と React（JSX）に慣れた人が、そのまま読み書きできる構文にする
- 繰り返しや共通部分を、純粋なコンポーネント（関数）にまとめられる
- 定数計算・条件・配列変換・ポリシー関数を、副作用なしで書ける
- ホストを限定しない（Contrast 以外にも埋め込める）
- 誤りを、位置つきの診断で確実に報告する

### 非目標

- 汎用プログラミング言語ではない（チューリング完全でない）
- TypeScript の再実装ではない（書き方と型の表記は借りるが、TS / JS のコードをそのまま実行することは約束しない）
- I/O・時計・乱数・ネットワークを持たない
- 動的コード評価やメタプログラミングを持たない

### 用語

| 用語 | 意味 |
| --- | --- |
| Document | 1 つの `.cdl` エントリファイルを評価して得られる、型付きの要素ツリー |
| Element | `<Name prop={…}>…</Name>` と書く、ツリーの 1 ノード |
| Component | props を受け取り Element を返す純関数。ホストが提供する組み込みと、CDL 内で書くものがある |
| Schema | ホストが提供する組み込みコンポーネントの型契約（props の型と、子に許す要素の型） |
| Host | CDL を組み込むアプリケーション（例: Contrast） |
| Policy | ホストから呼ばれる、純粋関数としての判定ロジック |
| Edition | 言語の互換性世代。ファイル先頭で宣言する |

## 2. 設計原則

機能を足すより削ることを優先し、各原則は「検証できるか」で判断します。

1. **決定性**: 同じ入力（ソースとホスト値）は、常に同じ結果と同じ診断を返す。
2. **純粋性**: 評価は外界を読み書きしない。副作用はホストが結果を受け取って行う。
3. **停止性**: 再帰と無限ループを構文・型検査の段階で排除し、さらにステップ上限で保証する。
4. **静的型付け**: すべての式は評価前に型が決まる。`any` と暗黙変換は存在しない。
5. **閉じた世界**: オブジェクト型は既定で閉じており、未知のキーは誤りとして報告する。
6. **ホスト非依存**: コア実装はファイルシステムにも Contrast にも依存しない。
7. **診断優先**: 最初の誤りで止めず、回復して複数の誤りを位置つきで報告する。
8. **小さな言語面**: 迷ったら入れない。追加は edition 単位で行う。

## 3. 字句構造とファイル形式

ソースは UTF-8（BOM なし）です。改行は LF と CRLF を受け付け、内部では LF に正規化します。BOM・単独の CR・U+2028・U+2029 は字句エラーです。診断の位置（span）は元のソースのバイトオフセットで表し、改行の正規化が影響するのはテンプレート文字列の値だけです。拡張子は `.cdl` で、構文は TSX（TypeScript + JSX）のサブセットです。文末は `;` 必須です（自動セミコロン挿入は曖昧さの元なので採用しません）。

### トークン

| 種類 | 規則 | 例 |
| --- | --- | --- |
| 識別子 | Unicode XID\_Start / `$` / `_` で始まり、XID\_Continue が続く。コンポーネント名は ASCII の `A`–`Z` で始める | `gap`, `Appearance` |
| 数値 | `number` 型。10 進・`0x`・`0b`、`_` 区切り可、小数・指数可。`0o`・先頭が `0` の 8 進表記・BigInt は不可。有限にならないリテラル（`1e400` など）は字句エラー。`NaN` と `Infinity` はリテラルにも名前にも存在しない | `1_000`, `0xff`, `1.5` |
| 文字列 | `"…"` または `'…'`。エスケープは `\n \t \\ \" \' \u{…}`。`\u{…}` は Unicode スカラー値に限る（サロゲート不可） | `"niri"` |
| テンプレート文字列 | バッククォートで囲み、`${式}` で補間する。エスケープは文字列と同じものに `` \` `` と `\$` を加える。タグ付きテンプレートは不可 |  |
| 真偽 | `true` / `false` |  |
| 不在 | `undefined`（型は `T \| undefined`） |  |
| JSX | 要素 `<A />` `<A>…</A>`、フラグメント `<>…</>`。属性値は `"…"` か `{式}`、子は要素か `{式}` | `<Animation enabled />` |

JSX のテキスト子（`<Label>text</Label>`）は使えません。文字列を子に置くときは `{"text"}` と書きます。

JSX の属性文字列（`name="…"`）は JSX と同じく、エスケープを解釈しません（`"a\nb"` は `\` と `n` を含む 4 文字）。エスケープが要るときは `name={"…"}` と書きます。文字参照（`&amp;` など）は使えず、属性文字列に `&` を書くと字句エラーです（`&` を含む文字列は `name={"a & b"}` と書く）。

単位つきの値は専用のリテラルではなく、組み込み関数で作ります（`px(8)`、`ms(120)`、`color("#89b4fa")`。第 4 章）。

`null` は存在しません。値の不在は `undefined` だけで表し、型は `T | undefined` と書きます。

### コメント

`//` は行コメント、`/* … */` はブロックコメント（TypeScript と同じく入れ子不可）です。`/** … */` はドキュメントコメントとして直後の宣言に紐づき、診断とエディタ補完に使われます。

### 予約語

使えるキーワード: `const` `function` `return` `if` `else` `switch` `case` `default` `type` `interface` `import` `export` `from` `satisfies` `true` `false` `undefined`

TypeScript や React にあって CDL にない構文は、予約して使うと診断します: `let` `var` `class` `new` `this` `null` `for` `while` `do` `break` `continue` `async` `await` `yield` `try` `catch` `throw` `enum` `namespace` `as`。React の Hooks、イベントハンドラ属性、`ref`、`key`、Context も存在しません。具体的には、`use[A-Z0-9_]` で始まる名前（`useState` など。`user` は使える）、`on[A-Z]` で始まる属性、`ref` 属性、`key` 属性は、専用の診断になります。

キーワードは、プロパティ名（`{ default: 1 }`、`a.type`、`<X type="…" />`）としては使えます。束縛の名前には使えません。

## 4. 型システム

型は構造的で、評価前にすべて決まります。注釈は TypeScript と同じ書き方で、推論は `const` の初期化式に限った局所推論です。`function`、トップレベルの `const f = (…) => …`、ポリシーのメンバーでは、引数と戻り値の注釈が必須です。組み込みメソッドの引数に直接書いたアロー関数リテラルだけは、引数と戻り値の型を文脈から推論します（`xs.map((x) => …)`）。

| 型 | 表記 | 備考 |
| --- | --- | --- |
| 真偽 | `boolean` |  |
| 数値 | `number` | IEEE 754 binary64。`NaN` と `±Infinity` を生む演算は評価エラー |
| 文字列 | `string` | UTF-8、長さ上限あり（第 7 章） |
| 長さ | `Length` | `px(8)` で作る。内部表現は binary64 の px で、負の値も許す。`L ± L`、`L * n`、`n * L`、`L / n` ができる（`L / L` は不可） |
| 時間 | `Duration` | `ms(120)` / `sec(1)` で作る。内部表現は整数ミリ秒。演算は `Length` と同じで、結果が整数ミリ秒にならなければ評価エラー |
| 色 | `Color` | `color("#89b4fa")` で作る。RGBA 各 8 bit。書式は `#rgb` `#rgba` `#rrggbb` `#rrggbbaa`（大文字小文字は問わない）で、不正な書式は評価エラー。演算は `===` / `!==` だけ |
| 配列 | `T[]` / `Array<T>` | 要素型は単一（ユニオンでもよい）。長さ上限あり。空配列 `[]` は型注釈が必須 |
| タプル | `[A, B]` | 固定長。型注釈か文脈型があるときだけタプルになる |
| レコード | `Record<string, T>` | 型注釈か文脈型が `Record<string, T>` のオブジェクトリテラルで作る。参照は `r["k"]` / `r[k]` だけで、型は `T \| undefined`。キー順は UTF-8 のバイト順で固定（決定性のため） |
| オブジェクト | `{ k: T; … }` / `interface` | 閉じた構造的型。余分なキーは誤り |
| 不在 | `T \| undefined` | オプショナルプロパティ `k?: T` は `k: T \| undefined` の省略可能版 |
| リテラル型 | `"a" \| "b"` | 文字列・数値・真偽のリテラルとユニオン |
| 要素 | `Element<C>` | `C` はホストの組み込みコンポーネント名か、そのユニオン。`<C … />` の型は、`C` の戻り値型になる。フラグメント `<>…</>` の型は、子を平坦化した配列 `Element<…>[]` |

### 規則

- 部分型は、ユニオンの包含（`"a"` は `"a" | "b"` に入る）と、不変なコンテナ（配列・タプル・`Record`・オブジェクトのフィールド・`Element<…>`）の共変性に限る。関数型は不変（同一の型どうしだけ代入できる）。オブジェクトと props は、TypeScript の過剰プロパティ検査を常に適用し、余分なフィールドを許さない（幅部分型はない）。
- ユニオンにできるのは、リテラル型どうし、`T | undefined`、`Element<A | B>` だけ。オブジェクト型どうしのユニオンや `string | number` は書けない。
- 式の型を合わせるとき（`?:` の両枝、`??` の両辺、配列リテラルの要素、推論する戻り値）は、片方がもう片方に置けるなら広いほうの型にし、そうでなければユニオンにする。文脈型のもとで検査したオブジェクトリテラルの型は、文脈型になる。
- プリミティブのリテラルで初期化した `const` はリテラル型を持つ（`const m = "a";` の型は `"a"`）。オブジェクトや配列の中のリテラルは基本型に広げる。文脈型があれば、それを優先する。配列リテラルの要素型は、広げた要素型のユニオンになる。
- プロパティがないことと、値が `undefined` であることは区別しない（`k={undefined}` と明示してもよい）。
- コンポーネントの props と `children` は、ふつうの `interface` / 型リテラルで型付けする。子に置ける要素の型は、ホストのスキーマが宣言する。
- `undefined` を含む型は、`??`・`?.`・`=== undefined` / `!== undefined` で絞り込んでから使う。絞り込めるのは識別子と `a.b.c` の形のパスで、`if`・`?:`・`&&`・`||` の中で効く。`switch` の各 `case` の中では、判定する値がそのリテラル型に絞られる。
- `any`・`unknown`・型アサーション `as`・非 null アサーション `!` はない。使える型の演算子は `satisfies` だけ。
- 型の等価は構造で決まり、`type` と `interface` の別名は区別しない。
- `type` と `interface` の参照が循環する型（再帰型）は書けない（名前解決でエラー）。
- ユーザーはジェネリクスを定義できない。型引数を取るのは組み込みの `Array<T>`・`Record<string, T>`・`Element<C>` だけ。
- 組み込みの関数 `px` `ms` `sec` `color` と、型 `Length` `Duration` `Color` `Element` `Array` `Record` は、import せずに使える。

## 5. 構文と意味論

CDL は TypeScript の宣言と式のサブセットに、JSX を加えたものです。式指向で、再代入・ループ・クラスはなく、束縛はすべて不変（`const`）です。値は宣言順ではなく依存関係の順に評価され、依存が循環していればエラーです。

### 宣言

| 宣言 | 役割 |
| --- | --- |
| `"use cdl 1";` | edition 宣言。ファイルの最初の文として 1 回だけ置く |
| `import { … } from "…";` / `import type { … } from "…";` | 他ファイル・ホスト提供の名前を取り込む（第 9 章） |
| `const name: T = expr;` | 不変の束縛。型注釈は省略可（局所推論） |
| `type Name = T;` / `interface Name { … }` | 型宣言 |
| `function name(a: A): R { … }` / `(a: A): R => …` | 純関数 |
| `function Name(props: P): Element<…> { return <…/>; }` | コンポーネント。名前は ASCII の大文字で始め、props を 1 つだけ受け取る。戻り値型は `Element<…>` に限る。JSX からだけ使え、関数として呼べない |
| `export` | 他ファイルへ公開する |
| `export default <…/>;` | ルート要素の宣言（第 6 章） |
| `export const policy = { … } satisfies Policy;` | ポリシーの宣言（第 8 章） |

関数とコンポーネントは、再帰も相互再帰もできません。トップレベルの宣言どうしの参照（呼び出しに限らず、名前が現れるすべての箇所）は循環してはいけません。

トップレベルの宣言は順序を問いません。ブロックの中の `const` は、前に宣言した名前だけを参照できます。名前の隠蔽（組み込みの名前・import した名前・外側のスコープの名前と同じ名前を、内側で宣言すること）はできません。

### 式

- 算術 `+ - * / %` と単項の `-`、比較 `=== !== < <= > >=`（`==` と `!=` はない）、論理 `&& || !`、`??`
  - `&& || !` のオペランドは `boolean` だけ（真とみなせる値による判定はない）
  - `+ - < <= > >=` は `number`・`Length`・`Duration` に使う（同じ単位どうし）。文字列の連結は、テンプレート文字列で書く
  - `=== !==` は、プリミティブ・リテラル・単位つきの値・`undefined` に使う。オブジェクト・配列・要素どうしや、重なりのない型どうしは比べられない
  - `%` の結果の符号は被除数に合わせる（JS と同じ）
  - `**` とビット演算はない
- 条件演算子 `c ? a : b`（条件は `boolean`。両枝の型は、上の規則で合わせる）
- 関数本体では `const` と、全経路が `return` で終わる `if` / `else` と `switch` が書ける（`else` は省略できない。`if` の条件は `boolean`）
- `switch (x) { case "a": return …; }`（1 つの `case` に文は 1 つで、フォールスルーはない。`case` の重複はエラー。リテラルのユニオン型では網羅性を検査し、`default` は省略できる。それ以外の型では `default` が必須）
- テンプレート文字列 `${…}`: 補間できるのは `string` と `number` だけ。`number` は JS の Number→String と同じ規則で文字列にする
- 配列の操作は、組み込みの有界なメソッドだけ: `map((x) => U)` `filter((x) => boolean)` `reduce((acc, x) => A, init)`（初期値は必須） `includes(x)` `concat(xs)`、プロパティの `length`。`Record` と文字列にメソッドはない
  - `map` の結果は、コールバックの戻り値の型を広げた型の配列（`xs.map((x) => "a")` は `string[]`）
  - `reduce` の累積値の型は、初期値の型を広げた型（`reduce(…, 0)` は `number`）で、コールバックはその型の値を返す
  - `filter` の結果は、元の要素型の配列（タプルにも使えるが、結果は配列になる）
- プロパティ参照 `a.b`、`a?.b`、添字 `a[i]`（型は `T | undefined`。範囲外・負・整数でない添字は `undefined`。タプルを範囲内のリテラルで引いたときは要素の型そのもの）、スプレッド `...`

JSX の式:

- 要素 `<Name a="x" b={expr} flag>…</Name>` と自己閉じ `<Name />`。真偽の属性は `flag` と省略できる
- フラグメント `<>…</>`。属性のスプレッド `<Name {...props} />`
- スプレッドとほかのキー・属性が重なったら、あとに書いたものが勝つ（オブジェクトリテラルでも同じ）。明示のキーどうしの重複は誤り。値が `undefined` のフィールドのスプレッドは、前の値を上書きしない
- 条件は `c ? <A /> : undefined`。`undefined` の子は無視される。`c && <A />` は `false` が子になるので使えない
- 繰り返しは `xs.map((x) => <Item … />)`。配列の子は、自動で平坦化される
- `{…}` の中は純粋な式だけで、イベントハンドラ属性や `ref` はない

0 除算は評価エラーです。浮動小数は IEEE 754 binary64 の丸めに従い、`NaN` や `±Infinity` を生む演算も評価エラーです。

TypeScript の構文のうち、次のものは書けません: 分割代入（引数を含む）、`enum`、import の改名、既定引数と省略可能な引数、計算プロパティ名、メソッド構文、`interface … extends`、交差型、`keyof` などの型演算子、正規表現、BigInt、カンマ演算子、`export { … }` と再エクスポート、ブロックの中の関数宣言、関数呼び出しの引数のスプレッド（`f(...xs)`）。省略プロパティ `{ keys }` と、任意の式の位置の `satisfies` は書けます。

### 文法（抜粋, EBNF）

```ebnf
file      = directive { item } ;
directive = STRING ";" ;            (* 値は "use cdl N" に限る *)
item      = import | export | decl ;
import    = "import" [ "type" ] "{" names "}" "from" STRING ";" ;
export    = "export" ( decl | "default" expr ";" ) ;
decl      = const | typedecl | function ;
const     = "const" IDENT [ ":" type ] "=" expr ";" ;
function  = "function" IDENT "(" [ params ] ")" ":" type block ;
arrow     = "(" [ params ] ")" [ ":" type ] "=>" ( expr | block ) ;
block     = "{" { const } stmt "}" ;
stmt      = "return" expr ";" | if | switch ;
if        = "if" "(" expr ")" block "else" ( block | if ) ;
switch    = "switch" "(" expr ")" "{" { case } [ default ] "}" ;
case      = "case" literal ":" stmt ;
expr      = binary [ "?" expr ":" expr ] | jsx | frag | "(" expr ")" ;
jsx       = "<" NAME { attr } ( "/>" | ">" { child } "</" NAME ">" ) ;
frag      = "<>" { child } "</>" ;
attr      = IDENT [ "=" ( STRING | "{" expr "}" ) ] | "{" "..." expr "}" ;
child     = jsx | frag | "{" [ expr ] "}" | TEXT ;   (* 中身が空白とコメントだけの {} は子にならない *)
```

オブジェクト・配列・引数リストは末尾カンマを許容します。TypeScript と同じく、オブジェクトリテラルの区切りはカンマ、`interface` と型リテラルの区切りは `;` か `,` です。

## 6. ドキュメントモデル

ホストが読み込むエントリファイルは、ちょうど 1 つの `export default`（ルート要素）を持ちます。評価すると、コンポーネントがすべて展開された、不変の要素ツリー（ドキュメント）になります。各ノードは、その要素を作った JSX の位置（span）と、展開の途中で通ったコンポーネント呼び出しの位置の列を保持するので、ホストは「どの行の設定か」を診断に使えます。ライブラリとして読まれるファイルは `export default` を持てません。

### スキーマ

スキーマは、ホストが提供する組み込みコンポーネントの宣言で、`import { Contrast, Appearance } from "host:contrast";` のように取り込みます。各コンポーネントは、props の型と、子に許す要素の型を持ちます。ルート要素は、ホストが指定したコンポーネント（ここでは `Contrast`）でなければなりません。CDL 内で書いたコンポーネントも、組み込みと同じ規則で型検査されます。

スキーマはホストが Rust の API で登録します。CDL の中で `declare` を書くことはできません。次の `declare function` は、ホストが登録した内容を CDL から見た型として説明するための表記です。

```ts
// host:contrast が公開する組み込みコンポーネントを、CDL 側から見た型
export declare function Contrast(props: {
  children?: Element<Appearance | Animation | Keybind>[];
}): Element<Contrast>;
export declare function Appearance(props: {
  gap: Length; radius: Length; focusColor?: Color;
}): Element<Appearance>;
export declare function Animation(props: {
  duration: Duration; enabled?: boolean;
}): Element<Animation>;
export declare function Keybind(props: {
  keys: string; action: string;
}): Element<Keybind>;
```

### 要素とプロパティ

- props は、`name={expr}`、`name="text"`、真偽値の省略形 `flag` で書く。
- props は閉じた型。省略できるのは `?` 付きのものだけで、それ以外の欠落と余分な props は診断される。
- 子は、平坦化して `undefined` を取り除いた配列として渡される。`children` の型は `Element<…>[]`（省略可能にしてよい）。`children` 属性と JSX の子は同時に書けない。
- 子に置けるのは、ホストが宣言した型の要素だけ。型の違う子は診断される。
- ホストが一意と宣言した子（例: `Appearance`）の重複はエラー（後勝ちにしない）。`map` が作る子は静的に数えられないので、評価後の要素ツリーで検査する。
- 値が `undefined` の props は、要素ツリーに含めない。
- 値が関数の props は、要素ツリーに置けない。
- ツリーは評価後に不変で、ホストは旧ツリーとの差分を取ってリロードできる。

### 例

```tsx
"use cdl 1";
import { Contrast, Appearance, Animation, Keybind } from "host:contrast";

const BASE = px(8);

const binds = [
  { keys: "mod+Return", action: "spawn terminal" },
  { keys: "mod+q", action: "close" },
];

export default (
  <Contrast>
    <Appearance gap={BASE * 2} radius={BASE} focusColor={color("#89b4fa")} />
    <Animation duration={ms(120)} enabled />
    {binds.map((b) => <Keybind keys={b.keys} action={b.action} />)}
  </Contrast>
);
```

## 7. 評価モデル

評価は、パース → 名前解決 → 型検査 → 評価の 4 段階で行い、前段に誤りがあれば後段へは進みません（診断は段階をまたいで蓄積されます）。次の段階に進むかどうかは、全モジュールを合わせたプログラム全体で判断します（すべてのファイルの構文解析 → import の解決 → すべてのファイルの名前解決 → 依存先から順に型検査 → 評価）。`warning` では止まりません。トップレベルの `const` は、参照されていなくても評価します。順序は参照グラフのトポロジカル順で、順位が同じものはソースの順です。評価器は I/O を持たず、入力はソースとホスト提供の読み取り専用値だけです。

### 停止性の保証

- トップレベルの宣言の参照グラフは DAG でなければならない。辺は呼び出しに限らず、名前が現れるすべての箇所。再帰・相互再帰、コンポーネントが自分自身を子に置くことは、名前解決でエラー。
- 再帰型を書けない。参照グラフが DAG であることと合わせて、関数を値として渡しても評価は必ず停止する。
- ループ構文はなく、反復は `map` / `filter` / `reduce` などの有界な組み込みだけ。
- 上記に加えて、実行時にリソース上限を強制する（多重防御）。

### リソース上限

上限を超えると、評価はその場で打ち切られ、`E5xxx` の診断になります。位置は、上限を超えたときに評価していたトップレベルの宣言（`const` の名前、`export default` の式、フックの呼び出しなら `policy` の名前）です。0 除算などの評価の誤りは、その演算の位置で報告します。値はホストが `Limits` で上書きできます（既定値は暫定）。

| 項目 | 既定値 | 対象 |
| --- | --- | --- |
| 評価ステップ | 1,000,000 | Document 全体の評価 |
| 呼び出し深さ | 64 | 関数のネスト |
| 文字列長 | 64 KiB | 1 値あたり（UTF-8 のバイト数） |
| 配列・マップ要素数 | 65,536 | 1 値あたり |
| 値木ノード数 | 1,000,000 | 評価結果全体 |
| ソースサイズ | 1 MiB | 1 ファイル |
| 構文ネスト深さ | 64 | 括弧・JSX・前置演算子・三項演算子の入れ子と、二項演算子・`.`・`=>`・`if` の連鎖（連鎖は同じ括弧の中で数え、`,`・`;`・トップレベルに戻る `}` で数え直す。1 ファイルあたり。超過は `E1xxx`。パースの再帰を有界にする） |
| 値木の深さ | 64 | 評価結果全体（型検査・評価・ホスト変換の再帰を有界にする）。超過は `E5011` |

評価ステップは、式を 1 つ評価するごと、文（`return`・`if`・`switch`）を 1 つ実行するごと、組み込みメソッドが要素を 1 つ処理するごと、`concat` とスプレッドが要素を 1 つ写すごとに 1 つ数えます。呼び出し深さは、関数・コンポーネント・コールバックの呼び出しを数えます。値木ノード数は、要素・配列・オブジェクト・葉のすべてを数えます。値木の深さは、要素・配列・オブジェクトを 1 段と数え、葉は数えません（`<Root />` は 1、`[[1]]` は 2）。値を作った時点で検査するので、`const` を連ねて構文ネスト深さの上限を抜けても、上限を超えた値は作れません。ホストがフックに渡す引数にも、同じ上限を適用します。

### 決定性

- `number` は IEEE 754 binary64 の丸めに従い、`NaN` や `±Infinity` を生む演算（0 除算を含む）はエラー。演算の結果の `-0` は `+0` に正規化する。
- `Record` は UTF-8 のバイト順で保持し、ハッシュ順に依存しない。
- 組み込み関数に、環境・時刻・ロケールへの依存はない。

## 8. ポリシーとイベントフック

ポリシーは、ホストが「判断が必要な瞬間」に呼ぶ純関数です。CDL が決めるのは判定の結果だけで、実際の操作（ウィンドウの配置など）はホストが行います。

```ts
"use cdl 1";
import type { Policy, WindowInfo, Placement } from "host:contrast";

export const policy = {
  windowOpen: (w: WindowInfo): Placement => {
    switch (w.appId) {
      case "firefox":
        return { workspace: 2, floating: false };
      case "pavucontrol":
        return { workspace: w.workspace, floating: true };
      default:
        return { workspace: w.workspace, floating: false };
    }
  },
} satisfies Policy;
```

### 規則

- `policy` の各メンバー（フック）の引数型と戻り値型は、ホストのスキーマ `Policy` が宣言したものと完全一致しなければならない（`satisfies` で検査する。関数型は不変なので、代入できることと完全一致は同じ意味になる）。
- フックはすべて省略できる。`Policy` にないフックは診断される。
- `export const policy` を置けるのはエントリファイルだけで、`satisfies Policy` は必須。
- ホストから渡された引数の値がフックの引数型に合わなければ、診断を返す。
- 引数は読み取り専用の値で、ポリシーが保持する状態はない（呼び出し間で記憶しない）。
- 呼び出しごとに専用のステップ上限（既定 10,000）を適用し、第 7 章の上限とは別に数える。
- 上限超過・評価エラー時は、そのフックのホスト既定動作に戻す。ホストはエラーを診断として受け取る。
- `export const policy` は 1 ファイルに 1 つ。同じフックを複数定義できないので、合成が必要なら 1 つの関数にまとめる。

### ホットリロード

ホストは新しい版を評価して成功した場合だけ、旧版と差し替えます。型検査か評価に失敗したら旧版を保持し、診断だけを報告します。ポリシーとドキュメントの差し替えは、同じ世代番号で原子的に行います。CDL の実装は、ドキュメントとポリシーを 1 つの不変な値として返します。世代番号と差し替えはホストが管理します。

### 境界

フックの一覧と、引数・戻り値の型は CDL の仕様ではなく、各ホストのスキーマの責任です。Contrast が定義するフックは、Contrast 側の仕様書で管理します。

## 9. モジュールとインポート

複数ファイルに分割できますが、依存は静的に解決でき、循環しないものに限ります。

```tsx
import { BASE, Panel } from "./theme.cdl";
import { Contrast } from "host:contrast";
```

| 構文 | 解決先 | 規則 |
| --- | --- | --- |
| `from "./path.cdl"` | 他の `.cdl` ファイル | `./` か `../` で始まり `.cdl` で終わる相対パスのみ。絶対パス・URL・拡張子のないものは不可（`E2xxx`） |
| `from "host:name"` | ホストが公開するモジュール | ホストの登録した型・組み込みコンポーネント・読み取り専用の定数だけ |

- インポートできるのは `export` を付けた `const` / `type` / `interface` / `function` だけ。`export default` はインポートできない。
- `import` は名前の値と型の両方を取り込み、`import type` は型だけを取り込む。
- ライブラリとして読まれるファイルは、`export default` と `export const policy` を持てない。
- インポートグラフが循環していたら、全ファイルをまとめてエラーにする。
- 同じファイルは評価 1 回につき 1 度だけ評価し、結果を共有する。
- パッケージ管理と外部レジストリは持たない。

### ファイルアクセスの抽象化

コア実装は `ModuleResolver` トレイトを通してのみソースを読みます。リゾルバは、読み込み元の ID と指定子を受け取り、正準 ID とソースを返します。同じファイルかどうかは、正準 ID で判断します。ファイルシステムの実装はホスト側（または CLI）が提供するため、コアはサンドボックスや WASM でもそのまま動きます。`..` による範囲外参照を許すかどうかも、リゾルバ側の責任です。

## 10. 診断とエラーモデル

すべての誤りは、安定したコード・重大度・位置・メッセージを持つ診断として返され、コアはパニックも `abort` もしません。パーサは同期点（`;` と `}`）で回復し、1 回の実行で複数の誤りを報告します。

### 診断の構造

| フィールド | 内容 |
| --- | --- |
| `code` | `E0001` のような安定コード。意味を変える場合は新コードを振る |
| `severity` | `error` / `warning` / `hint` |
| `span` | ファイル・開始と終了のバイトオフセット（元のソース上。行・列は表示時に計算） |
| `message` | 人間向けの 1 文 |
| `labels` | 関連する別の位置（例: 重複定義の元の場所）と短い注記 |
| `help` | 修正案（任意） |

### コード帯

| 帯 | 段階 | 例 |
| --- | --- | --- |
| E0xxx | 字句 | 閉じていない文字列、不正なエスケープ |
| E1xxx | 構文 | 期待したトークンがない、構文ネスト深さの超過 |
| E2xxx | 名前解決 | 未定義の名前、循環参照、再帰呼び出し |
| E3xxx | 型検査 | 型不一致、未知のキー、`switch` の網羅性不足 |
| E4xxx | スキーマ適合 | 組み込みコンポーネントの props の欠落・余分・型違い、子の型違い、ルート要素の違い、一意な子の重複、`Policy` にないフック |
| E5xxx | 評価 | リソース上限超過、0 除算、オーバーフロー |

E3 と E4 の境界: ホストの組み込みコンポーネントと `Policy` についての検査は E4、それ以外のオブジェクト型の検査（CDL 内で書いたコンポーネントや型を含む）は E3 です。

### 出力形式

診断は、人間向け（ソース抜粋と下線つき）と機械向け（JSON）の両方でレンダリングできます。JSON 形式は互換性の対象で、フィールドの追加は許されますが、削除と意味変更は edition を上げない限りしません。

```json
{ "code": "E3001", "severity": "error",
  "span": { "file": "main.cdl", "start": 120, "end": 128 },
  "message": "…", "labels": [ { "span": { … }, "message": "…" } ], "help": "…" }
```

診断は (ファイル, 開始, 終了, コード) の順に並べて返します。コードの一覧は `docs/DIAGNOSTICS.md` に置き、実装の定義と一致させます。

## 11. 実装アーキテクチャ

CDL は独立リポジトリのクレート群として実装し、Contrast は公開 API を持つ `cdl-host` だけに依存します。

```
cdl-diag ← cdl-syntax ← cdl-sema ← cdl-eval ← cdl-host ← cdl-cli
                                                  ↑
                                               Contrast
```

`cdl-host` が、Contrast との唯一の接点です。

- 公開 API の互換性（semver）を約束するのは `cdl-host` だけで、他のクレートは内部実装として自由に変えられる。
- コアのクレートは、ファイルシステム・環境変数・時刻に触れない。必要な入力は、すべて `cdl-host` の引数から渡される。
- ホストは、評価結果の値木を、スキーマに対応する Rust の型へ変換して使う。変換に失敗したら、診断として返る。
- CDL のプロトタイプ段階に限り、`oxc_parser` で構文を読み、CDL が許す範囲外の構文を診断で弾く構成を許す。本実装では `cdl-syntax` の自前パーサに置き換え、適合テストスイート（第 12 章）で同じ結果を保証する。
- Contrast 側の仕様（フック一覧、スキーマ、リロード時の挙動）は Contrast のリポジトリで管理し、CDL の仕様書には書かない。

## 12. 検証とテスト方針

書いたコードを後から書き直さないことを目標に、仕様の主張ごとに、それを守る検証手段を 1 つ以上割り当てます。

| 主張 | 守る手段 |
| --- | --- |
| 任意入力でパニックも abort もしない | clippy の deny（unwrap・expect・panic・添字・算術・縮小キャスト）、ネスト深さの上限、proptest（任意の文字列を入力） |
| パーサ・評価器は必ず停止する | 設計（呼び出しグラフは DAG、反復は有界な組み込みのみ、評価ステップ上限）、上限ちょうどと上限 + 1 の境界テスト、proptest（任意入力で完了する） |
| 整数演算にラップ・未定義動作がない | `unsafe_code = "forbid"`、clippy の `arithmetic_side_effects` を deny（checked 演算のみ） |
| `fmt(parse(x))` は冪等で、意味を変えない | proptest（任意の AST から生成） |
| 型検査を通った式は評価で型エラーにならない（健全性） | proptest（型付き式ジェネレータ）と、参照モデルとの差分テスト |
| 診断のコード・位置が回帰しない | ゴールデンテスト（`.cdl` と期待 JSON） |
| 同じ入力は常に同じ出力 | 実行を 2 回行う差分テスト、clippy の `disallowed_types` / `disallowed_methods`（HashMap・時刻・環境・乱数を禁止） |
| リソース上限が効く | 各上限（第 7 章）の境界テスト（ちょうどなら成功、+ 1 なら診断） |

### 実装ルール

- コアのクレートは `unsafe` を禁止する（ワークスペースの lint 設定 `unsafe_code = "forbid"`）。
- ワークスペースの lint で、clippy の `unwrap_used` `expect_used` `panic` `todo` `unimplemented` `unreachable` `indexing_slicing` `arithmetic_side_effects` と、縮小・符号変更のキャスト（`cast_possible_truncation` `cast_possible_wrap` `cast_sign_loss`）を deny にする。テストコードだけは clippy.toml で許す。
- 決定性のため、clippy.toml の `disallowed-types` で `HashMap` / `HashSet` を、`disallowed-methods` で時刻・環境変数・乱数の取得を禁止する。
- パーサ・型検査・評価器の再帰は、ネスト深さの上限（第 7 章）の内側でだけ行う。パーサの繰り返しは、入力を 1 トークン以上進めるか、終了する。
- 公開 API は `Result` のみを返し、診断は常に値として扱う。
- 言語仕様の変更は、先に適合テスト（`.cdl` と期待結果の組）を足してから実装する。

### 採らない手段

この仕様の検証には、Kani と cargo-fuzz を使いません（開発環境で実行できないため）。cargo-mutants と形式証明系は、この段階では採りません。Miri は、`unsafe` を禁止しているので不要です。したがって、上の表の主張は証明されたものではなく、静的な lint と設計上の制約とテストで守るものです。

### 検証の実行段階

- 日常: 変更した crate だけを `cargo test -p` / `cargo clippy -p` にかけ、`cargo fmt --check` を行う。proptest の回数は少なめにする（64）。
- 区切り（マージ・リリースの前）: ワークスペース全体の test と clippy。proptest は既定の回数（256）で行う。
- 重い実行は、メモリ上限をかけて行う。

### 適合テストスイート

`conformance/` に、言語仕様の節番号と対応する `.cdl` と期待結果（値木または診断の JSON）を置きます。値木の JSON では、要素を `{"element": "Name", "props": {…}, "children": […]}`、単位つきの値を `{"length": 16}` `{"duration": 120}` `{"color": [r, g, b, a]}`、配列の中の `undefined` を `{"undefined": true}` と書き、`number`・`string`・`boolean` はそのまま書きます。値木の比較では span を比べず、診断の比較では span を比べます。実装に依存しない形式なので、将来の別実装（例: Contrast のプロトタイプ用パーサ）が同じスイートで検証できます。

## 13. バージョニングと互換性

互換性の単位は edition で、すべてのファイルの最初の文に置く `"use cdl 1";` が必須です。文字列は厳密に一致させます（`use cdl` と 10 進の正の整数のあいだに空白 1 つ。引用符はどちらでもよい）。edition が同じなら、古いファイルは新しい実装でも同じ意味で動きます。

| 変更の種類 | 扱い |
| --- | --- |
| 新しい組み込み関数・型の追加 | 同一 edition で可（既存名と衝突しないものだけ） |
| 予約語の追加 | 事前に予約し、使用を警告してから有効化する |
| 構文・意味の変更、削除 | 新しい edition でのみ行う |
| 診断コードの意味変更 | 禁止（新コードを振る） |

- ホストは、対応する edition の範囲を実装側に問い合わせ、範囲外のファイルは E1xxx で拒否する。
- 非推奨は 1 edition のあいだ `warning` として残し、次の edition で削除する。
- スキーマのバージョンは言語の edition とは独立で、ホストが管理する。
- 仕様書が `0.x` のあいだは、edition 1 は不安定で、互換性の約束はまだない。
- 移行用に、CLI が `cdl migrate` で機械的に書き換えられる変更だけを、edition 間の破壊的変更として許す。

## 14. 付録

### A. 設定例（コンポーネントと undefined を使う）

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

### B. 未決事項

決めないと実装に進めない項目を、影響の大きい順に並べます。

- [x] 子の型付け: 子に許す要素の型を `Element<A | B>[]` と書く現案でよいか → 現案（DECISIONS B1・B5・B6）
- [x] JSX のテキスト子（`<Label>text</Label>`）を許すか、`{"text"}` だけにするか → `{"text"}` だけ（A1）
- [x] 要素の同一性: `key` を導入するか → 導入しない（C4）
- [x] 条件描画: `c ? <A /> : undefined` に加えて `c && <A />` を許すか → 許さない（A2）
- [x] スキーマの定義元: ホストに一本化するか、CDL 内の `declare` も許すか → ホストの Rust API に一本化（E1）
- [x] 単位型を関数（`px(8)`）で作る現案でよいか → 現案（G1）
- [x] TypeScript の構文のうち、どこまで許すか → 第 5 章に一覧（C1）
- [x] `number` を binary64 のみにするか → binary64 のみ（G2）
- [x] edition 宣言を `"use cdl 1";` にするか → 現案（C8）
- [x] 診断コード表の持ち方 → `docs/DIAGNOSTICS.md`（F1）
- [x] 検証手段: lint・設計の制約・proptest・境界テストで守る。Kani と cargo-fuzz は採用しない
- [x] ネスト深さの上限: 構文 64（E1xxx）・値木 64（暫定、ホストが上書き可）
- [ ] `fmt` の規約、LSP の範囲と時期（プロトタイプの対象外）
