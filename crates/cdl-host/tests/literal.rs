//! host が `Literal` でリテラルのユニオンを宣言できること（DECISIONS L1）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use cdl_host::diag::Code;
use cdl_host::{
    ComponentDef, Engine, Field, Literal, ModuleResolver, Node, Program, Schema, Source, Ty, Value,
};

const M: &str = "host:t";

fn lit(s: &str) -> Ty {
    Ty::Literal(Literal::String(s.to_owned()))
}

fn mod_key() -> Ty {
    Ty::union(vec![lit("alt"), lit("super")])
}

fn schema() -> Schema {
    Schema::builder()
        .root(M, "Root")
        .env(
            M,
            Ty::object([(
                "kind",
                Field::required(Ty::union(vec![lit("main"), lit("aux")])),
            )]),
        )
        .component(
            M,
            ComponentDef::new("Root").optional_prop("modKey", mod_key()),
        )
        .build()
        .unwrap()
}

struct Memory(BTreeMap<&'static str, String>);

impl ModuleResolver for Memory {
    fn resolve(&self, _importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        let id = specifier.trim_start_matches("./");
        self.0
            .get(id)
            .map(|t| Source {
                id: id.to_owned(),
                text: t.clone(),
            })
            .ok_or_else(|| "なし".to_owned())
    }
}

fn load(body: &str) -> Result<Program, Vec<cdl_host::Diagnostic>> {
    let src = format!("\"use cdl 1\";\nimport {{ Root }} from \"host:t\";\n{body}\n");
    let files = Memory(BTreeMap::from([("main.cdl", src)]));
    Engine::new().with_schema(schema()).load(&files, "main.cdl")
}

#[test]
fn a_literal_union_prop_accepts_its_members() {
    for v in ["alt", "super"] {
        let program = load(&format!("export default (<Root modKey=\"{v}\" />);")).unwrap();
        let got = Node::new(program.document().unwrap())
            .prop::<Option<String>>("modKey")
            .unwrap();
        assert_eq!(got.as_deref(), Some(v));
    }
    assert!(load("export default (<Root />);").is_ok());
}

#[test]
fn a_value_outside_the_union_is_e4004_that_names_the_union() {
    let err = load("export default (<Root modKey=\"meta\" />);").unwrap_err();
    assert_eq!(err[0].code, Code::HostPropType);
    assert!(
        err[0].message.contains("\"alt\"") && err[0].message.contains("\"super\""),
        "{}",
        err[0].message
    );
    // string 型の値は、リテラルのユニオンに入らない。
    let err =
        load("const m: string = \"alt\";\nexport default (<Root modKey={m} />);").unwrap_err();
    assert_eq!(err[0].code, Code::HostPropType);
}

#[test]
fn a_literal_union_in_env_is_checked_on_render() {
    let body = "import type { Env } from \"host:t\";\n\
        export default function App(env: Env): Element<Root> {\n\
        \x20 return env.kind === \"main\" ? <Root modKey=\"alt\" /> : <Root />;\n\
        }";
    let program = load(body).unwrap();
    let env = |k: &str| Value::object([("kind", Value::string(k))]);
    let main = program.render(&env("main")).unwrap();
    assert_eq!(
        Node::new(&main)
            .prop::<Option<String>>("modKey")
            .unwrap()
            .as_deref(),
        Some("alt")
    );
    let aux = program.render(&env("aux")).unwrap();
    assert_eq!(
        Node::new(&aux).prop::<Option<String>>("modKey").unwrap(),
        None
    );
    let err = program.render(&env("other")).unwrap_err();
    assert_eq!(err[0].code, Code::EnvMismatch);
}
