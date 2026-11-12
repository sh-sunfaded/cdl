//! `Program::render`（ルートが関数の形のとき。DECISIONS U1）の結合テスト。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use cdl_host::diag::Code;
use cdl_host::{
    ComponentDef, Engine, Field, Limits, ModuleResolver, Node, Program, Schema, Source, Ty, Value,
};

fn env_ty() -> Ty {
    Ty::object([(
        "output",
        Field::required(Ty::object([
            ("name", Field::required(Ty::String)),
            ("scale", Field::required(Ty::Number)),
        ])),
    )])
}

fn schema(with_env: bool) -> Schema {
    let m = "host:t";
    let b = Schema::builder()
        .root(m, "Root")
        .component(m, ComponentDef::new("Root").children(&["Item"]))
        .component(m, ComponentDef::new("Item").prop("name", Ty::String));
    let b = if with_env { b.env(m, env_ty()) } else { b };
    b.build().unwrap()
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

const FUNCTION_ROOT: &str = "\"use cdl 1\";\nimport { Root, Item } from \"host:t\";\nimport type { Env } from \"host:t\";\n\
    const names = [\"a\", \"b\", \"c\"];\n\
    export default function App({ output }: Env): Element<Root> {\n  return (\n    <Root>\n      {names.map((n) => <Item key={n} name={`${output.name}-${n}-${output.scale}`} />)}\n    </Root>\n  );\n}\n";

const STATIC_ROOT: &str = "\"use cdl 1\";\nimport { Root, Item } from \"host:t\";\nexport default (<Root><Item name=\"s\" /></Root>);\n";

fn load(src: &'static str, with_env: bool) -> Result<Program, Vec<cdl_host::Diagnostic>> {
    let files = Memory(BTreeMap::from([("main.cdl", src)]));
    Engine::new()
        .with_schema(schema(with_env))
        .load(&files, "main.cdl")
}

fn env(name: &str, scale: f64) -> Value {
    Value::object([(
        "output",
        Value::object([
            ("name", Value::string(name)),
            ("scale", Value::Number(scale)),
        ]),
    )])
}

fn names(tree: &cdl_host::Element) -> Vec<String> {
    Node::new(tree)
        .children()
        .map(|c| c.prop::<String>("name").unwrap())
        .collect()
}

#[test]
fn one_program_renders_many_times_and_is_deterministic() {
    let program = load(FUNCTION_ROOT, true).unwrap();
    assert!(program.is_function_root());
    assert!(program.root().is_none() && program.document().is_none());
    let a = program.render(&env("DP-1", 2.0)).unwrap();
    let b = program.render(&env("DP-1", 2.0)).unwrap();
    assert_eq!(a, b);
    assert_eq!(names(&a), ["DP-1-a-2", "DP-1-b-2", "DP-1-c-2"]);
    let c = program.render(&env("HDMI-1", 1.5)).unwrap();
    assert_eq!(names(&c), ["HDMI-1-a-1.5", "HDMI-1-b-1.5", "HDMI-1-c-1.5"]);
    // key は評価結果に残る。
    let keys: Vec<Option<&str>> = Node::new(&c).children().map(|n| n.key()).collect();
    assert_eq!(keys, [Some("a"), Some("b"), Some("c")]);
}

#[test]
fn render_is_shareable_across_threads() {
    let program = Arc::new(load(FUNCTION_ROOT, true).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let p = Arc::clone(&program);
            std::thread::spawn(move || {
                let tree = p.render(&env(&format!("O{i}"), 1.0)).unwrap();
                names(&tree)
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        assert_eq!(h.join().unwrap()[0], format!("O{i}-a-1"));
    }
}

#[test]
fn static_roots_ignore_the_env() {
    let program = load(STATIC_ROOT, true).unwrap();
    assert!(!program.is_function_root());
    let a = program.render(&env("x", 1.0)).unwrap();
    let b = program.render(&Value::Undefined).unwrap();
    assert_eq!(a, b);
    assert_eq!(&*a, program.document().unwrap());
    // Env を宣言していないホストでも、静的な形は読める。
    let program = load(STATIC_ROOT, false).unwrap();
    assert_eq!(names(&program.render(&Value::Undefined).unwrap()), ["s"]);
}

#[test]
fn a_wrong_env_value_is_a_diagnostic() {
    let program = load(FUNCTION_ROOT, true).unwrap();
    for bad in [
        Value::Undefined,
        Value::object([("output", Value::Number(1.0))]),
        Value::object([(
            "output",
            Value::object([("name", Value::Number(1.0)), ("scale", Value::Number(1.0))]),
        )]),
        // 余分なキーも誤り（閉じた型）。
        Value::object([
            (
                "output",
                match env("a", 1.0) {
                    Value::Object(m) => m["output"].clone(),
                    _ => panic!("object"),
                },
            ),
            ("extra", Value::Bool(true)),
        ]),
    ] {
        assert_eq!(program.render(&bad).unwrap_err()[0].code, Code::EnvMismatch);
    }
}

#[test]
fn each_render_has_its_own_step_budget() {
    let files = Memory(BTreeMap::from([("main.cdl", FUNCTION_ROOT)]));
    let probe = |steps: u64| {
        let limits = Limits {
            max_steps: steps,
            ..Limits::default()
        };
        Engine::new()
            .with_schema(schema(true))
            .with_limits(limits)
            .load(&files, "main.cdl")
            .map(|p| p.render(&env("DP", 1.0)).map(|_| ()))
    };
    // 読み込み（const の評価）だけなら通り、render で使い切る上限がある。
    let steps = (0..1000)
        .find(|n| matches!(probe(*n), Ok(Err(_))))
        .expect("render だけが上限を超える値がある");
    assert_eq!(probe(steps).unwrap().unwrap_err()[0].code, Code::StepLimit);
    // 予算は render ごとに新しい: 通る上限なら、何度 render しても通る。
    let enough = (steps..2000)
        .find(|n| matches!(probe(*n), Ok(Ok(()))))
        .unwrap();
    let limits = Limits {
        max_steps: enough,
        ..Limits::default()
    };
    let program = Engine::new()
        .with_schema(schema(true))
        .with_limits(limits)
        .load(&files, "main.cdl")
        .unwrap();
    for _ in 0..20 {
        program.render(&env("DP", 1.0)).unwrap();
    }
}

#[test]
fn a_function_root_needs_a_declared_env() {
    // Env を宣言していないホストでは、`Env` を import できず（E2008）、関数の形も E4013。
    let errs = load(FUNCTION_ROOT, false).unwrap_err();
    assert!(
        errs.iter().any(|d| d.code == Code::ImportNotFound),
        "{errs:#?}"
    );
    const NO_IMPORT: &str = "\"use cdl 1\";\nimport { Root } from \"host:t\";\n\
        export default function App(o: { x: number }): Element<Root> { return <Root />; }\n";
    let errs = load(NO_IMPORT, false).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(errs[0].code, Code::RootFunctionWithoutEnv);
}
