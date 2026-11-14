//! C0 報告 §9 の検証 V1〜V4・V6: 関数形ルートと `import`・条件・`policy` の同居、`Duration` の小数、
//! オブジェクト型の prop。実装を変えずに、挙動をテストで固定する。

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
    ComponentDef, Engine, Field, FromValue, Millis, ModuleResolver, Node, Program, Schema, Source,
    Ty, Value,
};

const M: &str = "host:t";

fn env_ty() -> Ty {
    Ty::object([(
        "output",
        Field::required(Ty::object([
            ("name", Field::required(Ty::String)),
            ("scale", Field::required(Ty::Number)),
        ])),
    )])
}

fn spec_ty() -> Ty {
    Ty::object([
        ("command", Field::required(Ty::String)),
        ("args", Field::required(Ty::Array(Box::new(Ty::String)))),
    ])
}

fn schema() -> Schema {
    let window = Ty::object([("isDialog", Field::required(Ty::Boolean))]);
    let placement = Ty::object([("floating", Field::required(Ty::Boolean))]);
    Schema::builder()
        .root(M, "Root")
        .env(M, env_ty())
        .component(
            M,
            ComponentDef::new("Root").children(&["Item", "Wait", "Job"]),
        )
        .component(M, ComponentDef::new("Item").prop("name", Ty::String))
        .component(M, ComponentDef::new("Wait").prop("d", Ty::Duration))
        .component(M, ComponentDef::new("Job").prop("spec", spec_ty()))
        .type_def(M, "WindowInfo", window.clone())
        .type_def(M, "Placement", placement.clone())
        .type_def(
            M,
            "Policy",
            Ty::object([(
                "windowOpen",
                Field::optional(Ty::Function(vec![window], Box::new(placement))),
            )]),
        )
        .build()
        .unwrap()
}

struct Memory(BTreeMap<&'static str, &'static str>);

impl ModuleResolver for Memory {
    fn resolve(&self, _importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        let id = specifier.trim_start_matches("./");
        self.0
            .get(id)
            .map(|t| Source {
                id: id.to_owned(),
                text: (*t).to_owned(),
            })
            .ok_or_else(|| "なし".to_owned())
    }
}

fn load(files: &[(&'static str, &'static str)]) -> Result<Program, Vec<cdl_host::Diagnostic>> {
    let files = Memory(files.iter().copied().collect());
    Engine::new().with_schema(schema()).load(&files, "main.cdl")
}

fn env(scale: f64) -> Value {
    Value::object([(
        "output",
        Value::object([
            ("name", Value::string("DP-1")),
            ("scale", Value::Number(scale)),
        ]),
    )])
}

fn names(tree: &cdl_host::Element) -> Vec<String> {
    Node::new(tree)
        .children_named("Item")
        .map(|c| c.prop::<String>("name").unwrap())
        .collect()
}

// ---- V1: import + 関数形ルート ----

const V1_MAIN: &str = "\"use cdl 1\";\n\
    import { Root } from \"host:t\";\n\
    import type { Env } from \"host:t\";\n\
    import { Part, BASE } from \"./parts.cdl\";\n\
    export default function App(env: Env): Element<Root> {\n\
    \x20 return <Root><Part scale={env.output.scale} /><Part scale={BASE} /></Root>;\n\
    }\n";

const V1_PARTS: &str = "\"use cdl 1\";\n\
    import { Item } from \"host:t\";\n\
    export const BASE = 8;\n\
    export function Part(props: { scale: number }): Element<Item> {\n\
    \x20 return <Item name={`s${props.scale * BASE}`} />;\n\
    }\n";

#[test]
fn v1_import_with_a_function_root() {
    let program = load(&[("main.cdl", V1_MAIN), ("parts.cdl", V1_PARTS)]).unwrap();
    assert!(program.is_function_root());
    let tree = program.render(&env(2.0)).unwrap();
    assert_eq!(names(&tree), ["s16", "s64"]);
    let tree = program.render(&env(1.5)).unwrap();
    assert_eq!(names(&tree), ["s12", "s64"]);
}

// ---- V2: 関数形ルートでの条件 ----

const V2: &str = "\"use cdl 1\";\n\
    import { Root, Item } from \"host:t\";\n\
    import type { Env } from \"host:t\";\n\
    export default function App({ output }: Env): Element<Root> {\n\
    \x20 return (<Root>{output.scale > 1 && <Item name=\"hidpi\" />}</Root>);\n\
    }\n";

#[test]
fn v2_conditional_child_in_a_function_root() {
    let program = load(&[("main.cdl", V2)]).unwrap();
    assert_eq!(names(&program.render(&env(2.0)).unwrap()), ["hidpi"]);
    assert!(names(&program.render(&env(1.0)).unwrap()).is_empty());
}

// ---- V3: policy と関数形ルートの同居 ----

const V3: &str = "\"use cdl 1\";\n\
    import { Root } from \"host:t\";\n\
    import type { Env, Policy, WindowInfo, Placement } from \"host:t\";\n\
    export const policy = { windowOpen: (w: WindowInfo): Placement => ({ floating: w.isDialog }) } satisfies Policy;\n\
    export default function App(env: Env): Element<Root> { return <Root />; }\n";

#[test]
fn v3_policy_and_a_function_root_coexist() {
    let program = load(&[("main.cdl", V3)]).unwrap();
    assert!(program.is_function_root());
    assert!(program.render(&env(1.0)).is_ok());
    let out = program
        .call_hook(
            "windowOpen",
            Value::object([("isDialog", Value::Bool(true))]),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out, Value::object([("floating", Value::Bool(true))]));
}

// ---- V4: Duration の小数 ----

fn wait_of(expr: &str) -> Result<Option<Millis>, Code> {
    let src: &'static str = Box::leak(
        format!(
            "\"use cdl 1\";\nimport {{ Root, Wait }} from \"host:t\";\n\
             export default (<Root><Wait d={{{expr}}} /></Root>);\n"
        )
        .into_boxed_str(),
    );
    match load(&[("main.cdl", src)]) {
        Ok(p) => {
            let root = p.root().unwrap();
            let w = root.children_named("Wait").next().unwrap();
            Ok(Some(w.prop::<Millis>("d").unwrap()))
        }
        Err(d) => Err(d[0].code),
    }
}

#[test]
fn v4_duration_from_ms_and_sec() {
    assert_eq!(wait_of("ms(150)"), Ok(Some(Millis(150))));
    assert_eq!(wait_of("sec(1)"), Ok(Some(Millis(1000))));
    assert_eq!(wait_of("ms(1.5)"), Err(Code::DurationRange));
    // V4 の実測: 下の 2 行の値は、実行して確かめた結果で固定する。
    assert_eq!(wait_of("sec(0.15)"), Ok(Some(Millis(150))));
    // C0 の推測（`1.1 * 1000.0` が整数にならず E5009）は外れた。実測は 1100。
    assert_eq!(wait_of("sec(1.1)"), Ok(Some(Millis(1100))));
    // 整数ミリ秒にならない秒は E5009（SPEC 第 4 章）。負の値は通る。
    assert_eq!(wait_of("sec(1.0001)"), Err(Code::DurationRange));
    assert_eq!(wait_of("sec(0.0004)"), Err(Code::DurationRange));
    assert_eq!(wait_of("ms(-5)"), Ok(Some(Millis(-5))));
}

// ---- V6: オブジェクト型の prop ----

#[derive(Debug, PartialEq)]
struct Spec {
    command: String,
    args: Vec<String>,
}

impl FromValue for Spec {
    fn expected() -> String {
        "{ command: string; args: string[] }".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        let Value::Object(m) = v else { return None };
        Some(Spec {
            command: String::from_value(m.get("command")?)?,
            args: Vec::<String>::from_value(m.get("args")?)?,
        })
    }
}

#[test]
fn v6_object_typed_prop_is_checked_and_readable() {
    let ok = "\"use cdl 1\";\nimport { Root, Job } from \"host:t\";\n\
        export default (<Root><Job spec={{ command: \"sh\", args: [\"-c\", \"x\"] }} /></Root>);\n";
    let program = load(&[("main.cdl", ok)]).unwrap();
    let root = program.root().unwrap();
    let job = root.children_named("Job").next().unwrap();
    assert_eq!(
        job.prop::<Spec>("spec").unwrap(),
        Spec {
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), "x".to_owned()],
        }
    );
    // オブジェクトリテラルの中の誤りは、文脈型のもとで E3xxx（E3003・E3002・E3001）になる。
    for (bad, code) in [
        ("{ command: \"sh\" }", Code::MissingProperty),
        (
            "{ command: \"sh\", args: [], extra: 1 }",
            Code::UnknownProperty,
        ),
        ("{ command: 1, args: [] }", Code::TypeMismatch),
    ] {
        let src: &'static str = Box::leak(
            format!(
                "\"use cdl 1\";\nimport {{ Root, Job }} from \"host:t\";\n\
                 export default (<Root><Job spec={{{bad}}} /></Root>);\n"
            )
            .into_boxed_str(),
        );
        let err = load(&[("main.cdl", src)]).unwrap_err();
        assert_eq!(err[0].code, code, "{bad}: {err:#?}");
    }
}
