//! 評価の結合テスト（SPEC 第 7・8 章）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_diag::{Code, Diagnostic};
use cdl_eval::sema::schema::{ComponentDef, Schema};
use cdl_eval::sema::syntax::{self, ast::Module};
use cdl_eval::sema::types::{Field, Ty};
use cdl_eval::sema::{self as sema, Checked};
use cdl_eval::{Evaluated, Limits, Value, call_hook, evaluate};
use proptest::prelude::*;

/// import のない 1 つのファイル。
static NO_DEPS: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

fn unit<'a>(
    module: &'a cdl_eval::sema::syntax::ast::Module,
    checked: &'a cdl_eval::sema::Checked,
) -> cdl_eval::Unit<'a> {
    cdl_eval::Unit {
        module,
        checked,
        deps: &NO_DEPS,
    }
}

fn schema() -> Schema {
    let window = Ty::object([("appId", Field::required(Ty::String))]);
    let m = "host:t";
    Schema::builder()
        .root(m, "Root")
        .component(
            m,
            ComponentDef::new("Root")
                .children(&["Item", "Row", "Label"])
                .unique_child("Item"),
        )
        .component(m, ComponentDef::new("Item").prop("name", Ty::String))
        .component(
            m,
            ComponentDef::new("Row")
                .optional_prop("n", Ty::Number)
                .children(&["Row", "Label"]),
        )
        .component(
            m,
            ComponentDef::new("Label")
                .children(&["Row"])
                .text_children(),
        )
        .type_def(
            m,
            "Policy",
            Ty::object([
                (
                    "open",
                    Field::optional(Ty::Function(vec![window], Box::new(Ty::Number))),
                ),
                (
                    "close",
                    Field::optional(Ty::Function(vec![Ty::Number], Box::new(Ty::Number))),
                ),
            ]),
        )
        .build()
        .unwrap()
}

const HEAD: &str = "\"use cdl 1\";\nimport { Root, Item, Row, Label } from \"host:t\";\nimport type { Policy } from \"host:t\";\n";

fn load(body: &str) -> (Module, Checked) {
    let src = format!("{HEAD}{body}");
    let module = syntax::parse("main.cdl", &src, &syntax::Limits::default())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let checked = sema::check(&module, &schema()).unwrap_or_else(|d| panic!("{src}\n{d:#?}"));
    (module, checked)
}

/// すべての誤り。
fn run_all(body: &str, limits: &Limits) -> Result<Evaluated, Vec<Diagnostic>> {
    let (module, checked) = load(body);
    evaluate(&[unit(&module, &checked)], 0, &schema(), limits)
}

/// 最初の誤りだけを見る。
fn run(body: &str, limits: &Limits) -> Result<Evaluated, Diagnostic> {
    run_all(body, limits).map_err(|mut ds| ds.remove(0))
}

fn code(body: &str, limits: &Limits) -> Code {
    run(body, limits)
        .err()
        .map(|d| d.code)
        .unwrap_or_else(|| panic!("誤りにならない: {body}"))
}

const ROOT: &str = "export default (<Root />);\n";

#[test]
fn components_are_expanded_and_spans_are_kept() {
    let body = "function Named(props: { n: string }): Element<Item> {\n  return <Item name={props.n} />;\n}\n\
                function Outer(props: {}): Element<Item> {\n  return <Named n=\"x\" />;\n}\n\
                export default (<Root><Outer /></Root>);\n";
    let src = format!("{HEAD}{body}");
    let ev = run(body, &Limits::default()).unwrap();
    let item = ev.document.as_ref().unwrap().children[0]
        .as_element()
        .unwrap();
    assert_eq!(item.name, "Item");
    assert_eq!(item.props["name"], Value::string("x"));
    let slice = |s: &cdl_diag::Span| {
        &src[usize::try_from(s.start).unwrap()..usize::try_from(s.end).unwrap()]
    };
    assert_eq!(slice(&item.span), "<Item name={props.n} />");
    // 内側の呼び出しから順に、展開したコンポーネントの位置を持つ（DECISIONS D6）。
    let from: Vec<&str> = item.expanded_from.iter().map(slice).collect();
    assert_eq!(from, ["<Named n=\"x\" />", "<Outer />"]);
}

#[test]
fn limits_can_be_overridden() {
    let small = |f: fn(&mut Limits)| {
        let mut l = Limits::default();
        f(&mut l);
        l
    };
    let body =
        format!("const xs = [1, 2, 3, 4];\nconst ys = xs.map((x) => `${{x}}${{x}}`);\n{ROOT}");
    assert!(run(&body, &Limits::default()).is_ok());
    assert_eq!(code(&body, &small(|l| l.max_steps = 10)), Code::StepLimit);
    assert_eq!(
        code(&body, &small(|l| l.max_string_bytes = 1)),
        Code::StringLimit
    );
    assert_eq!(
        code(&body, &small(|l| l.max_collection_len = 3)),
        Code::CollectionLimit
    );
    assert_eq!(
        code(&body, &small(|l| l.max_call_depth = 0)),
        Code::CallDepth
    );
    let doc = "export default (<Root><Item name=\"a\" /></Root>);\n";
    assert!(run(doc, &small(|l| l.max_nodes = 3)).is_ok());
    assert_eq!(code(doc, &small(|l| l.max_nodes = 2)), Code::NodeLimit);
}

/// 値木の深さの上限（SPEC 第 7 章）: ちょうどなら成功、+ 1 なら E5011。
#[test]
fn value_depth_boundary() {
    let with = |max: u32| Limits {
        max_value_depth: max,
        ..Limits::default()
    };
    // 配列 3 段は深さ 3。
    let arrays = format!("const a = [[[1]]];\n{ROOT}");
    assert!(run(&arrays, &with(3)).is_ok());
    assert_eq!(code(&arrays, &with(2)), Code::ValueDepth);
    // オブジェクトも 1 段と数える。
    let objects = format!("const a = {{ x: {{ y: {{ z: 1 }} }} }};\n{ROOT}");
    assert!(run(&objects, &with(3)).is_ok());
    assert_eq!(code(&objects, &with(2)), Code::ValueDepth);
    // 要素も 1 段と数える。`<Root>` だけで深さ 1、子を 1 つ置くと 2。
    let element = "export default (<Root><Item name=\"a\" /></Root>);\n";
    assert!(run(element, &with(2)).is_ok());
    assert_eq!(code(element, &with(1)), Code::ValueDepth);
    assert!(run(ROOT, &with(1)).is_ok());
    assert_eq!(code(ROOT, &with(0)), Code::ValueDepth);
    // map の結果も検査する。
    let mapped = format!("const a = [1].map((x) => [[x]]);\n{ROOT}");
    assert!(run(&mapped, &with(3)).is_ok());
    assert_eq!(code(&mapped, &with(2)), Code::ValueDepth);
}

/// 構文の入れ子の上限を、`const` の連鎖で抜けても、既定の上限（64）で止まる。
#[test]
fn value_depth_default_bounds_const_chains() {
    let chain = |n: usize| {
        let mut body = String::from("const a0 = [1];\n");
        for i in 1..n {
            body.push_str(&format!("const a{i} = [a{}];\n", i - 1));
        }
        body.push_str(ROOT);
        body
    };
    assert_eq!(Limits::default().max_value_depth, 64);
    assert!(run(&chain(64), &Limits::default()).is_ok());
    assert_eq!(code(&chain(65), &Limits::default()), Code::ValueDepth);
    assert_eq!(code(&chain(100), &Limits::default()), Code::ValueDepth);
    // 型の深さの上限（128）を超える連鎖は、評価の前に意味解析が E3013 で止める（AUDIT P2）。
    let src = format!("{HEAD}{}", chain(5_000));
    let module = syntax::parse("main.cdl", &src, &syntax::Limits::default()).unwrap();
    let diags = sema::check(&module, &schema()).unwrap_err();
    assert!(diags.iter().all(|d| d.code == Code::TypeTooLarge));
}

#[test]
fn limit_errors_point_at_the_declaration() {
    let body = format!("const ok = 1;\nconst heavy = [1, 2, 3].map((x) => x + 1);\n{ROOT}");
    let l = Limits {
        max_steps: 8,
        ..Limits::default()
    };
    let d = run(&body, &l).unwrap_err();
    let src = format!("{HEAD}{body}");
    let start = usize::try_from(d.span.start).unwrap();
    assert_eq!(&src[start..start + 5], "heavy");
}

#[test]
fn arithmetic_errors() {
    let l = Limits::default();
    for (expr, want) in [
        ("0 / 0", Code::DivisionByZero),
        ("1e308 + 1e308", Code::NonFinite),
        ("-1e308 * 10", Code::NonFinite),
        ("px(1e308) * 10", Code::NonFinite),
        ("ms(9007199254740992) + ms(1)", Code::DurationRange),
        ("ms(1) * 0.5", Code::DurationRange),
        ("sec(0.0001)", Code::DurationRange),
        ("color(\"#12\")", Code::InvalidColor),
    ] {
        assert_eq!(
            code(&format!("const a = {expr};\n{ROOT}"), &l),
            want,
            "{expr}"
        );
    }
    // 負の Duration は許す（DECISIONS B11）。
    assert!(run(&format!("const a = ms(-1);\n{ROOT}"), &l).is_ok());
}

#[test]
fn values_follow_the_decisions() {
    let body = "const o = { a: 1, b: undefined };\nconst p = { ...o, a: undefined };\nconst q = { a: 2, ...o };\n\
                export default (<Root><Item name={`${-0} ${p.a ?? \"none\"} ${q.a} ${[1, 2][5] ?? 9}`} /></Root>);\n";
    let ev = run(body, &Limits::default()).unwrap();
    // -0 は 0、明示の undefined はキーがないのと同じ、スプレッドの undefined は上書きしない。
    assert_eq!(
        ev.document.as_ref().unwrap().children[0]
            .as_element()
            .unwrap()
            .props["name"],
        Value::string("0 none 1 9")
    );
}

#[test]
fn unique_children_are_checked_after_evaluation() {
    let body = "export default (<Root>{[\"a\", \"b\"].map((n) => <Item name={n} />)}</Root>);\n";
    assert_eq!(code(body, &Limits::default()), Code::DuplicateUniqueChild);
}

#[test]
fn hooks_are_called_with_their_own_budget() {
    let body = "const xs = [1, 2, 3, 4, 5, 6, 7, 8];\nconst sum = xs.reduce((a, x) => a + x, 0);\n\
                export const policy = {\n  open: (w: { appId: string }): number => w.appId === \"big\" ? sum : 0,\n} satisfies Policy;\n"
        .to_owned()
        + ROOT;
    let (module, checked) = load(&body);
    let mut limits = Limits::default();
    let ev = evaluate(&[unit(&module, &checked)], 0, &schema(), &limits).unwrap();
    let call = |limits: &Limits, arg: Value| {
        call_hook(
            &[unit(&module, &checked)],
            0,
            &schema(),
            &ev,
            limits,
            "open",
            arg,
        )
    };
    let arg = |s: &str| Value::object([("appId", Value::string(s))]);
    assert_eq!(call(&limits, arg("big")), Ok(Some(Value::Number(36.0))));
    assert_eq!(call(&limits, arg("x")), Ok(Some(Value::Number(0.0))));
    // 定義されていないフックは None（ホストの既定の動作）。
    assert_eq!(
        call_hook(
            &[unit(&module, &checked)],
            0,
            &schema(),
            &ev,
            &limits,
            "close",
            Value::Number(1.0)
        ),
        Ok(None)
    );
    // 引数が型に合わなければ E4007。
    assert_eq!(
        call(&limits, Value::Number(1.0)).unwrap_err().code,
        Code::HookArgument
    );
    // ホストが渡した引数の深さも、上限で抑える。
    let mut deep = arg("deep");
    for _ in 0..limits.max_value_depth {
        deep = Value::object([("appId", deep)]);
    }
    assert_eq!(call(&limits, deep).unwrap_err().code, Code::ValueDepth);
    // 呼び出しごとの上限。ドキュメントの評価に使った分は数えない。
    limits.max_hook_steps = 2;
    assert_eq!(call(&limits, arg("big")).unwrap_err().code, Code::StepLimit);
}

#[test]
fn evaluation_is_deterministic() {
    let body = "const r: Record<string, number> = { b: 2, a: 1 };\n\
                export default (<Root><Item name={`${r[\"a\"] ?? 0}${0.1 + 0.2}`} /></Root>);\n";
    let a = run(body, &Limits::default()).unwrap();
    let b = run(body, &Limits::default()).unwrap();
    assert_eq!(a.document, b.document);
}

// ---- 任意の入力で完了する（SPEC 第 12 章） -----------------------------------------

/// 構文解析・型検査・評価を通す。どこで誤りになってもよいが、パニックせずに戻ること。
fn pipeline(src: &str) {
    let Ok(module) = syntax::parse("main.cdl", src, &syntax::Limits::default()) else {
        return;
    };
    let Ok(checked) = sema::check(&module, &schema()) else {
        return;
    };
    let _ = evaluate(&[unit(&module, &checked)], 0, &schema(), &Limits::default());
}

/// CDL らしい断片。組み合わせて、評価の奥まで届く入力を作る。
const TOKENS: &[&str] = &[
    "const",
    "function",
    "return",
    "export default",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "<Root>",
    "</Root>",
    "<Item name=\"a\" />",
    "<",
    ">",
    "=",
    "=>",
    ":",
    ";",
    ",",
    ".map(",
    "...",
    "?",
    "+",
    "/",
    "x",
    "f",
    "1",
    "0",
    "\"s\"",
    "`${",
    "}`",
    "px(1)",
    "ms(1)",
    "color(\"#fff\")",
    "\n",
];

proptest! {
    // 回数は環境変数 PROPTEST_CASES で変える（既定 256）。
    #[test]
    fn arbitrary_strings_complete_without_panic(src in any::<String>()) {
        pipeline(&src);
    }

    #[test]
    fn arbitrary_strings_after_a_valid_header_complete(body in any::<String>()) {
        pipeline(&format!("{HEAD}{body}"));
    }

    #[test]
    fn token_soup_completes_without_panic(
        tokens in proptest::collection::vec(proptest::sample::select(TOKENS), 0..200)
    ) {
        pipeline(&format!("{HEAD}{}", tokens.join(" ")));
    }
}

// ---- React 式の宣言的 UI（DECISIONS R1–R4） -----------------------------------------

use cdl_eval::{ChildNode, Element};

fn root_of(body: &str) -> std::sync::Arc<Element> {
    run(body, &Limits::default())
        .unwrap_or_else(|d| panic!("{body}\n{d:#?}"))
        .document
        .unwrap()
}

fn element_at(e: &Element, i: usize) -> &Element {
    e.children[i].as_element().unwrap()
}

/// 子の要素の `n` の列（`Row` の props）。
fn ns(e: &Element) -> Vec<f64> {
    e.children
        .iter()
        .filter_map(ChildNode::as_element)
        .map(|c| match c.props.get("n") {
            Some(Value::Number(n)) => *n,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn text_children_are_normalized_and_kept_in_order() {
    let root = root_of(
        "export default (<Root><Label>\n  a   b\n\n  c  \n</Label><Label>x<Row />y{\"z\"}</Label></Root>);\n",
    );
    let first = element_at(&root, 0);
    assert_eq!(first.children, [ChildNode::Text("a   b c".into())]);
    // 要素をはさむとテキストは別のノードになる。`{"…"}` は加工しない。
    let second = element_at(&root, 1);
    assert_eq!(second.children.len(), 4);
    assert_eq!(second.children[0].as_text(), Some("x"));
    assert_eq!(second.children[1].as_element().unwrap().name, "Row");
    assert_eq!(second.children[2].as_text(), Some("y"));
    assert_eq!(second.children[3].as_text(), Some("z"));
}

#[test]
fn text_reaches_user_components_as_a_string_array() {
    let root = root_of(
        "function T(props: { children: string[] }): Element<Label> {\n  return <Label>{props.children}</Label>;\n}\n\
         export default (<Root><T>hello</T><T>a{\"b\"}</T></Root>);\n",
    );
    assert_eq!(
        element_at(&root, 0).children,
        [ChildNode::Text("hello".into())]
    );
    assert_eq!(
        element_at(&root, 1).children,
        [ChildNode::Text("a".into()), ChildNode::Text("b".into())]
    );
}

#[test]
fn false_and_undefined_children_are_removed_everywhere() {
    let root = root_of(
        "const on = true;\nconst off = false;\n\
         function Wrap(props: { children: Element<Row>[] }): Element<Row> {\n  return <Row n={0}>{props.children}</Row>;\n}\n\
         export default (<Root>\n\
           {on && <Row n={1} />}{off && <Row n={2} />}{false}{undefined}\n\
           {[off && <Row n={3} />, on && <Row n={4} />, undefined]}\n\
           <Wrap>{off && <Row n={5} />}<Row n={6} />{on && <Row n={7} />}</Wrap>\n\
           <>{on && <Row n={8} />}{off && <Row n={9} />}</>\n\
         </Root>);\n",
    );
    // 左から: `on &&` の 1、配列の 4、`Wrap`（n=0）、フラグメントの 8。
    assert_eq!(ns(&root), [1.0, 4.0, 0.0, 8.0]);
    // `Wrap` の子は、`false` が取り除かれた 6 と 7。
    assert_eq!(ns(element_at(&root, 2)), [6.0, 7.0]);
}

#[test]
fn and_does_not_evaluate_the_right_side_when_the_left_is_false() {
    // 右辺は評価されない（評価すると 0 での除算になる）。
    let off =
        "const off = false;\nexport default (<Root>{off && <Label>{`${1 / 0}`}</Label>}</Root>);\n";
    assert!(root_of(off).children.is_empty());
    let on = off.replace("const off = false;", "const off = true;");
    assert_eq!(code(&on, &Limits::default()), Code::DivisionByZero);
}

#[test]
fn keys_are_kept_on_nodes_and_not_in_props() {
    let root = root_of(
        "const ids = [\"a\", \"b\"];\n\
         export default (<Root key=\"r\">\n\
           <Row key=\"s\" n={1} />\n\
           <Row key={`${2}`} n={2} />\n\
           <Row key={`${0.5}`} n={3} />\n\
           {ids.map((i) => <Row key={`k-${i}`} n={4} />)}\n\
           <Row n={5} />\n\
         </Root>);\n",
    );
    assert_eq!(root.key.as_deref(), Some("r"));
    let keys: Vec<Option<&str>> = root
        .children
        .iter()
        .filter_map(ChildNode::as_element)
        .map(|c| c.key.as_deref())
        .collect();
    assert_eq!(
        keys,
        [
            Some("s"),
            Some("2"),
            Some("0.5"),
            Some("k-a"),
            Some("k-b"),
            None
        ]
    );
    assert!(
        root.children
            .iter()
            .filter_map(ChildNode::as_element)
            .all(|c| !c.props.contains_key("key"))
    );
}

#[test]
fn a_components_key_becomes_the_key_of_its_root_element() {
    let root = root_of(
        "function Inner(props: { n: number }): Element<Row> {\n  return <Row key=\"inner\" n={props.n} />;\n}\n\
         function Outer(props: { n: number }): Element<Row> {\n  return <Inner key=\"mid\" n={props.n} />;\n}\n\
         export default (<Root><Inner key=\"a\" n={1} /><Inner n={2} /><Outer key={`${7}`} n={3} /><Outer n={4} /></Root>);\n",
    );
    let keys: Vec<Option<&str>> = root
        .children
        .iter()
        .filter_map(ChildNode::as_element)
        .map(|c| c.key.as_deref())
        .collect();
    // 付けなければ key なし（コンポーネントの中のルート要素の key は引き継がない）。
    assert_eq!(keys, [Some("a"), None, Some("7"), None]);
}

#[test]
fn duplicate_keys_among_siblings_are_an_error() {
    // 文字列リテラルの key の重複は、評価の前（意味解析）で見つかる。ここでは、動的な key だけを扱う。
    let each = |keys: &str| format!("{{[{keys}].map((k) => <Row key={{k}} />)}}");
    let dup = |children: &str| {
        code(
            &format!("export default (<Root>{children}</Root>);\n"),
            &Limits::default(),
        )
    };
    assert_eq!(dup(&each("\"a\", \"b\", \"a\"")), Code::DuplicateChildKey);
    // フラグメントの展開後に比べる。
    assert_eq!(
        dup(&format!("{}<>{}</>", each("\"a\""), each("\"a\""))),
        Code::DuplicateChildKey
    );
    // 親が違えば、同じ key でもよい。key のない子は何個あってもよい。
    let ok = "<Row key=\"a\"><Row key=\"a\" /></Row><Row key=\"b\" /><Row /><Row />";
    assert!(
        run(
            &format!("export default (<Root>{ok}</Root>);\n"),
            &Limits::default()
        )
        .is_ok()
    );
    // 見つかった重複は、すべて報告される（DECISIONS U3）。
    let all = run_all(
        &format!(
            "export default (<Root>{}</Root>);\n",
            each("\"a\", \"b\", \"a\", \"b\", \"a\"")
        ),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|d| d.code == Code::DuplicateChildKey));
    // 誤りの位置は、重複した子を作った要素。
    let body = format!("export default (<Root>{}</Root>);\n", each("\"a\", \"a\""));
    let d = run(&body, &Limits::default()).unwrap_err();
    let src = format!("{HEAD}{body}");
    let start = usize::try_from(d.span.start).unwrap();
    let end = usize::try_from(d.span.end).unwrap();
    assert_eq!(&src[start..end], "<Row key={k} />");
}

#[test]
fn defaults_are_evaluated_per_call_only_when_needed_and_cost_steps() {
    let defs = "function f({ a, b = 1 / 0 }: { a: number; b?: number }): number { return a; }\n";
    // 既定値が使われなければ、評価されない（評価すると 0 での除算）。
    let root = |call: &str| format!("{defs}const v = {call};\n{ROOT}");
    assert!(run(&root("f({ a: 1, b: 2 })"), &Limits::default()).is_ok());
    assert_eq!(
        code(&root("f({ a: 1 })"), &Limits::default()),
        Code::DivisionByZero
    );
    assert_eq!(
        code(&root("f({ a: 1, b: undefined })"), &Limits::default()),
        Code::DivisionByZero
    );
    // 既定値の評価は、呼び出しごとにステップを使う。
    let heavy = "function g({ xs = [1, 2, 3, 4, 5, 6, 7, 8] }: { xs?: number[] }): number { return xs.length; }\n\
                 const ns = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];\n";
    let with_default = format!("{heavy}const a = ns.map((n) => g({{}}));\n{ROOT}");
    let with_value = format!("{heavy}const a = ns.map((n) => g({{ xs: [] }}));\n{ROOT}");
    let small = |steps: u64| Limits {
        max_steps: steps,
        ..Limits::default()
    };
    // 同じ上限で、既定値を使う側だけが超える。
    let used = (0..400)
        .find(|n| run(&with_default, &small(*n)).is_ok())
        .unwrap();
    let skipped = (0..400)
        .find(|n| run(&with_value, &small(*n)).is_ok())
        .unwrap();
    assert!(used > skipped + 10 * 8, "{used} {skipped}");
}

#[test]
fn defaults_are_evaluated_outside_the_functions_own_arguments() {
    // 既定値は、アロー関数が閉じ込めた外側の値を見られる。
    let root = root_of(
        "const base = 10;\nconst f = (k: number): number => ((({ a, b = base + k }: { a: number; b?: number }): number => a + b)({ a: 1 }));\n\
         export default (<Root><Row n={f(5)} /></Root>);\n",
    );
    assert_eq!(ns(&root), [16.0]);
}

#[test]
fn evaluation_with_the_new_features_is_deterministic() {
    let body = "const ids = [\"a\", \"b\"];\n\
                function R({ n, w = 2 }: { n: number; w?: number }): Element<Row> {\n  return <Row n={n * w} />;\n}\n\
                export default (<Root>{ids.map((i) => <R key={i} n={1} />)}{true && <Label> hi  there </Label>}</Root>);\n";
    assert_eq!(
        run(body, &Limits::default()).unwrap().document,
        run(body, &Limits::default()).unwrap().document
    );
}

proptest! {
    /// `c && <A />` の子は、条件が真のものだけが、元の順序で残る（DECISIONS R2）。
    #[test]
    fn and_children_keep_exactly_the_true_conditions(
        flags in proptest::collection::vec(any::<bool>(), 0..8),
    ) {
        let kids: String = flags
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{{{f} && <Row n={{{i}}} />}}"))
            .collect();
        let root = root_of(&format!("export default (<Root>{kids}</Root>);\n"));
        let want: Vec<f64> = flags
            .iter()
            .enumerate()
            .filter(|(_, f)| **f)
            .map(|(i, _)| f64::from(u32::try_from(i).unwrap()))
            .collect();
        prop_assert_eq!(ns(&root), want);
    }
}

/// テキストの子も値木のノードとして数える（要素 2 つ + テキスト 3 つ = 5。ちょうどなら成功、1 つ少なければ超過）。
#[test]
fn text_children_count_as_value_tree_nodes() {
    let body = "export default (<Root><Label>{\"a\"}{\"b\"}{\"c\"}</Label></Root>);\n";
    let with = |n: u64| Limits {
        max_nodes: n,
        ..Limits::default()
    };
    assert!(run(body, &with(5)).is_ok());
    assert_eq!(code(body, &with(4)), Code::NodeLimit);
}

/// 空文字列 `{""}` の子は残る。number の子は置けない（変更しない。DECISIONS R1・U5）。
#[test]
fn empty_string_children_stay_and_number_children_are_rejected() {
    let root = root_of("export default (<Root><Label>{\"\"}</Label></Root>);\n");
    assert_eq!(element_at(&root, 0).children, [ChildNode::Text("".into())]);
}
