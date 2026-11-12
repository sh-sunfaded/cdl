//! `parse` の結合テスト（SPEC 第 3・5 章）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use cdl_diag::Code;
use cdl_syntax::ast::*;
use cdl_syntax::{Limits, parse};

fn ok(src: &str) -> Module {
    match parse("main.cdl", src, &Limits::default()) {
        Ok(m) => m,
        Err(d) => panic!("{src}\n{d:#?}"),
    }
}

/// 診断の (コード, 開始, 終了)。
fn errors(src: &str) -> Vec<(Code, u32, u32)> {
    match parse("main.cdl", src, &Limits::default()) {
        Ok(_) => Vec::new(),
        Err(d) => d
            .iter()
            .map(|d| (d.code, d.span.start, d.span.end))
            .collect(),
    }
}

fn codes(src: &str) -> Vec<Code> {
    errors(src).into_iter().map(|(c, _, _)| c).collect()
}

/// edition 宣言を前に付ける。
fn file(body: &str) -> String {
    format!("\"use cdl 1\";\n{body}")
}

const SPEC_6: &str = r##""use cdl 1";
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
"##;

const SPEC_8: &str = r##""use cdl 1";
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
"##;

const SPEC_14A: &str = r##""use cdl 1";
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
"##;

#[test]
fn spec_examples_parse() {
    let m = ok(SPEC_6);
    assert_eq!(m.edition, 1);
    assert_eq!(m.items.len(), 4);
    let ItemKind::ExportDefault(root) = &m.items[3].kind else {
        panic!()
    };
    let ExprKind::Element(el) = &root.kind else {
        panic!()
    };
    assert_eq!(el.name.name, "Contrast");
    // 空白だけのテキスト子は無視する。
    assert_eq!(el.children.len(), 3);

    let m = ok(SPEC_8);
    let ItemKind::Const(policy) = &m.items[1].kind else {
        panic!()
    };
    assert!(m.items[1].exported);
    assert!(matches!(policy.init.kind, ExprKind::Satisfies { .. }));

    let m = ok(SPEC_14A);
    let ItemKind::Function(f) = &m.items[3].kind else {
        panic!()
    };
    assert_eq!(f.name.name, "Bind");
    assert!(matches!(*f.body.stmt, Stmt::Return(..)));
}

#[test]
fn values_are_taken_from_the_source() {
    let m = ok(&file(
        "const a = [0xff, 1_000, .5, -2, `x${1}\\`y`, 'it\\'s', \"\\u{1F600}\"];\n",
    ));
    let ItemKind::Const(c) = &m.items[0].kind else {
        panic!()
    };
    let ExprKind::Array(elems) = &c.init.kind else {
        panic!()
    };
    let kinds: Vec<&ExprKind> = elems
        .iter()
        .map(|e| match e {
            ArrayElem::Expr(e) => &e.kind,
            ArrayElem::Spread(..) => panic!(),
        })
        .collect();
    assert_eq!(kinds[0], &ExprKind::Number(255.0));
    assert_eq!(kinds[1], &ExprKind::Number(1000.0));
    assert_eq!(kinds[2], &ExprKind::Number(0.5));
    assert!(matches!(
        kinds[3],
        ExprKind::Unary {
            op: UnaryOp::Neg,
            ..
        }
    ));
    let ExprKind::Template { quasis, exprs } = kinds[4] else {
        panic!()
    };
    assert_eq!(quasis, &["x".to_owned(), "`y".to_owned()]);
    assert_eq!(exprs.len(), 1);
    assert_eq!(kinds[5], &ExprKind::String("it's".to_owned()));
    assert_eq!(kinds[6], &ExprKind::String("\u{1F600}".to_owned()));
}

#[test]
fn crlf_is_normalized_only_in_template_values() {
    let m = ok("\"use cdl 1\";\r\nconst t = `a\r\nb`;\r\n");
    let ItemKind::Const(c) = &m.items[0].kind else {
        panic!()
    };
    assert_eq!(
        c.init.kind,
        ExprKind::Template {
            quasis: vec!["a\nb".to_owned()],
            exprs: vec![]
        }
    );
    // span は元のソースのバイトオフセット。
    assert_eq!(c.init.span, Span { start: 24, end: 30 });
}

#[test]
fn jsx_attribute_strings_have_no_escapes_and_empty_containers_are_ignored() {
    let m = ok(&file("const e = <A k=\"a\\nb\">{/* c */}{ }</A>;\n"));
    let ItemKind::Const(c) = &m.items[0].kind else {
        panic!()
    };
    let ExprKind::Element(el) = &c.init.kind else {
        panic!()
    };
    let Attr::Named {
        value: Some(AttrValue::String(s)),
        ..
    } = &el.attrs[0]
    else {
        panic!()
    };
    assert_eq!(s.value, "a\\nb");
    assert!(el.children.is_empty());
}

#[test]
fn edition_declaration() {
    assert_eq!(codes("const a = 1;"), [Code::InvalidEdition]);
    assert_eq!(codes("\"use cdl  1\";"), [Code::InvalidEdition]);
    assert_eq!(codes("\"use cdl 01\";"), [Code::InvalidEdition]);
    assert_eq!(codes("\"use cdl 2\";"), [Code::UnsupportedEdition]);
    assert_eq!(
        codes("\"use cdl 99999999999\";"),
        [Code::UnsupportedEdition]
    );
    assert_eq!(
        codes("\"use cdl 1\";\n\"use strict\";"),
        [Code::InvalidEdition]
    );
    assert!(codes("'use cdl 1';").is_empty());
    assert_eq!(errors("\"use cdl 1\""), [(Code::MissingSemicolon, 11, 11)]);
}

#[test]
fn semicolons_are_required() {
    let src = file(
        "import { A } from \"x\"\nconst a = 1\ntype T = number\nexport const b = 2\nexport default (<A/>)\nfunction f(): number { return 1 }\n",
    );
    let got = codes(&src);
    assert_eq!(got, [Code::MissingSemicolon; 6], "{src}");
    // interface と function の後ろには要らない。
    assert!(
        codes(&file(
            "interface I { a: number }\nfunction f(): number { return 1; }\n"
        ))
        .is_empty()
    );
}

#[test]
fn reserved_words_are_reported() {
    let src = file(
        "let a = 1;\nvar b = 1;\nclass C {}\nenum E { A }\nnamespace N {}\nfor (;;) {}\nwhile (x) {}\ndo {} while (x);\nconst c = null;\nconst d = this;\nconst e = new X();\nconst f = x as number;\nconst as = 1;\nconst g = async () => 1;\nimport { h as i } from \"y\";\nconst type = 1;\nconst undefined = 1;\n",
    );
    let got = codes(&src);
    assert_eq!(got.len(), 17, "{got:?}");
    assert!(got.iter().all(|c| *c == Code::ReservedWord), "{got:?}");
}

#[test]
fn reserved_statements_inside_functions() {
    let src = file(
        "function f(): number { try { return 1; } catch { return 2; } }\nfunction g(): number { throw 1; }\nfunction h(): number { switch (x) { case 1: break; default: return 1; } }\n",
    );
    assert_eq!(codes(&src), [Code::ReservedWord; 3]);
}

#[test]
fn unsupported_syntax_is_reported() {
    let cases = [
        "const { a } = b;",
        "const [a] = b;",
        "const a = 1, b = 2;",
        "const a = x == y;",
        "const a = x ** 2;",
        "const a = x | y;",
        "const a = (x, y);",
        "const a = typeof x;",
        "const a = +x;",
        "const a = x!;",
        "const a = x?.[0];",
        "const a = f?.();",
        "const a = f(...xs);",
        "const a = [1, , 2];",
        "const a = { [k]: 1 };",
        "const a = { m() { return 1; } };",
        "const a = { get g() { return 1; } };",
        "const a = function () { return 1; };",
        "const a = f`x`;",
        "const a = <A>{...xs}</A>;",
        "const a = <A b=<B /> />;",
        "const a = <A.B />;",
        "const a = <a:b />;",
        "const a = <A x:y=\"1\" />;",
        "const a = (x = 1) => x;",
        "const a = (x?: number) => x;",
        "const a = (...xs: number[]) => xs;",
        "const a = <T,>(x: T) => x;",
        "function f<T>(x: T): T { return x; }",
        "function* g(): number { return 1; }",
        "type T<U> = U;",
        "type T = A & B;",
        "type T = keyof A;",
        "type T = any;",
        "type T = unknown;",
        "type T = [a?: number];",
        "interface I extends J { a: number }",
        "interface I { m(): void }",
        "interface I { readonly a: number }",
        "interface I { a }",
        "export { a };",
        "export * from \"x\";",
        "export default class {}",
        "import X from \"x\";",
        "import * as X from \"x\";",
        "import \"x\";",
        "import { type X } from \"x\";",
        "x;",
        ";",
        "function f(): number { const a = 1; }",
        "function f(): number { return 1; const a = 2; }",
        "function f(): number { { return 1; } }",
        "function f(): number { function g(): number { return 1; } return 1; }",
        "function f(): number { if (x) return 1; else return 2; }",
        "function f(): number { switch (x) { case 1: case 2: return 1; default: return 0; } }",
        "function f(): number { switch (x) { case 1: { return 1; } default: return 0; } }",
        "function f(): number { switch (x) { case y: return 1; default: return 0; } }",
        "function f(): number { switch (x) { default: return 0; case 1: return 1; } }",
        "function f(): number { return; }",
    ];
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|body| {
            let got = codes(&file(&format!("{body}\n")));
            let ok = !got.is_empty() && got.iter().all(|c| *c == Code::UnsupportedSyntax);
            (!ok).then(|| format!("{body}: {got:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn react_features_are_reported() {
    // `ref`・イベントハンドラ属性・Hooks は E1007。`key` は許可（DECISIONS R3）。
    let src = file(
        "const useThing = 1;\nconst a = useState(0);\nconst b = <A ref={r} key=\"k\" onClick={f} />;\n",
    );
    assert_eq!(codes(&src), [Code::ReactFeature; 4]);
    // `use` のあとが小文字なら Hooks の名前ではない。
    assert!(codes(&file("const user = 1;\nconst a = <A once={x} />;\n")).is_empty());
    assert!(
        codes(&file(
            "const a = <A key=\"k\" />;\nconst b = <A key={k} />;\n"
        ))
        .is_empty()
    );
}

#[test]
fn jsx_rules() {
    assert_eq!(
        codes(&file("const a = <div />;\n")),
        [Code::LowercaseJsxTag]
    );
    // テキスト子は許可（意味解析が、受け付けるコンポーネントかを検査する。DECISIONS R1）。
    assert_eq!(codes(&file("const a = <A>text</A>;\n")), []);
    assert_eq!(codes(&file("const a = <>{\"x\"}{y}</>;\n")), []);
}

/// 式 `const a = <A>…</A>;` の子を取り出す。
fn children_of(src: &str) -> Vec<Child> {
    let m = ok(&file(src));
    let ItemKind::Const(c) = &m.items[0].kind else {
        panic!()
    };
    let ExprKind::Element(el) = &c.init.kind else {
        panic!()
    };
    el.children.clone()
}

fn texts(children: &[Child]) -> Vec<String> {
    children
        .iter()
        .filter_map(|c| match c {
            Child::Text(t, _) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// テキスト子の空白の規則（DECISIONS R1）: 各行の前後の空白を除き、空行を捨て、1 つの空白でつなぐ。
#[test]
fn text_children_are_normalized() {
    let t = |src: &str| texts(&children_of(src));
    assert_eq!(t("const a = <A>hello</A>;\n"), ["hello"]);
    // JSX の標準: 単一行のテキストの前後の空白は残る（DECISIONS U5）。
    assert_eq!(t("const a = <A>  a   b  </A>;\n"), ["  a   b  "]);
    assert_eq!(t("const a = <A> hi </A>;\n"), [" hi "]);
    // 複数行: 最初の行の先頭と最後の行の末尾は残り、行の境目の空白は除かれる。
    assert_eq!(t("const a = <A>  x  \n  y  </A>;\n"), ["  x y  "]);
    assert_eq!(t("const a = <A>x \n\n \t y</A>;\n"), ["x y"]);
    assert_eq!(
        t("const a = <A>\n  one\n\n    two  \n  </A>;\n"),
        ["one two"]
    );
    // 空白だけのテキストは、常に捨てる（標準と違う点）。
    assert_eq!(t("const a = <A> </A>;\n"), Vec::<String>::new());
    assert_eq!(t("const a = <A><B /> <B /></A>;\n"), Vec::<String>::new());
    assert_eq!(
        t("const a = <A>\n  <B />\n  <B />\n</A>;\n"),
        Vec::<String>::new()
    );
    // 要素や `{…}` をはさむと、別々のテキストになる。
    assert_eq!(t("const a = <A>x<B />y{z}w</A>;\n"), ["x", "y", "w"]);
    // 記号・引用符・`\` はそのまま。
    assert_eq!(
        t("const a = <A>a, (b) 'c' \"d\" \\ e</A>;\n"),
        ["a, (b) 'c' \"d\" \\ e"]
    );
    // span は元のテキスト全体。
    let src = file("const a = <A>\n  hi\n</A>;\n");
    let Child::Text(_, span) = &children_of("const a = <A>\n  hi\n</A>;\n")[0] else {
        panic!()
    };
    let (start, end) = (
        usize::try_from(span.start).unwrap(),
        usize::try_from(span.end).unwrap(),
    );
    assert_eq!(&src[start..end], "\n  hi\n");
}

#[test]
fn text_children_reject_character_references() {
    let src = file("const a = <A>x &amp; y</A>;\n");
    let got = errors(&src);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, Code::JsxTextCharRef);
    let (start, end) = (
        usize::try_from(got[0].1).unwrap(),
        usize::try_from(got[0].2).unwrap(),
    );
    assert_eq!(&src[start..end], "&");
    assert!(helps(&src)[0].is_some());
    // 式の中の `&` は文字列なので問題ない。
    assert_eq!(codes(&file("const a = <A>{\"x & y\"}</A>;\n")), []);
}

#[test]
fn long_text_does_not_hit_the_nesting_limit() {
    // 句読点の多い文章でも、構文ネスト深さ（E1008）に当たらない。
    let words = "a, b; c. ".repeat(100);
    assert_eq!(codes(&file(&format!("const a = <A>{words}</A>;\n"))), []);
}

fn first_param(src: &str) -> Param {
    let m = ok(&file(src));
    let ItemKind::Function(f) = &m.items[0].kind else {
        panic!()
    };
    f.params[0].clone()
}

/// 引数の 1 階層のオブジェクトパターン（DECISIONS R4）。
#[test]
fn object_pattern_params() {
    let p = first_param("function f({ a, b = 1 }: P): number { return a; }\n");
    let ParamPattern::Object(props, _) = &p.pattern else {
        panic!()
    };
    let names: Vec<&str> = props.iter().map(|x| x.name.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(props[0].default.is_none());
    assert!(matches!(
        props[1].default.as_ref().map(|d| &d.kind),
        Some(ExprKind::Number(n)) if *n == 1.0
    ));
    assert!(p.ty.is_some());
    assert_eq!(
        p.bound_names()
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    // アロー関数の引数にも書ける。
    assert_eq!(codes(&file("const g = ({ a }: P): number => a;\n")), []);
    // 空のパターンも、構文としては書ける。
    assert_eq!(
        codes(&file("function h({}: P): number { return 1; }\n")),
        []
    );
}

#[test]
fn object_pattern_params_reject_other_shapes() {
    for body in [
        "function f({ a: x }: P): number { return x; }",
        "function f({ a: { b } }: P): number { return b; }",
        "function f({ ...r }: P): number { return 1; }",
        "function f({ [k]: x }: P): number { return x; }",
        "function f([a]: P): number { return a; }",
        "function f({ a } = o): number { return a; }",
        "function f({ a: x = 1 }: P): number { return x; }",
        "function f({ a: { b } = {} }: P): number { return 1; }",
        "function f(...xs: number[]): number { return 1; }",
        "const g = ({ a: x }: P): number => x;",
        "type F = ({ a }: P) => number;",
    ] {
        let got = codes(&file(&format!("{body}\n")));
        assert!(
            !got.is_empty() && got.iter().all(|c| *c == Code::UnsupportedSyntax),
            "{body}: {got:?}"
        );
    }
    // `const` への分割代入は、従来どおり書けない。
    assert_eq!(
        codes(&file("const { a } = o;\n")),
        [Code::UnsupportedSyntax]
    );
}

#[test]
fn if_requires_else() {
    let src = file("function f(): number { if (x) { return 1; } }\n");
    assert_eq!(codes(&src), [Code::MissingElse]);
    let src = file(
        "function f(): number { if (x) { return 1; } else if (y) { return 2; } else { return 3; } }\n",
    );
    assert!(codes(&src).is_empty());
}

#[test]
fn several_errors_in_one_file_are_all_reported_in_order() {
    let src = file("let a = 1;\nconst b = x == y;\nconst c = <div />;\n");
    assert_eq!(
        codes(&src),
        [
            Code::ReservedWord,
            Code::UnsupportedSyntax,
            Code::LowercaseJsxTag
        ]
    );
}

#[test]
fn lexical_errors_stop_before_parsing() {
    assert_eq!(
        codes(&file("const a = \"\\x41\";\nconst b = 0o7;\n")),
        [Code::InvalidEscape, Code::UnsupportedLiteral]
    );
    assert_eq!(codes(&file("const a = 1e400;\n")), [Code::NonFiniteNumber]);
    assert_eq!(
        codes(&file("const a = 0x1n;\n")),
        [Code::UnsupportedLiteral]
    );
}

#[test]
fn oxc_syntax_errors_are_reported() {
    assert_eq!(codes(&file("const a = ;\n")), [Code::SyntaxError]);
    assert_eq!(codes(&file("const a = <A></B>;\n")), [Code::SyntaxError]);
    // 空の属性値 `{}` は oxc が拒む。
    assert_eq!(codes(&file("const a = <A b={} />;\n")), [Code::SyntaxError]);
}

#[test]
fn source_size_limit() {
    let limits = Limits {
        max_source_bytes: 16,
        ..Limits::default()
    };
    let d = parse("main.cdl", &file("const a = 1;"), &limits).unwrap_err();
    assert_eq!(d[0].code, Code::SourceTooLarge);
}

/// 上限ちょうどの入れ子が、oxc と変換を通ってもスタックを使い切らない（テストのスレッドは 2 MiB）。
#[test]
fn nesting_at_the_limit_does_not_overflow_the_stack() {
    let n = 62;
    let inputs = [
        format!("const a = {}x{};", "(".repeat(n), ")".repeat(n)),
        format!("const a = {}x{};", "[".repeat(n), "]".repeat(n)),
        format!("const a = {}x;", "!".repeat(n)),
        format!("const a = {}x;", "-".repeat(n)),
        format!("const a = x{};", " + x".repeat(n / 2)),
        format!("const a = x{};", ".y".repeat(n)),
        format!("const a = f{};", "()".repeat(n / 2)),
        format!("const a = {}0;", "c ? 1 : ".repeat(n / 2)),
        format!("const a = {}x{};", "`${".repeat(n), "}`".repeat(n)),
        format!("const a = {}{};", "<A>".repeat(n), "</A>".repeat(n)),
        format!("const a = {}x;", "(y) => ".repeat(n / 3)),
        format!("type T = {}number;", "Array<".repeat(n / 2)),
        format!(
            "function f(): number {{ return {}; }}",
            "{a: ".repeat(n / 2)
        ),
    ];
    for body in inputs {
        let src = file(&body);
        // 構文として正しくなくてもよい。パニックせず、診断か AST が返ればよい。
        let _ = parse("main.cdl", &src, &Limits::default());
    }
}

#[test]
fn nesting_beyond_the_limit_is_reported() {
    for unit in ["(", "!", "[", "<A>", "`${", "a.", "x ? y : ", "(y) => "] {
        let src = file(&format!("const a = {}", unit.repeat(10_000)));
        assert!(codes(&src).contains(&Code::NestingTooDeep), "{unit}");
    }
}

/// 既定の上限（64）ちょうどの入れ子は通り、1 つ超えると E1008 になる（SPEC 第 7 章）。
#[test]
fn nesting_limit_boundary_at_the_default() {
    let limit = usize::try_from(Limits::default().max_nesting).unwrap();
    assert_eq!(limit, 64);
    // `const a =` の `=` も連鎖の 1 つに数えるので、括弧は上限 - 1 個まで書ける。
    let at = limit - 1;
    let inputs = |n: usize| {
        [
            format!("const a = {}x{};", "(".repeat(n), ")".repeat(n)),
            format!("const a = {}x{};", "[".repeat(n), "]".repeat(n)),
            format!("const a = {}x;", "!".repeat(n)),
            format!("const a = {}x{};", "<A>".repeat(n), "</A>".repeat(n)),
        ]
    };
    for src in inputs(at) {
        assert!(
            !codes(&file(&src)).contains(&Code::NestingTooDeep),
            "上限ちょうどは通る: {src}"
        );
    }
    for src in inputs(at + 1) {
        assert!(
            codes(&file(&src)).contains(&Code::NestingTooDeep),
            "上限 + 1 は E1008: {src}"
        );
    }
}

/// `Debug` の出力に現れるすべての span（AST のすべてのノード）を取り出す。
fn all_spans(m: &Module) -> Vec<(u32, u32)> {
    let debug = format!("{m:?}");
    let mut out = Vec::new();
    let mut rest = debug.as_str();
    while let Some(i) = rest.find("Span { start: ") {
        let tail = &rest[i + "Span { start: ".len()..];
        let (start, tail) = tail.split_once(", end: ").unwrap();
        let (end, tail) = tail.split_once(" }").unwrap();
        out.push((start.parse().unwrap(), end.parse().unwrap()));
        rest = tail;
    }
    out
}

fn slice(src: &str, span: Span) -> &str {
    &src[usize::try_from(span.start).unwrap()..usize::try_from(span.end).unwrap()]
}

/// すべてのノードが、元のソースの範囲内の、空でない span を持つ。
#[test]
fn every_node_has_a_source_span() {
    for src in [SPEC_6, SPEC_8, SPEC_14A] {
        let m = ok(src);
        let spans = all_spans(&m);
        assert!(spans.len() > 50, "{}", spans.len());
        for (start, end) in spans {
            assert!(
                start < end && usize::try_from(end).unwrap() <= src.len(),
                "{start}..{end}"
            );
            assert!(
                src.get(usize::try_from(start).unwrap()..usize::try_from(end).unwrap())
                    .is_some(),
                "文字の境界: {start}..{end}"
            );
        }
    }
}

#[test]
fn spans_cover_the_written_syntax() {
    let src = file(concat!(
        "import { A } from \"x\";\n",
        "export interface I { k?: number }\n",
        "type T = I[];\n",
        "function f(p: I): number { return p.k ?? 0; }\n",
        "const o = { a: 1, ...b, c };\n",
        "const xs = [...ys, 1];\n",
        "const e = <A k=\"v\" {...o} flag>{x}<></></A>;\n",
    ));
    let m = ok(&src);
    let s = |span| slice(&src, span);
    assert_eq!(s(m.span), src);
    let kinds: Vec<&ItemKind> = m.items.iter().map(|i| &i.kind).collect();
    let ItemKind::Import(i) = kinds[0] else {
        panic!()
    };
    assert_eq!(s(i.span), "import { A } from \"x\";");
    assert_eq!(s(m.items[1].span), "export interface I { k?: number }");
    let ItemKind::Interface(i) = kinds[1] else {
        panic!()
    };
    assert_eq!(s(i.span), "interface I { k?: number }");
    assert_eq!(s(i.members[0].span), "k?: number");
    let ItemKind::TypeAlias(t) = kinds[2] else {
        panic!()
    };
    assert_eq!(s(t.span), "type T = I[];");
    let ItemKind::Function(f) = kinds[3] else {
        panic!()
    };
    assert_eq!(s(f.span), "function f(p: I): number { return p.k ?? 0; }");
    assert_eq!(s(f.params[0].span), "p: I");
    let ItemKind::Const(c) = kinds[4] else {
        panic!()
    };
    let ExprKind::Object(props) = &c.init.kind else {
        panic!()
    };
    let ObjProp::KeyValue { span, .. } = &props[0] else {
        panic!()
    };
    assert_eq!(s(*span), "a: 1");
    let ObjProp::Spread(_, span) = &props[1] else {
        panic!()
    };
    assert_eq!(s(*span), "...b");
    let ItemKind::Const(c) = kinds[5] else {
        panic!()
    };
    let ExprKind::Array(elems) = &c.init.kind else {
        panic!()
    };
    let ArrayElem::Spread(_, span) = &elems[0] else {
        panic!()
    };
    assert_eq!(s(*span), "...ys");
    let ItemKind::Const(c) = kinds[6] else {
        panic!()
    };
    let ExprKind::Element(el) = &c.init.kind else {
        panic!()
    };
    assert_eq!(s(el.span), "<A k=\"v\" {...o} flag>{x}<></></A>");
    let Attr::Named { span, .. } = &el.attrs[0] else {
        panic!()
    };
    assert_eq!(s(*span), "k=\"v\"");
    let Attr::Spread(_, span) = &el.attrs[1] else {
        panic!()
    };
    assert_eq!(s(*span), "{...o}");
    let Attr::Named { span, .. } = &el.attrs[2] else {
        panic!()
    };
    assert_eq!(s(*span), "flag");
    let Child::Expr(_, span) = &el.children[0] else {
        panic!()
    };
    assert_eq!(s(*span), "{x}");
    let Child::Fragment(_, span) = &el.children[1] else {
        panic!()
    };
    assert_eq!(s(*span), "<></>");
}

fn helps(src: &str) -> Vec<Option<String>> {
    parse("main.cdl", src, &Limits::default())
        .unwrap_err()
        .into_iter()
        .map(|d| d.help)
        .collect()
}

/// 代わりの書き方がある誤りには help を付ける。
#[test]
fn diagnostics_suggest_alternatives() {
    let src = file(concat!(
        "let a = 1;\n",
        "const b = null;\n",
        "for (const x of xs) {}\n",
        "enum E { A }\n",
        "const c = x as number;\n",
        "const d = useState(0);\n",
        "const { e } = o;\n",
        "const f = x == y;\n",
        "const g = <A onClick={h} />;\n",
    ));
    let got = helps(&src);
    assert_eq!(got.len(), 9);
    assert!(got.iter().all(Option::is_some), "{got:#?}");
    assert!(got[0].as_deref().unwrap().contains("`const`"));
    assert!(got[1].as_deref().unwrap().contains("`undefined`"));
    assert!(got[2].as_deref().unwrap().contains("`map`"));
    assert!(got[4].as_deref().unwrap().contains("`satisfies`"));
    assert!(helps("const a = 1;")[0].is_some());
    assert!(helps("\"use cdl 1\"")[0].is_some());
}

/// ルートの関数の形（DECISIONS U1）。`export default function` はアロー関数と同じ AST になる。
#[test]
fn root_function_forms() {
    for src in [
        "export default function App({ output }: Env): Element<Root> { return <Root />; }\n",
        "export default function (env: Env): Element<Root> { return <Root />; }\n",
        "export default (env: Env): Element<Root> => <Root />;\n",
        "export default ({ output }: Env): Element<Root> => { return <Root />; };\n",
    ] {
        let m = ok(&file(src));
        let ItemKind::ExportDefault(e) = &m.items[0].kind else {
            panic!("{src}")
        };
        assert!(matches!(e.kind, ExprKind::Arrow { .. }), "{src}");
    }
    assert_eq!(
        codes(&file(
            "export default async function f(): number { return 1; }\n"
        )),
        [Code::ReservedWord]
    );
}
