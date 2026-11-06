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
                .children(&["Item"])
                .unique_child("Item"),
        )
        .component(m, ComponentDef::new("Item").prop("name", Ty::String))
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

const HEAD: &str = "\"use cdl 1\";\nimport { Root, Item } from \"host:t\";\nimport type { Policy } from \"host:t\";\n";

fn load(body: &str) -> (Module, Checked) {
    let src = format!("{HEAD}{body}");
    let module = syntax::parse("main.cdl", &src, &syntax::Limits::default())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let checked = sema::check(&module, &schema()).unwrap_or_else(|d| panic!("{src}\n{d:#?}"));
    (module, checked)
}

fn run(body: &str, limits: &Limits) -> Result<Evaluated, Diagnostic> {
    let (module, checked) = load(body);
    evaluate(&[unit(&module, &checked)], 0, &schema(), limits)
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
    let item = &ev.document.children[0];
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
    assert_eq!(code(&chain(5_000), &Limits::default()), Code::ValueDepth);
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
        ev.document.children[0].props["name"],
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
