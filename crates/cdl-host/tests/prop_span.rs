//! props ごとの位置（`Node::prop_span` / `Node::prop_error`。DECISIONS P1）の結合テスト。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_host::diag::{Code, Span};
use cdl_host::{
    ComponentDef, Engine, Field, ModuleResolver, Node, Program, Schema, Source, Ty, Value,
};

const M: &str = "host:t";

struct One(String);

impl ModuleResolver for One {
    fn resolve(&self, _importer: Option<&str>, _specifier: &str) -> Result<Source, String> {
        Ok(Source {
            id: "main.cdl".to_owned(),
            text: self.0.clone(),
        })
    }
}

fn schema() -> Schema {
    Schema::builder()
        .root(M, "Root")
        .env(M, Ty::object([("n", Field::required(Ty::Number))]))
        .component(M, ComponentDef::new("Root").children(&["Row"]))
        .component(
            M,
            ComponentDef::new("Row")
                .optional_prop("n", Ty::Number)
                .optional_prop("tag", Ty::String),
        )
        .build()
        .unwrap()
}

fn load(body: &str) -> (Program, String) {
    let src = format!("\"use cdl 1\";\nimport {{ Root, Row }} from \"host:t\";\n{body}\n");
    let program = Engine::new()
        .with_schema(schema())
        .load(&One(src.clone()), "main.cdl")
        .unwrap();
    (program, src)
}

fn text<'a>(src: &'a str, span: &Span) -> &'a str {
    assert_eq!(span.file, "main.cdl");
    &src[usize::try_from(span.start).unwrap()..usize::try_from(span.end).unwrap()]
}

/// `Row` の `key` の prop を `bool` として読み、失敗の E4008 の位置と `prop_span` が一致することも確かめて、
/// 位置のソース上の文字列を返す。
fn failing_prop_text(program: &Program, src: &str, key: &str) -> String {
    let root = program.root().unwrap();
    let row = root.children_named("Row").next().unwrap();
    let err = row.prop::<bool>(key).unwrap_err();
    assert_eq!(err.code, Code::ConversionFailed);
    assert_eq!(Some(err.span.clone()), row.prop_span(key));
    text(src, &err.span).to_owned()
}

#[test]
fn a_direct_prop_points_at_its_attribute() {
    let (p, src) = load("export default (<Root><Row n={5} tag=\"x\" /></Root>);");
    assert_eq!(failing_prop_text(&p, &src, "n"), "n={5}");
    assert_eq!(failing_prop_text(&p, &src, "tag"), "tag=\"x\"");
}

#[test]
fn a_spread_prop_points_at_the_spread_attribute() {
    let (p, src) =
        load("const v = { n: 5, tag: \"x\" };\nexport default (<Root><Row {...v} /></Root>);");
    assert_eq!(failing_prop_text(&p, &src, "n"), "{...v}");
    assert_eq!(failing_prop_text(&p, &src, "tag"), "{...v}");
}

#[test]
fn the_later_attribute_wins_and_so_does_its_span() {
    let (p, src) = load(
        "const v = { n: 5, tag: \"x\" };\nexport default (<Root><Row {...v} n={7} /></Root>);",
    );
    assert_eq!(failing_prop_text(&p, &src, "n"), "n={7}");
    assert_eq!(failing_prop_text(&p, &src, "tag"), "{...v}");
}

#[test]
fn a_prop_of_an_expanded_component_points_into_the_component_body() {
    let (p, src) = load(
        "function R(props: { n: number }): Element<Row> {\n  return <Row n={props.n} tag=\"x\" />;\n}\n\
         export default (<Root><R n={1} /></Root>);",
    );
    assert_eq!(failing_prop_text(&p, &src, "n"), "n={props.n}");
    let root = p.root().unwrap();
    let row = root.children_named("Row").next().unwrap();
    assert_eq!(row.expanded_from().len(), 1);
}

#[test]
fn missing_and_undefined_props_have_no_span_and_fall_back_to_the_element() {
    let (p, src) = load("export default (<Root><Row n={undefined} /></Root>);");
    let root = p.root().unwrap();
    let row = root.children_named("Row").next().unwrap();
    assert_eq!(row.prop_span("n"), None);
    assert_eq!(row.prop_span("tag"), None);
    // 必須でない読み出しは、ないものを `None` として読める。
    assert_eq!(row.prop::<Option<bool>>("n").unwrap(), None);
    // `prop_error` は、位置がなければ要素の位置で E4008。
    let err = row.prop_error("tag", "なし");
    assert_eq!(err.code, Code::ConversionFailed);
    assert_eq!(&err.span, row.span());
    assert_eq!(text(&src, &err.span), "<Row n={undefined} />");
    // `prop::<bool>` が、ない prop で失敗するときも、要素の位置。
    let err = row.prop::<bool>("tag").unwrap_err();
    assert_eq!(&err.span, row.span());
}

#[test]
fn prop_error_uses_the_given_message_at_the_prop() {
    let (p, src) = load("export default (<Root><Row n={5} /></Root>);");
    let root = p.root().unwrap();
    let row = root.children_named("Row").next().unwrap();
    let err = row.prop_error("n", "範囲外");
    assert_eq!(err.message, "範囲外");
    assert_eq!(text(&src, &err.span), "n={5}");
}

#[test]
fn a_function_root_keeps_the_prop_spans_per_render() {
    let (p, src) = load(
        "import type { Env } from \"host:t\";\n\
         export default function App(env: Env): Element<Root> {\n  return <Root><Row n={env.n} /></Root>;\n}",
    );
    let tree = p
        .render(&Value::object([("n", Value::Number(3.0))]))
        .unwrap();
    let row = Node::new(&tree).children_named("Row").next().unwrap();
    let span = row.prop_span("n").unwrap();
    assert_eq!(text(&src, &span), "n={env.n}");
}
