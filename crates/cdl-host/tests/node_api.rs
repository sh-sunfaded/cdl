//! `Node` / `FromNode` / `FromValue`（ホストが評価結果を読む面。DECISIONS U4）の結合テスト。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_host::diag::Code;
use cdl_host::{
    Child, ComponentDef, Diagnostic, Engine, FromNode, FromValue, Millis, ModuleResolver, Node, Px,
    Rgba, Schema, Source, Ty, Value,
};

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
    let m = "host:t";
    Schema::builder()
        .root(m, "Root")
        .component(
            m,
            ComponentDef::new("Root")
                .children(&["Look", "Row", "Note"])
                .unique_child("Look"),
        )
        .component(
            m,
            ComponentDef::new("Look")
                .prop("gap", Ty::Length)
                .optional_prop("tint", Ty::Color)
                .optional_prop("tags", Ty::Array(Box::new(Ty::String)))
                .optional_prop("badge", Ty::element("Note")),
        )
        .component(
            m,
            ComponentDef::new("Row")
                .prop("name", Ty::String)
                .optional_prop("weight", Ty::Number)
                .optional_prop("on", Ty::Boolean)
                .optional_prop("delay", Ty::Duration),
        )
        .component(m, ComponentDef::new("Note").text_children())
        .build()
        .unwrap()
}

/// 手書きの実装例（derive マクロはない）。
#[derive(Debug, PartialEq)]
struct Look {
    gap: Px,
    tint: Option<Rgba>,
    tags: Vec<String>,
    badge: Option<Note>,
}

#[derive(Debug, PartialEq)]
struct Note(String);

#[derive(Debug, PartialEq)]
struct Row {
    name: String,
    weight: Option<f64>,
    on: bool,
    delay: Option<Millis>,
}

impl FromNode for Look {
    const COMPONENT: &'static str = "Look";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Look {
            gap: node.prop("gap")?,
            tint: node.prop("tint")?,
            tags: node
                .prop::<Option<Vec<String>>>("tags")?
                .unwrap_or_default(),
            badge: node.slot_as("badge")?,
        })
    }
}

impl FromNode for Note {
    const COMPONENT: &'static str = "Note";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        let text: Vec<&str> = node
            .child_nodes()
            .filter_map(|c| match c {
                Child::Text(t) => Some(t),
                Child::Element(_) => None,
            })
            .collect();
        Ok(Note(text.join("")))
    }
}

impl FromNode for Row {
    const COMPONENT: &'static str = "Row";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Row {
            name: node.prop("name")?,
            weight: node.prop("weight")?,
            on: node.prop::<Option<bool>>("on")?.unwrap_or(true),
            delay: node.prop("delay")?,
        })
    }
}

fn tree(body: &str) -> std::sync::Arc<cdl_host::Element> {
    let src =
        format!("\"use cdl 1\";\nimport {{ Root, Look, Row, Note }} from \"host:t\";\n{body}\n");
    let program = Engine::new()
        .with_schema(schema())
        .load(&One(src), "main.cdl")
        .unwrap_or_else(|d| panic!("{d:#?}"));
    program.render(&Value::Undefined).unwrap()
}

const FULL: &str = "export default (<Root key=\"r\">\n\
    <Look gap={px(4)} tint={color(\"#ff0000\")} tags={[\"a\", \"b\"]} badge={<Note>new</Note>} />\n\
    <Row key=\"x\" name=\"one\" weight={2} on={false} delay={ms(50)} />\n\
    <Row name=\"two\" />\n\
    </Root>);";

#[test]
fn node_accessors() {
    let t = tree(FULL);
    let root = Node::new(&t);
    assert_eq!(root.component(), "Root");
    assert_eq!(root.key(), Some("r"));
    assert!(root.props().is_empty());
    assert_eq!(root.span().file, "main.cdl");
    assert!(root.expanded_from().is_empty());
    assert_eq!(root.children().count(), 3);
    assert_eq!(root.children_named("Row").count(), 2);
    assert_eq!(root.child_nodes().count(), 3);
    let row = root.children_named("Row").next().unwrap();
    assert_eq!(row.key(), Some("x"));
    assert_eq!(row.props().len(), 4);
}

#[test]
fn typed_reading() {
    let t = tree(FULL);
    let root = Node::new(&t);
    let look: Look = root.required_child().unwrap();
    assert_eq!(
        look,
        Look {
            gap: Px(4.0),
            tint: Some(Rgba([255, 0, 0, 255])),
            tags: vec!["a".to_owned(), "b".to_owned()],
            badge: Some(Note("new".to_owned())),
        }
    );
    assert_eq!(root.unique_child::<Look>().unwrap(), Some(look));
    let rows: Vec<Row> = root.children_of().unwrap();
    assert_eq!(
        rows,
        [
            Row {
                name: "one".to_owned(),
                weight: Some(2.0),
                on: false,
                delay: Some(Millis(50)),
            },
            Row {
                name: "two".to_owned(),
                weight: None,
                on: true,
                delay: None,
            },
        ]
    );
    // `read` はコンポーネント名を確かめる。
    let first_row = root.children_named("Row").next().unwrap();
    assert!(first_row.read::<Row>().is_ok());
    let err = first_row.read::<Look>().unwrap_err();
    assert_eq!(err.code, Code::ConversionFailed);
    assert_eq!(err.span, *first_row.span());
}

#[test]
fn missing_and_duplicate_children() {
    let t = tree("export default (<Root><Row name=\"a\" /></Root>);");
    let root = Node::new(&t);
    assert_eq!(root.unique_child::<Look>().unwrap(), None);
    let err = root.required_child::<Look>().unwrap_err();
    assert_eq!(err.code, Code::ConversionFailed);
    assert_eq!(err.span, *root.span());
}

#[test]
fn failures_are_diagnostics_at_the_prop_or_the_element() {
    let t = tree("export default (<Root><Row name=\"a\" /></Root>);");
    let row = Node::new(&t).children().next().unwrap();
    // 型が違う: prop の位置（DECISIONS P1）。ない prop を必須で読む: 要素の位置。
    for (err, key) in [
        (row.prop::<f64>("name").unwrap_err(), "name"),
        (row.prop::<Vec<String>>("name").unwrap_err(), "name"),
    ] {
        assert_eq!(err.code, Code::ConversionFailed);
        assert_eq!(Some(err.span), row.prop_span(key));
    }
    let missing = row.prop::<String>("weight").unwrap_err();
    assert_eq!(missing.code, Code::ConversionFailed);
    assert_eq!(missing.span, *row.span());
    assert!(
        row.prop::<f64>("name")
            .unwrap_err()
            .message
            .contains("number")
    );
    assert!(
        row.prop::<Vec<Option<String>>>("name")
            .unwrap_err()
            .message
            .contains("(string | undefined)[]")
            || row
                .prop::<Vec<Option<String>>>("name")
                .unwrap_err()
                .message
                .contains("string | undefined[]")
    );
}

#[test]
fn from_value_for_standard_types() {
    let s = Value::string("x");
    let n = Value::Number(1.5);
    assert_eq!(String::from_value(&s), Some("x".to_owned()));
    assert_eq!(f64::from_value(&n), Some(1.5));
    assert_eq!(bool::from_value(&Value::Bool(true)), Some(true));
    assert_eq!(Px::from_value(&Value::Length(2.0)), Some(Px(2.0)));
    assert_eq!(Millis::from_value(&Value::Duration(7)), Some(Millis(7)));
    assert_eq!(
        Rgba::from_value(&Value::Color([1, 2, 3, 4])),
        Some(Rgba([1, 2, 3, 4]))
    );
    assert_eq!(Option::<f64>::from_value(&Value::Undefined), Some(None));
    assert_eq!(Option::<f64>::from_value(&n), Some(Some(1.5)));
    assert_eq!(Option::<f64>::from_value(&s), None);
    let xs = Value::array(vec![Value::Number(1.0), Value::Number(2.0)]);
    assert_eq!(Vec::<f64>::from_value(&xs), Some(vec![1.0, 2.0]));
    assert_eq!(Vec::<String>::from_value(&xs), None);
    assert_eq!(Vec::<f64>::from_value(&n), None);
    assert_eq!(
        Vec::<Option<f64>>::from_value(&Value::array(vec![Value::Undefined])),
        Some(vec![None])
    );
    assert_eq!(<Vec<f64>>::expected(), "number[]");
    assert_eq!(<Option<Px>>::expected(), "Length | undefined");
}
