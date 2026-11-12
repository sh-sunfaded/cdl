//! 公開 API の結合テスト（SPEC 第 9・11 章）。ファイルシステムには触れない。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use cdl_host::diag::Code;
use cdl_host::{
    ChildNode, ComponentDef, Engine, Field, Limits, Millis, ModuleResolver, Node, Px, Rgba, Schema,
    Source, Ty, Value,
};

fn schema() -> Schema {
    let m = "host:t";
    Schema::builder()
        .root(m, "Root")
        .component(m, ComponentDef::new("Root").children(&["Item"]))
        .component(
            m,
            ComponentDef::new("Item")
                .prop("name", Ty::String)
                .optional_prop("size", Ty::Length)
                .optional_prop("delay", Ty::Duration)
                .optional_prop("tint", Ty::Color),
        )
        .type_def(
            m,
            "Policy",
            Ty::object([(
                "pick",
                Field::optional(Ty::Function(vec![Ty::Number], Box::new(Ty::String))),
            )]),
        )
        .build()
        .unwrap()
}

/// メモリ上のファイル。`resolve` の呼び出しを数える。
struct Memory {
    files: BTreeMap<&'static str, &'static str>,
    calls: RefCell<Vec<String>>,
}

impl ModuleResolver for Memory {
    fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        self.calls.borrow_mut().push(specifier.to_owned());
        // このテストのファイルはすべて同じディレクトリにある。
        let _ = importer;
        let id = specifier.trim_start_matches("./");
        self.files
            .get(id)
            .map(|text| Source {
                id: id.to_owned(),
                text: (*text).to_owned(),
            })
            .ok_or_else(|| "ない".to_owned())
    }
}

fn memory(files: &[(&'static str, &'static str)]) -> Memory {
    Memory {
        files: files.iter().copied().collect(),
        calls: RefCell::new(Vec::new()),
    }
}

const MAIN: &str = r##""use cdl 1";
import { Root, Item } from "host:t";
import type { Policy } from "host:t";
import { size } from "./a.cdl";
import { label } from "./b.cdl";

export const policy = {
  pick: (n: number): string => (n > 0 ? label : "none"),
} satisfies Policy;

export default (
  <Root>
    <Item name={label} size={size} delay={ms(250)} tint={color("#ff0000")} />
    <Item name="plain" />
  </Root>
);
"##;
const A: &str =
    "\"use cdl 1\";\nimport { base } from \"./c.cdl\";\nexport const size = base * 2;\n";
const B: &str =
    "\"use cdl 1\";\nimport { count } from \"./c.cdl\";\nexport const label = `b${count}`;\n";
const C: &str = "\"use cdl 1\";\nexport const base = px(4);\nexport const count = 4;\n";

#[test]
fn imports_are_resolved_once_and_evaluated() {
    let files = memory(&[("main.cdl", MAIN), ("a.cdl", A), ("b.cdl", B), ("c.cdl", C)]);
    let program = Engine::new()
        .with_schema(schema())
        .load(&files, "main.cdl")
        .unwrap();
    // c.cdl は a と b から import される。リゾルバは import ごとに呼ばれるが、
    // 正準 id が同じなので、構文解析と評価は 1 回だけ（SPEC 第 9 章）。
    let calls = files.calls.borrow();
    assert_eq!(
        calls.iter().filter(|s| s.as_str() == "./c.cdl").count(),
        2,
        "{calls:?}"
    );
    assert_eq!(program.files().filter(|f| *f == "c.cdl").count(), 1);
    // 依存先が先に来る順。
    let order: Vec<&str> = program.files().collect();
    assert_eq!(order.last(), Some(&"main.cdl"));
    assert!(order.iter().position(|f| *f == "c.cdl") < order.iter().position(|f| *f == "a.cdl"));

    let root = program.root().unwrap();
    root.expect_component("Root").unwrap();
    let items: Vec<Node<'_>> = root.children().collect();
    assert_eq!(items[0].prop::<String>("name").unwrap(), "b4");
    assert_eq!(items[0].prop::<Px>("size").unwrap(), Px(8.0));
    assert_eq!(items[0].prop::<Millis>("delay").unwrap(), Millis(250));
    assert_eq!(
        items[0].prop::<Rgba>("tint").unwrap(),
        Rgba([255, 0, 0, 255])
    );
    assert_eq!(items[1].prop::<Option<Px>>("size").unwrap(), None);

    assert_eq!(
        program.call_hook("pick", Value::Number(1.0)),
        Ok(Some(Value::string("b4")))
    );
    assert_eq!(
        program.call_hook("pick", Value::Number(0.0)),
        Ok(Some(Value::string("none")))
    );
    assert_eq!(program.call_hook("missing", Value::Number(0.0)), Ok(None));
}

#[test]
fn conversion_failures_are_diagnostics_at_the_element() {
    let files = memory(&[("main.cdl", MAIN), ("a.cdl", A), ("b.cdl", B), ("c.cdl", C)]);
    let program = Engine::new()
        .with_schema(schema())
        .load(&files, "main.cdl")
        .unwrap();
    let item = program.root().unwrap().children().next().unwrap();
    let wrong = item.prop::<f64>("size").unwrap_err();
    assert_eq!(wrong.code, Code::ConversionFailed);
    assert_eq!(wrong.span, *item.span());
    assert_eq!(wrong.span.file, "main.cdl");
    let missing = program
        .root()
        .unwrap()
        .children()
        .nth(1)
        .unwrap()
        .prop::<Px>("size")
        .unwrap_err();
    assert_eq!(missing.code, Code::ConversionFailed);
    assert_eq!(
        program
            .root()
            .unwrap()
            .expect_component("Item")
            .unwrap_err()
            .code,
        Code::ConversionFailed
    );
}

#[test]
fn errors_in_imported_files_point_at_those_files() {
    let bad_c = "\"use cdl 1\";\nexport const base: number = px(4);\nexport const count = 4;\n";
    let files = memory(&[
        ("main.cdl", MAIN),
        ("a.cdl", A),
        ("b.cdl", B),
        ("c.cdl", bad_c),
    ]);
    let err = Engine::new()
        .with_schema(schema())
        .load(&files, "main.cdl")
        .unwrap_err();
    assert_eq!(err.len(), 1, "{err:#?}");
    assert_eq!(err[0].code, Code::TypeMismatch);
    assert_eq!(err[0].span.file, "c.cdl");
}

#[test]
fn evaluation_errors_in_imported_functions_point_at_their_file() {
    let main = "\"use cdl 1\";\nimport { Root, Item } from \"host:t\";\nimport { f } from \"./lib.cdl\";\nconst x = f(0);\nexport default (<Root />);\n";
    let lib = "\"use cdl 1\";\nexport function f(n: number): number {\n  return 1 / n;\n}\n";
    let files = memory(&[("main.cdl", main), ("lib.cdl", lib)]);
    let err = Engine::new()
        .with_schema(schema())
        .load(&files, "main.cdl")
        .unwrap_err();
    assert_eq!(err[0].code, Code::DivisionByZero);
    assert_eq!(err[0].span.file, "lib.cdl");
    assert_eq!(
        &lib[usize::try_from(err[0].span.start).unwrap()
            ..usize::try_from(err[0].span.end).unwrap()],
        "1 / n"
    );
}

#[test]
fn limits_apply_to_every_stage() {
    let files = memory(&[("main.cdl", MAIN), ("a.cdl", A), ("b.cdl", B), ("c.cdl", C)]);
    let tiny = |f: fn(&mut Limits)| {
        let mut l = Limits::default();
        f(&mut l);
        Engine::new().with_schema(schema()).with_limits(l)
    };
    assert_eq!(
        tiny(|l| l.max_source_bytes = 10)
            .load(&files, "main.cdl")
            .unwrap_err()[0]
            .code,
        Code::SourceTooLarge
    );
    assert_eq!(
        tiny(|l| l.max_steps = 5)
            .load(&files, "main.cdl")
            .unwrap_err()[0]
            .code,
        Code::StepLimit
    );
    let program = tiny(|l| l.max_hook_steps = 1)
        .load(&files, "main.cdl")
        .unwrap();
    assert_eq!(
        program
            .call_hook("pick", Value::Number(1.0))
            .unwrap_err()
            .code,
        Code::StepLimit
    );
}

#[test]
fn loading_is_deterministic() {
    let files = memory(&[("main.cdl", MAIN), ("a.cdl", A), ("b.cdl", B), ("c.cdl", C)]);
    let engine = Engine::new().with_schema(schema());
    let a = engine.load(&files, "main.cdl").unwrap();
    let b = engine.load(&files, "main.cdl").unwrap();
    assert_eq!(a.document().unwrap(), b.document().unwrap());
}

/// `Program` はスレッドをまたいで渡せ、共有できる（値は `Arc` で持つ）。
#[test]
fn program_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<cdl_host::Program>();
    assert_send_sync::<cdl_host::Value>();
    assert_send_sync::<cdl_host::Element>();
}

/// `key` とテキスト子を、公開 API から読める（DECISIONS R1・R3）。
#[test]
fn keys_and_text_children_are_readable_through_the_public_api() {
    let m = "host:t";
    let schema = Schema::builder()
        .root(m, "Root")
        .component(m, ComponentDef::new("Root").children(&["Item", "Label"]))
        .component(m, ComponentDef::new("Item").prop("name", Ty::String))
        .component(m, ComponentDef::new("Label").text_children())
        .build()
        .unwrap();
    const SRC: &str = "\"use cdl 1\";\nimport { Root, Item, Label } from \"host:t\";\n\
        function Row(props: { name: string }): Element<Item> {\n  return <Item name={props.name} />;\n}\n\
        export default (\n  <Root key=\"root\">\n    <Item key=\"a\" name=\"one\" />\n    <Row key={`${2}`} name=\"two\" />\n    <Item name=\"three\" />\n    <Label>\n      hello\n      world\n    </Label>\n  </Root>\n);\n";
    let files = memory(&[("main.cdl", SRC)]);
    let program = Engine::new()
        .with_schema(schema)
        .load(&files, "main.cdl")
        .unwrap();
    let root = program.root().unwrap();
    assert_eq!(root.key(), Some("root"));
    let elements: Vec<Node<'_>> = root.children().collect();
    assert_eq!(elements.len(), 4);
    let keys: Vec<Option<&str>> = elements.iter().map(Node::key).collect();
    assert_eq!(keys, [Some("a"), Some("2"), None, None]);
    // key は props ではない。
    assert!(elements.iter().all(|n| !n.props().contains_key("key")));
    // 識別 prop（ここでは name）は props として残る。
    assert_eq!(elements[1].prop::<String>("name").unwrap(), "two");
    // テキスト子は `child_nodes` と評価結果の `ChildNode` から読める。`children` は要素だけ。
    let label = elements[3];
    assert_eq!(label.children().count(), 0);
    let nodes: Vec<cdl_host::Child<'_>> = label.child_nodes().collect();
    assert!(matches!(
        nodes.as_slice(),
        [cdl_host::Child::Text("hello world")]
    ));
    assert_eq!(
        program.document().unwrap().children[3]
            .as_element()
            .unwrap()
            .children,
        [ChildNode::Text("hello world".into())]
    );
    // 要素の子は `Child::Element` で読める。
    let root_nodes: Vec<cdl_host::Child<'_>> = root.child_nodes().collect();
    assert!(
        root_nodes
            .iter()
            .all(|c| matches!(c, cdl_host::Child::Element(_)))
    );
}
