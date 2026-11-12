//! 意味解析の結合テスト（SPEC 第 4・6・7 章）。スキーマはテストの中でビルダーで組み立てる。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_diag::Code;
use cdl_sema::schema::{ComponentDef, Schema};
use cdl_sema::syntax::{Limits, parse};
use cdl_sema::types::{Field, Ty};
use cdl_sema::{Checked, check};

fn schema() -> Schema {
    let window = Ty::object([("appId", Field::required(Ty::String))]);
    let placement = Ty::object([("floating", Field::required(Ty::Boolean))]);
    let m = "host:t";
    Schema::builder()
        .root(m, "Root")
        .component(
            m,
            ComponentDef::new("Root")
                .children(&["Item", "Panel", "Label"])
                .unique_child("Panel"),
        )
        .component(
            m,
            ComponentDef::new("Label")
                .children(&["Item"])
                .text_children(),
        )
        .component(
            m,
            ComponentDef::new("Panel")
                .prop("gap", Ty::Length)
                .children(&["Item"]),
        )
        .component(
            m,
            ComponentDef::new("Item")
                .prop("name", Ty::String)
                .optional_prop("weight", Ty::Number),
        )
        .type_def(
            m,
            "Policy",
            Ty::object([(
                "open",
                Field::optional(Ty::Function(vec![window], Box::new(placement))),
            )]),
        )
        .build()
        .unwrap()
}

const HEAD: &str = "\"use cdl 1\";\nimport { Root, Panel, Item, Label } from \"host:t\";\nimport type { Policy } from \"host:t\";\n";

fn run(body: &str) -> Result<Checked, Vec<(Code, String)>> {
    let src = format!("{HEAD}{body}");
    let module =
        parse("main.cdl", &src, &Limits::default()).unwrap_or_else(|d| panic!("{src}\n{d:#?}"));
    check(&module, &schema()).map_err(|ds| {
        ds.into_iter()
            .map(|d| {
                let start = usize::try_from(d.span.start).unwrap();
                let end = usize::try_from(d.span.end).unwrap();
                (d.code, src[start..end].to_owned())
            })
            .collect()
    })
}

fn ok(body: &str) -> Checked {
    run(body).unwrap_or_else(|e| panic!("{body}\n{e:#?}"))
}

fn errs(body: &str) -> Vec<(Code, String)> {
    run(body).err().unwrap_or_default()
}

const ROOT: &str = "export default (<Root />);\n";

#[test]
fn const_order_puts_dependencies_first_and_keeps_source_order_otherwise() {
    let c = ok(&format!(
        "const c = b + a;\nconst a = 1;\nconst b = a * 2;\nconst d = 0;\n{ROOT}"
    ));
    assert_eq!(c.const_order, ["a", "b", "c", "d"]);
}

#[test]
fn several_errors_are_reported_and_do_not_cascade() {
    let got = errs(&format!(
        "const a: number = \"x\";\nconst b = a + 1;\nconst c: string = 1;\nconst d = px(1) * px(1);\nconst e = d + px(1);\n{ROOT}"
    ));
    // `b` と `e` は、前の誤りに引きずられて報告しない。
    assert_eq!(
        got,
        [
            (Code::TypeMismatch, "\"x\"".to_owned()),
            (Code::TypeMismatch, "1".to_owned()),
            (Code::BadOperand, "px(1) * px(1)".to_owned()),
        ]
    );
}

#[test]
fn name_errors_stop_before_type_checking() {
    let got = errs(&format!(
        "const a = missing;\nconst b: number = \"x\";\n{ROOT}"
    ));
    assert_eq!(got, [(Code::UndefinedName, "missing".to_owned())]);
}

#[test]
fn policy_hooks_must_match_exactly() {
    ok(&format!(
        "export const policy = {{\n  open: (w: {{ appId: string }}): {{ floating: boolean }} => ({{ floating: w.appId === \"x\" }}),\n}} satisfies Policy;\n{ROOT}"
    ));
    // 文脈から引数と戻り値の型が決まる。
    ok(&format!(
        "const p = {{ open: (w) => ({{ floating: true }}) }} satisfies Policy;\n{ROOT}"
    ));
    let got = errs(&format!(
        "const p = {{ open: (w: {{ appId: \"a\" }}): {{ floating: boolean }} => ({{ floating: true }}) }} satisfies Policy;\n{ROOT}"
    ));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, Code::TypeMismatch);
    let got = errs(&format!(
        "export const policy = {{ close: 1 }} satisfies Policy;\n{ROOT}"
    ));
    assert_eq!(got, [(Code::UnknownHook, "close".to_owned())]);
}

#[test]
fn tuples_records_and_optional_chaining() {
    ok(&format!(
        "const t: [number, string] = [1, \"a\"];\nconst n: number = t[0];\nconst s: string = t[1];\n\
         const r: Record<string, number> = {{ a: 1 }};\nconst m: number | undefined = r[\"a\"];\n\
         interface W {{ name: string }}\nconst w: W | undefined = undefined;\nconst k: string | undefined = w?.name;\n{ROOT}"
    ));
    let got = errs(&format!(
        "const r: Record<string, number> = {{ a: 1 }};\nconst m = r.a;\n{ROOT}"
    ));
    assert_eq!(got, [(Code::UnknownProperty, "a".to_owned())]);
}

#[test]
fn narrowing_through_logical_operators_and_member_paths() {
    ok(&format!(
        "interface O {{ inner: {{ v: number | undefined }} }}\n\
         function f(o: O): number {{\n  return o.inner.v !== undefined && o.inner.v > 1 ? o.inner.v : 0;\n}}\n\
         function g(x: number | undefined): boolean {{\n  return x === undefined || x > 0;\n}}\n{ROOT}"
    ));
}

#[test]
fn array_methods_are_typed() {
    ok(&format!(
        "const xs = [1, 2, 3];\nconst total: number = xs.reduce((acc, x) => acc + x, 0);\n\
         const names: string[] = xs.map((x) => `${{x}}`);\nconst big: number[] = xs.filter((x) => x > 1);\n\
         const has: boolean = xs.includes(2);\nconst all: number[] = xs.concat([4]);\nconst n: number = xs.length;\n{ROOT}"
    ));
    let got = errs(&format!(
        "const xs = [1];\nconst a = xs.filter((x) => x);\nconst b = xs.push(1);\n{ROOT}"
    ));
    let codes: Vec<Code> = got.iter().map(|(c, _)| *c).collect();
    // 配列にないメソッドは、未知のプロパティ（DECISIONS B15）。
    assert_eq!(
        codes,
        [Code::TypeMismatch, Code::UnknownProperty],
        "{got:?}"
    );
}

#[test]
fn equality_rules() {
    let got = errs(&format!(
        "const o = {{ a: 1 }};\nconst a = o === o;\nconst b = 1 === \"1\";\nconst c = [1] === [1];\n{ROOT}"
    ));
    let codes: Vec<Code> = got.iter().map(|(c, _)| *c).collect();
    assert_eq!(codes, [Code::BadOperand; 3], "{got:?}");
}

#[test]
fn schema_children_unique_and_root() {
    ok(
        "export default (<Root><Panel gap={px(1)}><Item name=\"a\" /></Panel>{[1, 2].map((n) => <Item name={`${n}`} weight={n} />)}</Root>);\n",
    );
    let got =
        errs("export default (<Root><Panel gap={px(1)} /><Panel gap={px(2)} /><Root /></Root>);\n");
    let codes: Vec<Code> = got.iter().map(|(c, _)| *c).collect();
    assert_eq!(
        codes,
        [Code::DuplicateUniqueChild, Code::HostChildType],
        "{got:?}"
    );
    let got = errs("export default (<Panel gap={px(1)} />);\n");
    assert_eq!(
        got,
        [(Code::RootElement, "<Panel gap={px(1)} />".to_owned())]
    );
    let got = errs(&format!("{ROOT}{ROOT}"));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, Code::RootElement);
}

#[test]
fn user_components_with_children() {
    ok(
        "function Group(props: { children: Element<Item>[] }): Element<Panel> {\n  return <Panel gap={px(1)}>{props.children}</Panel>;\n}\n\
         export default (<Root><Group><Item name=\"a\" /></Group></Root>);\n",
    );
    let got = errs(
        "function Group(props: { children: Element<Item>[] }): Element<Panel> {\n  return <Panel gap={px(1)}>{props.children}</Panel>;\n}\n\
         export default (<Root><Group children={[]}><Item name=\"a\" /></Group></Root>);\n",
    );
    assert!(
        got.iter().any(|(c, _)| *c == Code::ChildrenTwice),
        "{got:?}"
    );
    let got = errs(&format!(
        "function Bad(props: {{}}): number {{\n  return 1;\n}}\nconst X = 1;\nconst e = <X />;\n{ROOT}"
    ));
    let codes: Vec<Code> = got.iter().map(|(c, _)| *c).collect();
    assert_eq!(codes, [Code::TypeMismatch, Code::NotAComponent], "{got:?}");
}

#[test]
fn checking_is_deterministic() {
    let body = format!(
        "const a: number = \"x\";\nconst b = <Item name={{1}} />;\nconst c = [1, \"a\"];\n{ROOT}"
    );
    assert_eq!(run(&body), run(&body));
}

/// 構文の入れ子の上限ちょうどの式も、型検査でスタックを使い切らない（テストのスレッドは 2 MiB）。
#[test]
fn deep_expressions_at_the_nesting_limit_do_not_overflow() {
    let n = 120;
    let bodies = [
        format!("const a = {}1{};", "[".repeat(n), "]".repeat(n)),
        format!("const a = {}true;", "!".repeat(n)),
        format!("const a = 1{};", " + 1".repeat(n / 2)),
        format!("const a = {}0;", "true ? 1 : ".repeat(n / 2)),
        format!(
            "const a = {}1{};",
            "{ k: ".repeat(n / 2),
            " }".repeat(n / 2)
        ),
        format!(
            "const a = <Root>{}{}</Root>;",
            "<>".repeat(n / 2),
            "</>".repeat(n / 2)
        ),
    ];
    for body in bodies {
        let src = format!("{HEAD}{body}\n{ROOT}");
        if let Ok(module) = parse("main.cdl", &src, &Limits::default()) {
            let _ = check(&module, &schema());
        }
    }
}

// ---- React 式の宣言的 UI（DECISIONS R1–R4） -----------------------------------------

fn codes_of(body: &str) -> Vec<Code> {
    errs(body).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn text_children_need_a_component_that_accepts_text() {
    ok(&format!(
        "const a = <Root><Label>hi</Label><Label>{{\"x\"}}</Label></Root>;\n{ROOT}"
    ));
    // 受け付けないホストのコンポーネント（Item は子を持たず、Panel の子は Item だけ）。
    let got = errs(&format!(
        "const a = <Root><Panel gap={{px(1)}}>text</Panel><Root>x</Root></Root>;\n{ROOT}"
    ));
    assert!(
        got.iter()
            .any(|(c, t)| *c == Code::TextChildNotAccepted && t == "text"),
        "{got:?}"
    );
    // 式で書いた string も同じ。number は子に置けない（E4005）。
    assert_eq!(
        codes_of(&format!(
            "const a = <Panel gap={{px(1)}}>{{\"x\"}}</Panel>;\n{ROOT}"
        )),
        [Code::TextChildNotAccepted]
    );
    assert_eq!(
        codes_of(&format!("const a = <Label>{{1}}</Label>;\n{ROOT}")),
        [Code::HostChildType]
    );
}

#[test]
fn user_components_take_text_only_through_string_children() {
    ok(&format!(
        "function T(props: {{ children: string[] }}): Element<Label> {{ return <Label>{{props.children}}</Label>; }}\n\
         const a = <Root><T>hello</T></Root>;\n{ROOT}"
    ));
    // children が Element の配列なら、テキストは置けない。
    assert_eq!(
        codes_of(&format!(
            "function G(props: {{ children: Element<Item>[] }}): Element<Panel> {{ return <Panel gap={{px(1)}}>{{props.children}}</Panel>; }}\n\
             const a = <G>text</G>;\n{ROOT}"
        )),
        [Code::TextChildNotAccepted]
    );
    // children を持たない props にも置けない。
    assert_eq!(
        codes_of(&format!(
            "function N(props: {{}}): Element<Item> {{ return <Item name=\"a\" />; }}\nconst a = <N>text</N>;\n{ROOT}"
        )),
        [Code::TextChildNotAccepted]
    );
    // `children: string[]` に要素は置けない。`(Element<…> | string)[]` なら、要素とテキストを混ぜられる（DECISIONS U2）。
    let got = codes_of(&format!(
        "function T(props: {{ children: string[] }}): Element<Label> {{ return <Label>{{props.children}}</Label>; }}\n\
         const a = <T>text<Item name=\"a\" /></T>;\n{ROOT}"
    ));
    assert_eq!(got, [Code::TextChildNotAccepted], "{got:?}");
    ok(&format!(
        "function M(props: {{ children: (Element<Item> | string)[] }}): Element<Label> {{ return <Label>{{props.children}}</Label>; }}\n\
         const a = <Root><M>text<Item name=\"a\" />more</M></Root>;\n{ROOT}"
    ));
}

#[test]
fn and_has_a_boolean_left_side_and_a_false_or_b_type() {
    // `c && <A />` は子に置ける。配列の中でも、変数に入れても同じ。
    ok(&format!(
        "const c = true;\nconst x = c && <Item name=\"a\" />;\n\
         const a = <Root>{{c && <Item name=\"a\" />}}{{x}}{{[c && <Item name=\"b\" />, c && <Item name=\"c\" />]}}{{false}}{{undefined}}</Root>;\n{ROOT}"
    ));
    // 型は `false | B`。
    let got = errs(&format!(
        "const c = true;\nconst x: number = c && 1;\n{ROOT}"
    ));
    assert_eq!(got.len(), 1, "{got:?}");
    // boolean どうしの `&&` は boolean のまま。
    ok(&format!(
        "const a: boolean = true && false;\nconst b: boolean = (1 < 2) && (2 < 3);\n{ROOT}"
    ));
    // 左辺が boolean でなければ E3006。
    let n = "const n: number = 1;\nconst s: string = \"a\";\nconst m: boolean | undefined = undefined;\n";
    assert_eq!(
        errs(&format!("{n}const a = n && <Item name=\"a\" />;\n{ROOT}")).len(),
        1
    );
    for lhs in ["n", "s", "m"] {
        assert_eq!(
            codes_of(&format!(
                "{n}const a = {lhs} && <Item name=\"a\" />;\n{ROOT}"
            )),
            [Code::BadOperand],
            "{lhs}"
        );
    }
    // `||` は従来どおり両辺 boolean。
    assert_eq!(
        codes_of(&format!("const a = true || <Item name=\"a\" />;\n{ROOT}")),
        [Code::BadOperand]
    );
    // 左辺で絞り込んだ名前を右辺で使える。
    ok(&format!(
        "const v: string | undefined = undefined;\nconst a = <Root>{{v !== undefined && <Item name={{v}} />}}</Root>;\n{ROOT}"
    ));
}

#[test]
fn true_and_boolean_are_not_children() {
    assert_eq!(
        codes_of(&format!("const a = <Root>{{true}}</Root>;\n{ROOT}")),
        [Code::HostChildType]
    );
    assert_eq!(
        codes_of(&format!(
            "const b: boolean = true;\nconst a = <Root>{{b}}</Root>;\n{ROOT}"
        )),
        [Code::HostChildType]
    );
}

#[test]
fn key_is_a_string_or_number_and_not_a_prop() {
    ok(&format!(
        "const a = <Root><Item key=\"a\" name=\"a\" /><Item key={{\"b\"}} name=\"b\" /><Item key={{`k${{1}}`}} name=\"c\" /></Root>;\n{ROOT}"
    ));
    for bad in ["{true}", "{undefined}", "{[1]}", ""] {
        let attr = if bad.is_empty() {
            "key".to_owned()
        } else {
            format!("key={bad}")
        };
        assert_eq!(
            codes_of(&format!("const a = <Item {attr} name=\"a\" />;\n{ROOT}")),
            [Code::TypeMismatch],
            "{attr}"
        );
    }
    // number は E3017（help に、テンプレート文字列の書き方）。
    let n = "const n = 3;\nconst m: number = 4;\n";
    for key in ["{1}", "{n}", "{m}", "{1 + 2}"] {
        assert_eq!(
            codes_of(&format!(
                "{n}const a = <Item key={key} name=\"a\" />;\n{ROOT}"
            )),
            [Code::KeyNotString],
            "{key}"
        );
    }
    // 静的な重複（文字列リテラルの key どうし）は、すべて報告する。
    let got = codes_of(&format!(
        "const a = <Root><Item key=\"a\" name=\"1\" /><><Item key=\"a\" name=\"2\" /></><Item key={{\"a\"}} name=\"3\" /><Item key=\"b\" name=\"4\" /></Root>;\n{ROOT}"
    ));
    assert_eq!(got, [Code::DuplicateChildKey; 2]);
    // 動的な key は、静的には比べない。
    ok(&format!(
        "const k = \"a\";\nconst a = <Root><Item key={{k}} name=\"1\" /><Item key={{k}} name=\"2\" /></Root>;\n{ROOT}"
    ));
    // 重複する属性。
    assert_eq!(
        codes_of(&format!(
            "const a = <Item key=\"a\" key=\"b\" name=\"a\" />;\n{ROOT}"
        )),
        [Code::DuplicateKey]
    );
    // props.key は読めず、props の型にも書けない。
    assert_eq!(
        codes_of(&format!(
            "function R(props: {{ n: string }}): Element<Item> {{ return <Item name={{props.key}} />; }}\n{ROOT}"
        )),
        [Code::UnknownProperty]
    );
    assert_eq!(
        codes_of(&format!(
            "function D(props: {{ key: string }}): Element<Item> {{ return <Item name=\"a\" />; }}\n{ROOT}"
        )),
        [Code::UnknownProperty]
    );
    // コンポーネントにも付けられる。
    ok(&format!(
        "function C(props: {{ n: string }}): Element<Item> {{ return <Item name={{props.n}} />; }}\n\
         const a = <Root><C key=\"k\" n=\"a\" /></Root>;\n{ROOT}"
    ));
}

#[test]
fn object_pattern_params_are_typed_by_their_annotation() {
    let head = "interface P { name: string; label?: string; n?: number }\n";
    // 既定値を付けた名前は、本体で undefined が除かれる。付けなければ除かれない。
    ok(&format!(
        "{head}function f({{ name, label = \"x\" }}: P): string {{ return `${{name}}${{label}}`; }}\n{ROOT}"
    ));
    assert_eq!(
        codes_of(&format!(
            "{head}function f({{ label }}: P): string {{ return `${{label}}`; }}\n{ROOT}"
        )),
        [Code::MaybeUndefined]
    );
    // 省略可能でない prop への既定値。
    assert_eq!(
        codes_of(&format!(
            "{head}function f({{ name = \"x\" }}: P): string {{ return name; }}\n{ROOT}"
        )),
        [Code::DefaultOnRequired]
    );
    // 既定値の型・props にない名前・オブジェクトでない型・注釈なし。
    assert_eq!(
        codes_of(&format!(
            "{head}function f({{ n = \"x\" }}: P): number {{ return n; }}\n{ROOT}"
        )),
        [Code::TypeMismatch]
    );
    assert_eq!(
        codes_of(&format!(
            "{head}function f({{ nope }}: P): number {{ return 1; }}\n{ROOT}"
        )),
        [Code::UnknownProperty]
    );
    assert_eq!(
        codes_of(&format!(
            "function f({{ a }}: string): number {{ return 1; }}\n{ROOT}"
        )),
        [Code::TypeMismatch]
    );
    assert_eq!(
        codes_of(&format!(
            "function f({{ a }}): number {{ return 1; }}\n{ROOT}"
        )),
        [Code::AnnotationRequired]
    );
    // コールバックでも、分割代入の引数は注釈が要る。
    assert_eq!(
        codes_of(&format!(
            "const xs: {{ a: number }}[] = [];\nconst ys = xs.map(({{ a }}) => a);\n{ROOT}"
        )),
        [Code::AnnotationRequired]
    );
    ok(&format!(
        "const xs: {{ a: number }}[] = [];\nconst ys = xs.map(({{ a }}: {{ a: number }}) => a);\n{ROOT}"
    ));
    // 既定値は、同じ関数の引数を参照できない（名前解決の誤り）。
    assert_eq!(
        codes_of(&format!(
            "function f({{ a, b = a }}: {{ a: number; b?: number }}): number {{ return b; }}\n{ROOT}"
        )),
        [Code::UndefinedName]
    );
    // コンポーネントの props にも使える。
    ok(&format!(
        "function C({{ n, w = 1 }}: {{ n: string; w?: number }}): Element<Item> {{ return <Item name={{n}} weight={{w}} />; }}\n\
         const a = <Root><C n=\"a\" /></Root>;\n{ROOT}"
    ));
}

#[test]
fn fragments_with_text_have_a_string_array_type() {
    ok(&format!(
        "const t = <>one</>;\nconst a = <Root><Label>{{t}}</Label><Label><>x</></Label></Root>;\n{ROOT}"
    ));
    // 受け付けないコンポーネントには置けない（フラグメントを経由しても、型の上で string[] のまま）。
    assert_eq!(
        codes_of(&format!(
            "const t = <>one</>;\nconst a = <Panel gap={{px(1)}}>{{t}}</Panel>;\n{ROOT}"
        )),
        [Code::TextChildNotAccepted]
    );
    assert_eq!(
        codes_of(&format!("const a = <Root><>text</></Root>;\n{ROOT}")),
        [Code::TextChildNotAccepted]
    );
    // 要素とテキストの混在は `(Element<…> | string)[]`。
    ok(&format!(
        "const f = <>text<Item name=\"a\" /></>;\nconst a = <Label>{{f}}</Label>;\n{ROOT}"
    ));
}

#[test]
fn element_or_string_is_a_valid_union_and_other_mixes_are_not() {
    ok(&format!(
        "const a: (Element<Item> | string)[] = [\"x\", <Item name=\"a\" />];\n\
         const b: Element<Item> | string = \"s\";\nconst c: Element<Item> | undefined = undefined;\n{ROOT}"
    ));
    assert_eq!(
        codes_of(&format!("const a: Element<Item> | number = 1;\n{ROOT}")),
        [Code::InvalidType]
    );
    // 配列リテラルの要素が要素とテキストなら、`(Element<…> | string)[]`。
    ok(&format!(
        "const a = [\"x\", <Item name=\"a\" />];\nconst r = <Root><Label>{{a}}</Label></Root>;\n{ROOT}"
    ));
}

#[test]
fn slots_are_typed_props() {
    let defs = "function Card(props: { head: Element<Item>; rest?: Element<Item>[] }): Element<Panel> {\n  return <Panel gap={px(1)}>{props.head}</Panel>;\n}\n";
    ok(&format!(
        "{defs}const a = <Card head={{<Item name=\"a\" />}} rest={{[<Item name=\"b\" />]}} />;\n{ROOT}"
    ));
    assert_eq!(
        codes_of(&format!(
            "{defs}const a = <Card head={{<Panel gap={{px(1)}} />}} />;\n{ROOT}"
        )),
        [Code::SlotType]
    );
    assert_eq!(
        codes_of(&format!(
            "{defs}const a = <Card head={{<Item name=\"a\" />}} rest={{[<Panel gap={{px(1)}} />]}} />;\n{ROOT}"
        )),
        [Code::SlotType]
    );
    // 要素でない値は、従来の型の誤り。
    assert_eq!(
        codes_of(&format!("{defs}const a = <Card head={{1}} />;\n{ROOT}")),
        [Code::TypeMismatch]
    );
}

#[test]
fn whitespace_only_text_is_never_a_child() {
    // テキストを受け付けないコンポーネントの子の間の空白は、型エラーにならない（DECISIONS U5）。
    ok(&format!(
        "const a = <Root><Item name=\"a\" /> <Item name=\"b\" />\n  <Item name=\"c\" />   </Root>;\n{ROOT}"
    ));
    // 空白以外のテキストは E3014。
    assert_eq!(
        codes_of(&format!(
            "const a = <Panel gap={{px(1)}}> x </Panel>;\n{ROOT}"
        )),
        [Code::TextChildNotAccepted]
    );
}
