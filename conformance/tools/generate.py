"""適合テストスイート（conformance/）を生成する。

使い方: python3 -I conformance/tools/generate.py [出力先]（既定は conformance/）

各テストのソースと期待結果を、このスクリプトの中に書く。期待する診断の span は、
実装の出力からではなく、ソースの中の文字列を探して求める（実装とは独立のオラクル）。
テストを足すときは、ここに足して実行し、生成した差分を確かめてからコミットする。
出力は決定的で、同じ入力からはいつも同じファイルを作る。

配置（CLAUDE.md の「conformance の置き方」）: テスト名 `<章>-<名前>` は `<章>/<名前>.cdl` に置く。
禁止構文の id（FORBIDDEN）は `forbidden/<id>.cdl`。複数ファイルのテストは `<章>/<名前>/` に置く。
"""
import json, os, sys

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
HEAD = '"use cdl 1";\nimport { Contrast, Keybind } from "host:contrast";\n'
BS = chr(92)

# 禁止構文の id（CLAUDE.md）。`forbidden/<id>.cdl` に置く。
FORBIDDEN = {
    "3-forbidden-" + i: i
    for i in "let var class new this null for while try async enum as hooks event-handler ref "
             "destructuring rest-param and-render".split()
}
FORBIDDEN["5-jsx-text-child"] = "text-child"

def rel_of(name):
    """テスト名 -> 配置先（拡張子なし、OUT からの相対）。"""
    if name in FORBIDDEN:
        return "forbidden/" + FORBIDDEN[name]
    chapter, rest = name.split("-", 1)
    return chapter + "/" + rest

def el(name, props=None, children=None):
    return {"element": name, "props": props or {}, "children": children or []}

def kb(keys, action):
    return el("Keybind", {"keys": keys, "action": action})

def root(*children):
    return el("Contrast", {}, list(children))

def find_span(src, spec):
    """spec: needle | (context, needle) | (context, needle, "end") | ("@", offset, length) | ("eof", needle)"""
    b = src.encode()
    if isinstance(spec, str):
        spec = (spec, spec)
    if spec[0] == "@":
        return spec[1], spec[1] + spec[2]
    if spec[0] == "eof":
        i = b.index(spec[1].encode())
        return i, len(b)
    ctx, needle = spec[0].encode(), spec[1].encode()
    occurrence = spec[3] if len(spec) > 3 else 1
    i = -1
    for _ in range(occurrence):
        i = b.index(ctx, i + 1)
    # 探す文字列が文脈の中にあれば、文脈の中で最後に現れる位置にする（`function f` の `f` など）。
    j = i + ctx.rfind(needle) if needle in ctx else b.index(needle, i)
    if len(spec) > 2 and spec[2] == "end":
        return j + len(needle), j + len(needle)
    return j, j + len(needle)

def write(name, src, value=None, diags=None, crlf=False):
    if crlf:
        src = src.replace("\n", "\r\n")
    rel = rel_of(name)
    os.makedirs(os.path.join(OUT, os.path.dirname(rel)), exist_ok=True)
    path = os.path.join(OUT, rel + ".cdl")
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(src)
    if diags is not None:
        items = []
        for code, spec in diags:
            start, end = find_span(src, spec)
            items.append({"code": code, "severity": "error", "span": {"file": rel + ".cdl", "start": start, "end": end}})
        expected = {"diagnostics": items}
    else:
        expected = {"value": value}
    with open(os.path.join(OUT, rel + ".expected.json"), "w", encoding="utf-8") as f:
        json.dump(expected, f, ensure_ascii=False, indent=2)
        f.write("\n")

def program(comment, body_children, before=""):
    return f"// {comment}\n{HEAD}{before}\nexport default (\n  <Contrast>\n{body_children}  </Contrast>\n);\n"

# ---- §3 受理 ----------------------------------------------------------------
write("3-string-escapes",
      program("§3 文字列: エスケープ \\n \\t \\\\ \\\" \\' \\u{…} を受け付ける。",
              '    <Keybind keys={"a' + BS + 'nb' + BS + 'tc' + BS + BS + 'd' + BS + '"e' + BS + "'f\"} action={'" + BS + "u{41}" + BS + "u{1F600}'} />\n"),
      value=root(kb("a\nb\tc\\d\"e'f", "A\U0001F600")))
write("3-template-escapes",
      program("§3 テンプレート文字列: \\` と \\$ のエスケープ、${式} の補間（DECISIONS C9）。",
              "    <Keybind keys={`a" + BS + "`b" + BS + "${c}`} action={`x${\"y\"}z`} />\n"),
      value=root(kb("a`b${c}", "xyz")))
write("3-numeric-literals",
      program("§3 数値: 10 進・0x・0b・_ 区切り・小数・指数。補間は JS の Number→String と同じ（DECISIONS D1）。",
              "    <Keybind keys={`${1_000} ${0xff} ${0b101} ${.5} ${2.} ${1e3} ${1.5e-3}`} action=\"n\" />\n"),
      value=root(kb("1000 255 5 0.5 2 1000 0.0015", "n")))
write("3-crlf",
      program("§3 改行: CRLF を受け付け、テンプレート文字列の値では LF に正規化する（DECISIONS C9）。",
              "    <Keybind keys={`a\nb`} action=\"x\" />\n"),
      value=root(kb("a\nb", "x")), crlf=True)
write("3-comments",
      program("§3 コメント: 行・ブロック・ドキュメントコメントと、JSX の子の {/* … */}（DECISIONS C14）。",
              "    {/* 子のコメント */}\n    <Keybind /* タグの中 */ keys={k} action=\"y\" />\n",
              before="// 行コメント\n/* ブロック */\n/** ドキュメント */\nconst k = \"x\"; // 行末\n"),
      value=root(kb("x", "y")))
write("3-jsx-attr-string-raw",
      program("§3 JSX の属性文字列: エスケープを解釈しない（DECISIONS C13）。",
              "    <Keybind keys=\"a" + BS + "nb\" action='it\"s' />\n"),
      value=root(kb("a\\nb", 'it"s')))
write("3-keyword-property-names",
      program("§3 予約語: キーワードはプロパティ名に使える（DECISIONS C3）。",
              "    <Keybind keys={o.default} action={o.type} />\n",
              before='const o = { default: "d", type: "t" };\n'),
      value=root(kb("d", "t")))

# ---- §3 字句の誤り ------------------------------------------------------------
src = '// §3 文字列: 閉じていない文字列は E0001。\n"use cdl 1";\nconst s = "abc;\nconst t = 1;\n'
write("3-unterminated-string", src, diags=[("E0001", '"abc;')])

src = ('// §3 文字列: 一覧にないエスケープとサロゲートは E0002（DECISIONS C9）。\n"use cdl 1";\n'
       f'const a = "{BS}x41";\nconst b = "{BS}0";\nconst c = "{BS}u0041";\nconst d = "{BS}u{{D800}}";\n'
       f'const e = "{BS}u{{110000}}";\nconst f = `{BS}a`;\n')
write("3-invalid-escape", src, diags=[("E0002", BS + "x"), ("E0002", BS + "0"), ("E0002", BS + "u0"),
                                      ("E0002", BS + "u{D800}"), ("E0002", BS + "u{110000}"), ("E0002", BS + "a")][:2]
      + [("E0002", ("const c", BS + "u")), ("E0002", BS + "u{D800}"), ("E0002", BS + "u{110000}"), ("E0002", BS + "a")])

src = '// §3 テンプレート文字列: 閉じていないものは E0003。\n"use cdl 1";\nconst t = `abc ${1}\nconst u = 2;\n'
write("3-unterminated-template", src, diags=[("E0003", ("eof", "`abc"))])

src = '// §3 コメント: 閉じていないブロックコメントは E0004。\n"use cdl 1";\n/* 閉じない\nconst a = 1;\n'
write("3-unterminated-comment", src, diags=[("E0004", ("eof", "/* 閉じない"))])

src = ('// §3 数値: 0o・先頭 0・BigInt・区切りの誤り・正規表現は E0005（DECISIONS C9）。\n"use cdl 1";\n'
       'const a = 0o17;\nconst b = 017;\nconst c = 1n;\nconst d = 1__0;\nconst e = /ab+c/g;\n')
write("3-unsupported-literals", src, diags=[("E0005", "0o17"), ("E0005", ("const b", "017")), ("E0005", ("const c", "1n")),
                                             ("E0005", "1__0"), ("E0005", "/ab+c/g")])

src = '// §3 数値: 有限にならないリテラルは E0006（DECISIONS C9）。\n"use cdl 1";\nconst a = 1e400;\n'
write("3-non-finite-literal", src, diags=[("E0006", "1e400")])

src = ('// §3 ファイル形式: 単独の CR・U+2028・文字列の外の \\ は E0007（DECISIONS C9）。\n"use cdl 1";\n'
       'const a = 1;\rconst b = 2;\nconst c  = 3;\nconst ' + BS + 'u0064 = 4;\n')
write("3-forbidden-characters", src, diags=[("E0007", "\r"), ("E0007", " "), ("E0007", ("const " + BS, BS))])

src = '﻿// §3 ファイル形式: BOM は E0007（DECISIONS C9）。\n"use cdl 1";\n'
write("3-bom", src, diags=[("E0007", "﻿")])

src = program("§3 JSX の属性文字列: 文字参照は使えないので、& は E0008（DECISIONS C18）。",
              '    <Keybind keys="a &amp; b" action="x" />\n')
write("3-jsx-attr-char-ref", src, diags=[("E0008", ("a &amp;", "&"))])

# ---- §3 予約語・React --------------------------------------------------------
src = ('// §3 予約語: 予約語の構文と、予約語を名前に使うことは E1006。\n"use cdl 1";\n'
       'let a = 1;\nvar b = 2;\nclass C {}\nenum E { A }\nconst c = null;\nconst d = this;\nconst as = 1;\n'
       'import { x as y } from "./m.cdl";\n')
write("3-reserved-words", src, diags=[("E1006", "let a = 1;"), ("E1006", "var b = 2;"), ("E1006", "class C {}"),
                                       ("E1006", "enum E { A }"), ("E1006", "null"), ("E1006", "this"),
                                       ("E1006", ("const as", "as")), ("E1006", "x as y")])

src = program("§3 React: Hooks の名前・ref・key・イベントハンドラ属性は E1007（DECISIONS C4）。",
              '    <Keybind keys="a" action="b" ref={k} key="1" onPress={k} />\n',
              before="const useTheme = 1;\nconst k = useMemo;\n")
write("3-react-features", src, diags=[("E1007", "useTheme"), ("E1007", "useMemo"), ("E1007", ("ref={", "ref")),
                                       ("E1007", ('key="1"', "key")), ("E1007", "onPress")])

src = ('// §3 文末: `;` がなければ E1002（自動セミコロン挿入はしない）。\n"use cdl 1"\n'
       'import { Contrast } from "host:contrast"\nconst a = 1\ntype T = number\nexport default (<Contrast />)\n')
write("3-semicolons", src, diags=[("E1002", ('"use cdl 1"', '"use cdl 1"', "end")),
                                   ("E1002", ('"host:contrast"', '"host:contrast"', "end")),
                                   ("E1002", ("const a = 1", "const a = 1", "end")),
                                   ("E1002", ("type T = number", "type T = number", "end")),
                                   ("E1002", ("(<Contrast />)", "(<Contrast />)", "end"))])

# ---- §13 edition --------------------------------------------------------------
write("13-edition-missing", "// §13 edition: 宣言がなければ E1003。\nconst a = 1;\n", diags=[("E1003", ("@", 0, 0))])
src = '// §13 edition: 形が違えば E1003（DECISIONS C8）。\n"use cdl";\n'
write("13-edition-malformed", src, diags=[("E1003", '"use cdl";')])
src = '// §13 edition: 対応していない edition は E1004。\n"use cdl 2";\n'
write("13-edition-unsupported", src, diags=[("E1004", '"use cdl 2";')])
src = '// §13 edition: 宣言は最初に 1 回だけ（2 つめは E1003）。\n"use cdl 1";\n"use cdl 1";\n'
write("13-edition-duplicate", src, diags=[("E1003", ('"use cdl 1";', '"use cdl 1";', "", 2))])

# ---- §5 構文 ------------------------------------------------------------------
src = ('// §5 構文: CDL にない TypeScript の構文は E1005（DECISIONS C1）。\n"use cdl 1";\n'
       'const { a } = o;\nconst b = x == y;\nconst c = x ** 2;\nconst e = typeof x;\nconst f = x!;\n'
       'const g = function () { return 1; };\nconst h = f(...xs);\nconst i = { m() { return 1; } };\n'
       'type T = A & B;\ninterface I extends J { a: number }\nexport { a };\nimport X from "./m.cdl";\nx;\n')
write("5-unsupported-syntax", src, diags=[("E1005", "const { a } = o;"), ("E1005", "x == y"), ("E1005", "x ** 2"),
                                           ("E1005", "typeof x"), ("E1005", "x!"),
                                           ("E1005", "function () { return 1; }"), ("E1005", "...xs"),
                                           ("E1005", "m() { return 1; }"), ("E1005", "A & B"),
                                           ("E1005", "interface I extends J { a: number }"), ("E1005", "export { a };"),
                                           ("E1005", ("import X", "X")), ("E1005", ("\nx;", "x;"))])

src = ('// §5 文: `if` には `else` が要る（DECISIONS A4）。\n"use cdl 1";\n'
       'function f(x: boolean): number {\n  if (x) { return 1; }\n}\n')
write("5-if-requires-else", src, diags=[("E1010", "if (x) { return 1; }")])

src = ('// §5 ブロック: 最後の文のあとに文は置けない。\n"use cdl 1";\n'
       'function f(): number { return 1; const a = 2; }\n')
write("5-block-shape", src, diags=[("E1005", "const a = 2;")])

src = program("§5 JSX: テキスト子は E1009（DECISIONS A1）。", "    <Keybind keys=\"a\" action=\"b\" />hello\n")
write("5-jsx-text-child", src, diags=[("E1009", "hello\n  ")])

src = program("§5 JSX: 小文字で始まるタグは E1011（DECISIONS C5）。", "    <div />\n")
write("5-lowercase-tag", src, diags=[("E1011", "div")])

# ---- §7 入れ子の深さ -----------------------------------------------------------------
prefix = '// §7 リソース上限: 構文の入れ子の深さ 64 を超えると E1008（DECISIONS C10）。\n"use cdl 1";\nconst a = '
src = prefix + "(" * 200 + "1" + ")" * 200 + ";\n"
# `=` が 1、`(` が 1 つずつ。64 個めの `(` で 65 になる。
write("7-nesting-too-deep", src, diags=[("E1008", ("@", len(prefix.encode()) + 63, 1))])

# ---- §6・付録A の例 --------------------------------------------------------------
SPEC6 = '''// §6 ドキュメントモデル: 仕様の例。
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
'''
write("6-example", SPEC6, value=root(
    el("Appearance", {"gap": {"length": 16}, "radius": {"length": 8}, "focusColor": {"color": [137, 180, 250, 255]}}),
    el("Animation", {"duration": {"duration": 120}, "enabled": True}),
    kb("mod+Return", "spawn terminal"), kb("mod+q", "close")))

SPEC14 = '''// 付録A: コンポーネントと undefined を使う設定例。
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
'''
write("14-appendix-a", SPEC14, value=root(
    el("Appearance", {"gap": {"length": 16}, "radius": {"length": 8}, "focusColor": {"color": [137, 180, 250, 255]}}),
    el("Animation", {"duration": {"duration": 120}}),
    kb("mod+Return", "spawn terminal"), kb("mod+q", "close")))
print("ok")

# ---- 追加: 正常系（構文の全体を通す） -------------------------------------------
CF = '''// §5 文と式: function・ブロックの const・if / else if / else・switch・アロー関数・条件演算子。
"use cdl 1";
import { Contrast, Keybind } from "host:contrast";

function pick(n: number): string {
  const big = n > 10;
  if (big) {
    return "big";
  } else if (n === 0) {
    return "zero";
  } else {
    return "small";
  }
}

const label = (s: "a" | "b"): string => {
  switch (s) {
    case "a":
      return "A";
    default:
      return "other";
  }
};

export default (
  <Contrast>
    <Keybind keys={pick(20)} action={label("a")} />
    <Keybind keys={pick(0)} action={label("b")} />
    <Keybind keys={pick(3)} action={true ? "t" : "f"} />
  </Contrast>
);
'''
write("5-control-flow", CF, value=root(kb("big", "A"), kb("zero", "other"), kb("small", "t")))

FR = '''// §5 JSX: フラグメント・属性のスプレッド・配列のスプレッド・配列の子の平坦化・undefined の子。
"use cdl 1";
import { Contrast, Keybind } from "host:contrast";

const common = { action: "close" };
const more = [<Keybind keys="c" {...common} />];

export default (
  <Contrast>
    <>
      <Keybind keys="a" {...common} />
      <>{[<Keybind keys="b" action="open" />]}</>
    </>
    {[...more, undefined]}
    {false ? <Keybind keys="z" action="z" /> : undefined}
  </Contrast>
);
'''
write("5-jsx-fragments-and-spread", FR, value=root(kb("a", "close"), kb("b", "open"), kb("c", "close")))

TY = '''// §4・§5 型の宣言: type・interface・ユニオン・リテラル型・省略可能なプロパティ・satisfies・配列のメソッド。
"use cdl 1";
import { Contrast, Keybind } from "host:contrast";

type Mode = "spawn" | "close";
interface Bind { keys: string; action: Mode; note?: string }

const binds: Bind[] = [
  { keys: "a", action: "spawn" },
  { keys: "b", action: "close", note: undefined },
];
const fallback = { keys: "none", action: "close" } satisfies Bind;
const first = binds[0] ?? fallback;

export default (
  <Contrast>
    <Keybind keys={first.keys} action={first.action} />
    {binds.filter((b) => b.action === "close").map((b) => <Keybind keys={b.keys} action={`${b.action}!`} />)}
  </Contrast>
);
'''
write("5-types-and-satisfies", TY, value=root(kb("a", "spawn"), kb("b", "close!")))

# ---- 追加: 禁止構文ごとの異常系（1 ファイル 1 構文） --------------------------------
def forbidden(name, code, comment, body, spec):
    src = f'// {comment}\n"use cdl 1";\n{body}'
    write(name, src, diags=[(code, spec)])

forbidden("3-forbidden-let", "E1006", "§3 予約語: `let` は E1006（help: const）。", "let a = 1;\n", "let a = 1;")
forbidden("3-forbidden-var", "E1006", "§3 予約語: `var` は E1006（help: const）。", "var a = 1;\n", "var a = 1;")
forbidden("3-forbidden-class", "E1006", "§3 予約語: `class` は E1006。", "class C {}\n", "class C {}")
forbidden("3-forbidden-new", "E1006", "§3 予約語: `new` は E1006。", "const a = new Date();\n", "new Date()")
forbidden("3-forbidden-this", "E1006", "§3 予約語: `this` は E1006。", "const a = this;\n", ("= this", "this"))
forbidden("3-forbidden-null", "E1006", "§3 予約語: `null` は E1006（help: undefined）。", "const a = null;\n", ("= null", "null"))
forbidden("3-forbidden-for", "E1006", "§3 予約語: `for` は E1006（help: map など）。", "for (const x of xs) {}\n",
          "for (const x of xs) {}")
forbidden("3-forbidden-while", "E1006", "§3 予約語: `while` は E1006。", "while (true) {}\n", "while (true) {}")
forbidden("3-forbidden-try", "E1006", "§3 予約語: `try` は E1006。", "try {} catch {}\n", "try {} catch {}")
forbidden("3-forbidden-async", "E1006", "§3 予約語: `async` は E1006。", "const f = async (): number => 1;\n",
          "async (): number => 1")
forbidden("3-forbidden-enum", "E1006", "§3 予約語: `enum` は E1006（help: リテラルのユニオン型）。", "enum E { A }\n",
          "enum E { A }")
forbidden("3-forbidden-as", "E1006", "§3 予約語: 型アサーション `as` は E1006（help: satisfies）。",
          "const a = x as number;\n", "x as number")
forbidden("3-forbidden-hooks", "E1007", "§3 React: `use` で始まる名前（Hooks）は E1007。",
          "const s = useState(0);\n", "useState")
forbidden("3-forbidden-event-handler", "E1007", "§3 React: イベントハンドラ属性は E1007。",
          'import { Contrast, Keybind } from "host:contrast";\nexport default (<Contrast><Keybind keys="a" action="b" onClick={f} /></Contrast>);\n',
          "onClick")
forbidden("3-forbidden-ref", "E1007", "§3 React: `ref` 属性は E1007。",
          'import { Contrast, Keybind } from "host:contrast";\nexport default (<Contrast><Keybind keys="a" action="b" ref={r} /></Contrast>);\n',
          ("ref={", "ref"))
forbidden("3-forbidden-destructuring", "E1005", "§5 書けない構文: 分割代入は E1005（help: 1 つずつ取り出す）。",
          "const { a } = o;\n", "const { a } = o;")
forbidden("3-forbidden-rest-param", "E1005", "§5 書けない構文: 残余引数は E1005。",
          "function f(...xs: number[]): number { return 1; }\n", "...xs: number[]")
forbidden("3-forbidden-and-render", "E3006", "§5 JSX: `c && <A />` は書けない。`&&` のオペランドは boolean だけ（E3006。DECISIONS A2・S2）。",
          'import { Contrast, Keybind } from "host:contrast";\nconst on = true;\n'
          'export default (<Contrast>{on && <Keybind keys="a" action="b" />}</Contrast>);\n',
          'on && <Keybind keys="a" action="b" />')
print("added")

# ==== 意味解析（第 4・6・7 章） ===================================================
HOST_ALL = 'import { Contrast, Appearance, Animation, Keybind } from "host:contrast";\n'
ROOT = "export default (<Contrast />);\n"

def sema_err(name, comment, body, diags, root=True):
    src = f'// {comment}\n"use cdl 1";\n{HOST_ALL}{body}' + (ROOT if root else "")
    write(name, src, diags=diags)

def sema_ok(name, comment, before, children, value):
    src = f'// {comment}\n"use cdl 1";\n{HOST_ALL}{before}\nexport default (\n  <Contrast>\n{children}  </Contrast>\n);\n'
    write(name, src, value=value)

def app(gap, radius, color=None):
    props = {"gap": {"length": gap}, "radius": {"length": radius}}
    if color is not None:
        props["focusColor"] = {"color": color}
    return el("Appearance", props)

# ---- 正常系 ----
sema_ok("4-narrowing", "§4 undefined の絞り込み: !== undefined・=== undefined・?? で絞り込んでから使う。",
        'const maybe: string | undefined = undefined;\nconst k = maybe !== undefined ? maybe : "none";\n'
        'function label(x: string | undefined): string {\n  if (x === undefined) {\n    return "u";\n  } else {\n    return x;\n  }\n}\n',
        '    <Keybind keys={k} action={label("a")} />\n    <Keybind keys={maybe ?? "d"} action={label(undefined)} />\n',
        root(kb("none", "a"), kb("d", "u")))
sema_ok("4-units", "§4 単位つきの値: Length・Duration の演算と比較（DECISIONS B11）、color() の書式（B12）。",
        "",
        '    <Appearance gap={px(4) * 2 + px(1)} radius={px(10) / 2 - px(1)} focusColor={color("#fff")} />\n'
        '    <Animation duration={sec(1) + ms(250) * 2} enabled={ms(10) < sec(1)} />\n',
        root(app(9, 4, [255, 255, 255, 255]), el("Animation", {"duration": {"duration": 1500}, "enabled": True})))
sema_ok("4-switch-exhaustive", "§5 switch: リテラルのユニオンと boolean を網羅すれば default は要らない。",
        'type Mode = "a" | "b";\nfunction name(m: Mode): string {\n  switch (m) {\n    case "a":\n      return "A";\n    case "b":\n      return "B";\n  }\n}\n'
        'const flag = (b: boolean): string => {\n  switch (b) {\n    case true:\n      return "on";\n    case false:\n      return "off";\n  }\n};\n',
        '    <Keybind keys={name("b")} action={flag(true)} />\n',
        root(kb("B", "on")))
sema_ok("4-records", "§4 Record: 注釈つきのオブジェクトリテラルで作り、r[k] で引く（型は T | undefined。DECISIONS B16）。",
        'const names: Record<string, string> = { a: "x", b: "y" };\n',
        '    <Keybind keys={names["a"] ?? "-"} action={names["z"] ?? "-"} />\n',
        root(kb("x", "-")))
sema_ok("4-spread-later-wins", "§5 スプレッド: 重なったキーはあとに書いたものが勝つ（DECISIONS S1）。",
        'const base = { keys: "a", action: "base" };\nconst o = { ...base, action: "over" };\n',
        '    <Keybind {...o} />\n    <Keybind {...base} action="attr" />\n',
        root(kb("a", "over"), kb("a", "attr")))
sema_ok("7-dag-calls", "§7 停止性: 参照グラフが DAG なら、宣言の順序は問わない（DECISIONS C7）。",
        'const top = twice("x");\nfunction twice(s: string): string {\n  return `${once(s)}${once(s)}`;\n}\nfunction once(s: string): string {\n  return s;\n}\n',
        '    <Keybind keys={top} action={once("y")} />\n',
        root(kb("xx", "y")))

# ---- 型検査の誤り（E3） ----
sema_err("4-type-mismatch", "§4 型検査: 注釈・戻り値・引数の型が違えば E3001。",
         'const a: number = "x";\nconst b: string = px(1);\nfunction f(n: number): string {\n  return n;\n}\nconst c = f("1");\n',
         [("E3001", ("number = ", '"x"')), ("E3001", "px(1)"), ("E3001", ("return n", "n")), ("E3001", ('f("1")', '"1"'))])
sema_err("4-excess-property", "§4 閉じた型: 余分なキーは E3002、足りないキーは E3003。",
         "interface P { a: number }\nconst p: P = { a: 1, b: 2 };\nconst q: P = {};\n",
         [("E3002", ("b: 2", "b")), ("E3003", ("q: P = {}", "{}"))])
sema_err("4-undefined-must-be-narrowed", "§4 undefined を含む型は、絞り込むまで使えない（E3004）。",
         "interface W { name: string }\nconst w: W | undefined = undefined;\nconst n = w.name;\n"
         "function g(x: number | undefined): number {\n  return x + 1;\n}\n",
         [("E3004", ("w.name", "w")), ("E3004", ("return x", "x"))])
sema_err("4-switch-not-exhaustive", "§5 switch: リテラルのユニオンを網羅しないか、それ以外の型で default がなければ E3005。",
         'type Mode = "a" | "b" | "c";\nfunction f(m: Mode): number {\n  switch (m) {\n    case "a":\n      return 1;\n    case "b":\n      return 2;\n  }\n}\n'
         'function g(s: string): number {\n  switch (s) {\n    case "a":\n      return 1;\n  }\n}\n',
         [("E3005", ("switch (m)", "m")), ("E3005", ("switch (s)", "s"))])
sema_err("4-unit-operators", "§4 単位つきの値: 許されない組み合わせの演算は E3006（DECISIONS B11・B13）。",
         'const a = px(1) + 1;\nconst b = px(1) * px(2);\nconst c = ms(1) + px(1);\nconst d = color("#fff") + color("#000");\nconst e = "a" + "b";\n',
         [("E3006", "px(1) + 1"), ("E3006", "px(1) * px(2)"), ("E3006", "ms(1) + px(1)"),
          ("E3006", 'color("#fff") + color("#000")'), ("E3006", '"a" + "b"')])
sema_err("4-conditions-are-boolean", "§5 条件: if と ?: の条件は boolean（E3001）、! && || のオペランドも boolean（E3006。DECISIONS S2）。",
         'const s = "x";\nconst a = s ? 1 : 2;\nfunction f(x: number | undefined): number {\n  if (x) {\n    return 1;\n  } else {\n    return 0;\n  }\n}\nconst b = !s;\nconst c = s && true;\n',
         [("E3001", ("a = s", "s")), ("E3001", ("if (x)", "x")), ("E3006", "!s"), ("E3006", "s && true")])
sema_err("4-annotations-required", "§4 注釈: 宣言の引数と戻り値、空の配列には型注釈が要る（E3007。DECISIONS A3・B4）。",
         "function f(x): number {\n  return 1;\n}\nfunction g(x: number) {\n  return x;\n}\nconst h = (y: number) => y;\nconst e = [];\n",
         [("E3007", ("f(x)", "x")), ("E3007", ("function g", "g")), ("E3007", ("const h", "h")), ("E3007", "[]")])
sema_err("4-union-forms", "§4 ユニオン: リテラルどうし・T | undefined・Element のユニオンだけ（E3009。DECISIONS B2）。",
         "type T = string | number;\ntype U = { a: number } | { b: number };\n",
         [("E3009", "string | number"), ("E3009", "{ a: number } | { b: number }")])
sema_err("4-component-called-as-function", "§5 コンポーネントは JSX からだけ使う（E3008。DECISIONS C5）。",
         'function Bind(props: { keys: string }): Element<Keybind> {\n  return <Keybind keys={props.keys} action="x" />;\n}\nconst e = Bind({ keys: "a" });\n',
         [("E3008", 'Bind({ keys: "a" })')])

# ---- スキーマ適合（E4）とユーザーコンポーネント ----
def doc_err(name, comment, before, children, diags):
    src = f'// {comment}\n"use cdl 1";\n{HOST_ALL}{before}\nexport default (\n  <Contrast>\n{children}  </Contrast>\n);\n'
    write(name, src, diags=diags)

doc_err("6-unknown-host-prop", "§6 props: 組み込みコンポーネントの未知の props は E4003（DECISIONS A5）。", "",
        '    <Keybind keys="a" action="b" extra="c" />\n', [("E4003", "extra")])
doc_err("6-missing-host-prop", "§6 props: 組み込みコンポーネントの必須の props がなければ E4002。", "",
        '    <Keybind keys="a" />\n', [("E4002", ('<Keybind keys="a" />', "Keybind"))])
doc_err("6-host-prop-type", "§6 props: 組み込みコンポーネントの props の型が違えば E4004。", "",
        '    <Appearance gap={1} radius={px(1)} />\n    <Keybind keys={1} action="b" />\n',
        [("E4004", ("gap={1}", "1")), ("E4004", ("keys={1}", "1"))])
doc_err("6-child-type", "§6 子: ホストが許さない子は E4005。", "",
        '    <Contrast />\n    <Keybind keys="a" action="b">\n      <Animation duration={ms(1)} />\n    </Keybind>\n',
        [("E4005", "<Contrast />"), ("E4005", "<Animation duration={ms(1)} />")])
doc_err("6-unique-child", "§6 子: ホストが一意と宣言した子の重複は E4006（後勝ちにしない。DECISIONS S3）。", "",
        '    <Appearance gap={px(1)} radius={px(1)} />\n    <Appearance gap={px(2)} radius={px(2)} />\n',
        [("E4006", "<Appearance gap={px(2)} radius={px(2)} />")])
src = '// §6 ルート: ルート要素はホストが指定したコンポーネント（E4001）。\n"use cdl 1";\n' + HOST_ALL + 'export default (<Keybind keys="a" action="b" />);\n'
write("6-root-element", src, diags=[("E4001", '<Keybind keys="a" action="b" />')])
src = '// §6 ルート: エントリファイルには export default が要る（E4001）。\n"use cdl 1";\n' + HOST_ALL + "const a = 1;\n"
write("6-root-missing", src, diags=[("E4001", ("@", 0, 0))])
doc_err("6-user-component-props", "§6 ユーザーのコンポーネント: props の余分は E3002、欠落は E3003、型違いは E3001（DECISIONS A5）。",
        'function Bind(props: { keys: string }): Element<Keybind> {\n  return <Keybind keys={props.keys} action="x" />;\n}\n',
        '    <Bind keys="a" extra="b" />\n    <Bind />\n    <Bind keys={1} />\n',
        [("E3002", "extra"), ("E3003", ("<Bind />", "Bind")), ("E3001", ("keys={1}", "1"))])

# ---- 名前解決（E2） ----
sema_err("7-recursion", "§7 停止性: 再帰は E2005。",
         "function f(n: number): number {\n  return f(n);\n}\n", [("E2005", ("function f", "f"))])
sema_err("7-mutual-recursion", "§7 停止性: 相互再帰は E2005（最初に宣言した関数で報告する）。",
         "function g(n: number): number {\n  return h(n);\n}\nfunction h(n: number): number {\n  return g(n);\n}\n",
         [("E2005", ("function g", "g"))])
sema_err("7-component-self-child", "§7 停止性: コンポーネントが自分自身を子に置くことは E2005。",
         "function Loop(props: {}): Element<Keybind> {\n  return <Loop />;\n}\n", [("E2005", ("function Loop", "Loop"))])
sema_err("7-const-cycle", "§7 const の依存の循環は E2006。",
         "const a = b;\nconst b = a;\nconst c = k();\nfunction k(): number {\n  return c;\n}\n",
         [("E2006", ("const a", "a")), ("E2006", ("const c", "c"))])
sema_err("4-type-cycle", "§4 再帰型は E2007（DECISIONS B8）。",
         "type A = B[];\ntype B = A | undefined;\ninterface N { next: N | undefined }\n",
         [("E2007", ("type A", "A")), ("E2007", ("interface N", "N"))])
sema_err("5-undefined-name", "§5 名前解決: 未定義の名前は E2002。",
         "const a = missing;\nconst e = <Nope />;\ntype T = Unknown;\n",
         [("E2002", "missing"), ("E2002", "Nope"), ("E2002", "Unknown")])
sema_err("5-duplicate-names", "§5 名前解決: 同じ名前の宣言の重複は E2003。",
         "const a = 1;\nconst a = 2;\n", [("E2003", ("const a = 2", "a"))])
sema_err("5-shadowing", "§5 名前解決: 名前の隠蔽は E2004（DECISIONS C6）。",
         "const x = 1;\nfunction f(x: number): number {\n  return x;\n}\nconst g = [1].map((px) => px);\n",
         [("E2004", ("f(x", "x")), ("E2004", ("((px", "px"))])
src = ('// §9 import: ホストのモジュールにない名前は E2008、解決できないモジュールは E2001。\n"use cdl 1";\n' + HOST_ALL +
       'import { Nope } from "host:contrast";\nimport { A } from "host:other";\n' + ROOT)
write("9-import-errors", src, diags=[("E2008", "Nope"), ("E2001", '"host:other"')])
print("sema added")

# ==== 評価（第 7・8 章） ==========================================================
def eval_err(name, comment, body, code, spec):
    src = f'// {comment}\n"use cdl 1";\n{HOST_ALL}{body}' + ROOT
    write(name, src, diags=[(code, spec)])

eval_err("7-divide-by-zero", "§7 決定性: 0 除算は E5007。", "const a = 1 / 0;\n", "E5007", "1 / 0")
eval_err("7-remainder-by-zero", "§7 決定性: 0 での剰余は E5007。", "const a = 5 % 0;\n", "E5007", "5 % 0")
eval_err("7-divide-by-zero-in-function", "§7 決定性: 関数の中の 0 除算も、その演算の位置で E5007（DECISIONS D8）。",
         "function f(n: number): number {\n  return 10 / n;\n}\nconst c = f(0);\n", "E5007", "10 / n")
eval_err("7-non-finite", "§7 決定性: ±Infinity を生む演算は E5008。", "const a = 1e308 * 10;\n", "E5008", "1e308 * 10")
eval_err("7-length-zero-divide", "§7 決定性: Length の 0 除算も E5007。", "const a = px(1) / 0;\n", "E5007", "px(1) / 0")
eval_err("7-duration-not-integral", "§4 Duration: 整数ミリ秒にならない値は E5009（DECISIONS B11）。",
         "const a = ms(1.5);\n", "E5009", "ms(1.5)")
eval_err("7-duration-division", "§4 Duration: 割り算の結果が整数ミリ秒にならなければ E5009。",
         "const a = ms(100) / 3;\n", "E5009", "ms(100) / 3")
eval_err("7-color-format", "§4 Color: 書式が違えば E5010（DECISIONS B12）。", 'const a = color("blue");\n', "E5010",
         'color("blue")')

steps = ("const a = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];\n"
         + "".join(f"const a{i} = {'a' if i == 1 else f'a{i-1}'}.concat({'a' if i == 1 else f'a{i-1}'});\n" for i in range(1, 9))
         + "const s = a8.map((x) => a8.map((y) => x + y));\n")
eval_err("7-step-limit", "§7 リソース上限: 評価ステップ 1,000,000 を超えると E5002（位置は宣言。DECISIONS D8）。",
         steps, "E5002", ("const s", "s"))
coll = ("const a = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];\n"
        + "".join(f"const a{i} = {'a' if i == 1 else f'a{i-1}'}.concat({'a' if i == 1 else f'a{i-1}'});\n" for i in range(1, 14)))
eval_err("7-collection-limit", "§7 リソース上限: 配列の要素数 65,536 を超えると E5005。", coll, "E5005", ("const a13", "a13"))
strs = 'const s0 = "0123456789abcdef";\n' + "".join(f"const s{i} = `${{s{i-1}}}${{s{i-1}}}`;\n" for i in range(1, 14))
eval_err("7-string-limit", "§7 リソース上限: 文字列長 64 KiB を超えると E5004。", strs, "E5004", ("const s13", "s13"))
fns = "".join(f"function f{i}(n: number): number {{\n  return f{i+1}(n);\n}}\n" for i in range(1, 66)) + \
      "function f66(n: number): number {\n  return n;\n}\nconst d = f1(1);\n"
eval_err("7-call-depth", "§7 リソース上限: 呼び出しの深さ 64 を超えると E5003。", fns, "E5003", ("const d", "d"))

src = ('// §6 一意な子: map などが作る子の重複は、評価後に E4006（DECISIONS E2）。\n"use cdl 1";\n' + HOST_ALL +
       'export default (\n  <Contrast>\n    {[1, 2].map((n) => <Appearance gap={px(n)} radius={px(n)} />)}\n  </Contrast>\n);\n')
write("6-unique-child-after-eval", src, diags=[("E4006", "<Appearance gap={px(n)} radius={px(n)} />")])

sema_ok("7-evaluation-order", "§7 評価: const は依存の順に評価し、使われない const も評価する（DECISIONS C7）。",
        'const unused = ms(5);\nconst b = a * 2;\nconst a = 21;\nconst label = `${b}`;\n',
        '    <Keybind keys={label} action={`${-0} ${0.1 + 0.2} ${1e21} ${1.5e-7} ${-1 / 3}`} />\n',
        root(kb("42", "0 0.30000000000000004 1e+21 1.5e-7 -0.3333333333333333")))
sema_ok("7-builtins", "§5 配列の組み込みメソッドと添字（DECISIONS B15・B17）。",
        'const xs = [3, 1, 2];\nconst total = xs.reduce((acc, x) => acc + x, 0);\nconst big = xs.filter((x) => x > 1);\n'
        'const more = xs.concat([4]);\n',
        '    <Keybind keys={`${total} ${big.length} ${more.length} ${xs.includes(2) ? "yes" : "no"}`} action={`${xs[1] ?? -1} ${xs[9] ?? -1} ${xs[0.5] ?? -1}`} />\n',
        root(kb("6 2 4 yes", "1 -1 -1")))

# ---- §8 ポリシー -----------------------------------------------------------------
def write_calls(name, calls):
    with open(os.path.join(OUT, rel_of(name) + ".calls.json"), "w", encoding="utf-8") as f:
        json.dump(calls, f, ensure_ascii=False, indent=2)
        f.write("\n")

POLICY = '''// §8 ポリシー: 仕様の例。フックはホストから引数の値を受けて呼ばれる。
"use cdl 1";
import { Contrast } from "host:contrast";
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

export default (<Contrast />);
'''
write("8-policy-example", POLICY, value=root())
write_calls("8-policy-example", [
    {"hook": "windowOpen", "arg": {"appId": "firefox", "workspace": 1}, "result": {"workspace": 2, "floating": False}},
    {"hook": "windowOpen", "arg": {"appId": "pavucontrol", "workspace": 3}, "result": {"workspace": 3, "floating": True}},
    {"hook": "windowOpen", "arg": {"appId": "kitty", "workspace": 5}, "result": {"workspace": 5, "floating": False}},
    {"hook": "windowOpen", "arg": {"appId": "x"}, "error": "E4007"},
    {"hook": "windowOpen", "arg": {"appId": "x", "workspace": 1, "extra": 1}, "error": "E4007"},
])
HEAVY = '''// §8 ポリシー: フックの呼び出しごとに、専用のステップ上限（既定 10,000）を適用する。
"use cdl 1";
import { Contrast } from "host:contrast";
import type { Policy, WindowInfo, Placement } from "host:contrast";

const xs = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
const ys = xs.concat(xs).concat(xs).concat(xs);

export const policy = {
  windowOpen: (w: WindowInfo): Placement => {
    const n = ys.reduce((acc, x) => acc + ys.reduce((a, y) => a + y, 0), 0);
    return { workspace: w.workspace + n, floating: false };
  },
} satisfies Policy;

export default (<Contrast />);
'''
write("8-policy-step-limit", HEAVY, value=root())
write_calls("8-policy-step-limit", [
    {"hook": "windowOpen", "arg": {"appId": "a", "workspace": 1}, "error": "E5002"},
])
print("eval added")

# ==== モジュール（第 9 章） ==========================================================
def write_dir(name, files, value=None, diags=None):
    d = os.path.join(OUT, rel_of(name))
    os.makedirs(d, exist_ok=True)
    for rel, src in files.items():
        p = os.path.join(d, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write(src)
    if diags is not None:
        items = []
        for code, rel, spec in diags:
            start, end = find_span(files[rel], spec)
            items.append({"code": code, "severity": "error", "span": {"file": rel, "start": start, "end": end}})
        expected = {"diagnostics": items}
    else:
        expected = {"value": value}
    with open(os.path.join(d, "expected.json"), "w", encoding="utf-8") as f:
        json.dump(expected, f, ensure_ascii=False, indent=2)
        f.write("\n")

write_dir("9-imports", {
    "main.cdl": '''// §9 モジュール: 相対パスの import で、ほかのファイルの const・関数・型・コンポーネントを取り込む。
"use cdl 1";
import { Contrast, Appearance } from "host:contrast";
import { BASE, gap } from "./theme.cdl";
import { Bind } from "./parts/bind.cdl";
import type { Mode } from "./theme.cdl";

const mode: Mode = "dark";

export default (
  <Contrast>
    <Appearance gap={gap(2)} radius={BASE} />
    <Bind keys="q" action={mode} />
  </Contrast>
);
''',
    "theme.cdl": '''"use cdl 1";
export const BASE = px(8);
export function gap(n: number): Length {
  return BASE * n;
}
export type Mode = "dark" | "light";
''',
    "parts/bind.cdl": '''"use cdl 1";
import { Keybind } from "host:contrast";
import { BASE } from "../theme.cdl";

export function Bind(props: { keys: string; action: string }): Element<Keybind> {
  return <Keybind keys={`mod+${props.keys}`} action={props.action} />;
}
''',
}, value=root(app(16, 8), kb("mod+q", "dark")))

write_dir("9-import-cycle", {
    "main.cdl": '// §9 モジュール: import の循環は、まとめて 1 つの E2010（DECISIONS M2）。\n"use cdl 1";\nimport { Contrast } from "host:contrast";\nimport { a } from "./a.cdl";\nexport default (<Contrast />);\n',
    "a.cdl": '"use cdl 1";\nimport { b } from "./b.cdl";\nexport const a = 1;\n',
    "b.cdl": '"use cdl 1";\nimport { a } from "./a.cdl";\nexport const b = 2;\n',
}, diags=[("E2010", "a.cdl", '"./b.cdl"')])

write_dir("9-import-not-exported", {
    "main.cdl": '// §9 モジュール: export していない名前は import できない（E2008）。\n"use cdl 1";\nimport { Contrast } from "host:contrast";\nimport { hidden } from "./lib.cdl";\nexport default (<Contrast />);\n',
    "lib.cdl": '"use cdl 1";\nconst hidden = 1;\nexport const shown = hidden;\n',
}, diags=[("E2008", "main.cdl", ("{ hidden }", "hidden"))])

write_dir("9-library-rules", {
    "main.cdl": '// §6・§9 ライブラリのファイルは export default と export const policy を持てない（E2011。DECISIONS E6）。\n"use cdl 1";\nimport { Contrast } from "host:contrast";\nimport { x } from "./lib.cdl";\nexport default (<Contrast />);\n',
    "lib.cdl": '"use cdl 1";\nimport { Contrast } from "host:contrast";\nexport const x = 1;\nexport const policy = {};\nexport default (<Contrast />);\n',
}, diags=[("E2011", "lib.cdl", ("const policy", "policy")), ("E2011", "lib.cdl", "export default (<Contrast />);")])

src = ('// §9 import: 指定子は ./ か ../ で始まり .cdl で終わる相対パスだけ（E2009。DECISIONS M1）。\n"use cdl 1";\n' + HOST_ALL +
       'import { a } from "/abs/a.cdl";\nimport { b } from "./b";\nimport { c } from "https://example.com/c.cdl";\nimport { d } from "lib.cdl";\n' + ROOT)
write("9-import-specifier", src, diags=[("E2009", '"/abs/a.cdl"'), ("E2009", '"./b"'), ("E2009", '"https://example.com/c.cdl"'), ("E2009", '"lib.cdl"')])
src = ('// §9 import: 解決できないファイルは E2001。\n"use cdl 1";\n' + HOST_ALL + 'import { a } from "./missing.cdl";\n' + ROOT)
write("9-import-missing-file", src, diags=[("E2001", '"./missing.cdl"')])
print("modules added")

# ==== ポリシーの規則（DECISIONS A6・E5） ===============================================
POLICY_HEAD = ('"use cdl 1";\nimport { Contrast } from "host:contrast";\n'
               'import type { Policy, WindowInfo, Placement } from "host:contrast";\n\n')
src = ('// §8 ポリシー: `export const policy` には `satisfies Policy` が要る（E4009。DECISIONS E5）。\n' + POLICY_HEAD +
       'export const policy = {\n  windowOpen: (w: WindowInfo): Placement => ({ workspace: w.workspace, floating: false }),\n};\n\n'
       'export default (<Contrast />);\n')
write("8-policy-requires-satisfies", src, diags=[("E4009", ("const policy = {", "policy"))])
src = ('// §8 ポリシー: ホストの Policy にないフックは E4010（DECISIONS A6）。\n' + POLICY_HEAD +
       'export const policy = {\n  windowClose: (w: WindowInfo): Placement => ({ workspace: w.workspace, floating: false }),\n} satisfies Policy;\n\n'
       'export default (<Contrast />);\n')
write("8-policy-unknown-hook", src, diags=[("E4010", "windowClose")])
print("policy rules added")
