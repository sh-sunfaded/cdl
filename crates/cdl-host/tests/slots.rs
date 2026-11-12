//! スロット（要素を props に渡す。DECISIONS U2）の結合テスト。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_host::diag::Code;
use cdl_host::{ComponentDef, Engine, ModuleResolver, Schema, Source, Ty};

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
        .component(m, ComponentDef::new("Root").children(&["Panel"]))
        .component(
            m,
            ComponentDef::new("Panel")
                .optional_prop("header", Ty::element("Title"))
                .optional_prop("tail", Ty::Array(Box::new(Ty::element("Title"))))
                .children(&["Title"])
                .text_children(),
        )
        .component(
            m,
            ComponentDef::new("Title").optional_prop("text", Ty::String),
        )
        .build()
        .unwrap()
}

fn load(body: &str) -> Result<cdl_host::Program, Vec<cdl_host::Diagnostic>> {
    let src = format!("\"use cdl 1\";\nimport {{ Root, Panel, Title }} from \"host:t\";\n{body}\n");
    Engine::new()
        .with_schema(schema())
        .load(&One(src), "main.cdl")
}

#[test]
fn slot_elements_are_props_not_children_and_readable_through_node() {
    let program = load(
        "export default (<Root><Panel header={<Title text=\"h\" />} tail={[<Title text=\"a\" />, <Title text=\"b\" />]}>mid<Title text=\"c\" /></Panel></Root>);",
    )
    .unwrap();
    let panel = program.root().unwrap().children().next().unwrap();
    // 子には、スロットの要素は入らない。
    assert_eq!(panel.children().count(), 1);
    assert_eq!(panel.child_nodes().count(), 2);
    let header = panel.slot("header").unwrap().unwrap();
    assert_eq!(header.component(), "Title");
    assert_eq!(header.prop::<String>("text").unwrap(), "h");
    let tail: Vec<String> = panel
        .slots("tail")
        .unwrap()
        .iter()
        .map(|n| n.prop::<String>("text").unwrap())
        .collect();
    assert_eq!(tail, ["a", "b"]);
    // 値がなければ None / 空。
    let program = load("export default (<Root><Panel /></Root>);").unwrap();
    let panel = program.root().unwrap().children().next().unwrap();
    assert!(panel.slot("header").unwrap().is_none());
    assert!(panel.slots("tail").unwrap().is_empty());
    // 要素でない props を slot として読めば E4008（位置つき）。
    let program =
        load("export default (<Root><Panel header={<Title text=\"h\" />} /></Root>);").unwrap();
    let panel = program.root().unwrap().children().next().unwrap();
    let title = panel.slot("header").unwrap().unwrap();
    assert_eq!(title.slot("text").unwrap_err().code, Code::ConversionFailed);
    assert_eq!(
        title.slots("text").unwrap_err().code,
        Code::ConversionFailed
    );
}

#[test]
fn slot_type_mismatches_are_diagnostics() {
    let errs = load("export default (<Root><Panel header={<Panel />} /></Root>);").unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, Code::SlotType);
    let errs =
        load("export default (<Root><Panel tail={[<Title />, <Panel />]} /></Root>);").unwrap_err();
    assert_eq!(errs[0].code, Code::SlotType);
}
