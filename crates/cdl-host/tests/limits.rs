//! リソース上限の境界テスト（SPEC 第 7・12 章）。上限ちょうどなら成功し、1 超えると診断になる。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use cdl_host::diag::Code;
use cdl_host::{ComponentDef, Diagnostic, Engine, Limits, ModuleResolver, Schema, Source, Ty};

/// `main.cdl` だけを持つメモリ上のファイル。
struct One(String);

impl ModuleResolver for One {
    fn resolve(&self, _importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        if specifier == "main.cdl" {
            Ok(Source {
                id: specifier.to_owned(),
                text: self.0.clone(),
            })
        } else {
            Err("ない".to_owned())
        }
    }
}

fn schema() -> Schema {
    let m = "host:t";
    Schema::builder()
        .root(m, "Root")
        .component(m, ComponentDef::new("Root").children(&["Item"]))
        .component(m, ComponentDef::new("Item").prop("name", Ty::String))
        .build()
        .unwrap()
}

const HEAD: &str = "\"use cdl 1\";\nimport { Root, Item } from \"host:t\";\n";
const ROOT: &str = "export default (<Root />);\n";

fn check(src: &str, limits: Limits) -> Result<(), Vec<Diagnostic>> {
    Engine::new()
        .with_schema(schema())
        .with_limits(limits)
        .check(&One(src.to_owned()), "main.cdl")
}

fn codes(src: &str, limits: Limits) -> Vec<Code> {
    check(src, limits)
        .err()
        .unwrap_or_default()
        .iter()
        .map(|d| d.code)
        .collect()
}

/// 上限ちょうどは成功し、1 超えると `want` の診断が先頭に出る。
#[track_caller]
fn assert_boundary(at: &str, over: &str, limits: Limits, want: Code) {
    if let Err(d) = check(at, limits) {
        panic!("上限ちょうどは成功するはず: {d:#?}");
    }
    let got = codes(over, limits);
    assert_eq!(got.first(), Some(&want), "上限 + 1 は {want:?}: {got:?}");
}

#[test]
fn source_size() {
    let limits = Limits::default();
    assert_eq!(limits.max_source_bytes, 1 << 20);
    let src = |len: usize| {
        let base = format!("{HEAD}{ROOT}// ");
        format!("{base}{}", "a".repeat(len - base.len()))
    };
    let len = usize::try_from(limits.max_source_bytes).unwrap();
    assert_eq!(src(len).len(), len);
    assert_boundary(&src(len), &src(len + 1), limits, Code::SourceTooLarge);
}

#[test]
fn syntax_nesting() {
    for max in [64, 10] {
        let limits = Limits {
            max_nesting: max,
            ..Limits::default()
        };
        // `const a =` の `=` も連鎖の 1 つに数えるので、括弧は上限 - 1 個まで書ける。
        let n = usize::try_from(max).unwrap() - 1;
        let src = |n: usize| {
            format!(
                "{HEAD}const a = {}1{};\n{ROOT}",
                "(".repeat(n),
                ")".repeat(n)
            )
        };
        assert_boundary(&src(n), &src(n + 1), limits, Code::NestingTooDeep);
    }
    assert_eq!(Limits::default().max_nesting, 64);
}

#[test]
fn string_length() {
    let limits = Limits::default();
    assert_eq!(limits.max_string_bytes, 64 * 1024);
    let src = |len: usize| format!("{HEAD}const s = \"{}\";\n{ROOT}", "a".repeat(len));
    let len = usize::try_from(limits.max_string_bytes).unwrap();
    assert_boundary(&src(len), &src(len + 1), limits, Code::StringLimit);
}

#[test]
fn collection_length() {
    let limits = Limits::default();
    assert_eq!(limits.max_collection_len, 65_536);
    let src = |len: usize| format!("{HEAD}const a = [{}];\n{ROOT}", "0,".repeat(len));
    let len = usize::try_from(limits.max_collection_len).unwrap();
    assert_boundary(&src(len), &src(len + 1), limits, Code::CollectionLimit);
}

#[test]
fn call_depth() {
    let limits = Limits::default();
    assert_eq!(limits.max_call_depth, 64);
    // f0 ← f1 ← … ← f(n-1)。`f(n-1)()` は n 段の呼び出しになる。
    let src = |n: usize| {
        let mut body = String::from("function f0(): number { return 1; }\n");
        for i in 1..n {
            body.push_str(&format!(
                "function f{i}(): number {{ return f{}(); }}\n",
                i - 1
            ));
        }
        format!("{HEAD}{body}const a = f{}();\n{ROOT}", n - 1)
    };
    assert_boundary(&src(64), &src(65), limits, Code::CallDepth);
}

#[test]
fn value_depth() {
    let limits = Limits::default();
    assert_eq!(limits.max_value_depth, 64);
    // 構文の入れ子の上限を、`const` の連鎖で抜けても止まる。
    let src = |n: usize| {
        let mut body = String::from("const a0 = [1];\n");
        for i in 1..n {
            body.push_str(&format!("const a{i} = [a{}];\n", i - 1));
        }
        format!("{HEAD}{body}{ROOT}")
    };
    assert_boundary(&src(64), &src(65), limits, Code::ValueDepth);
    // ホストが上書きできる。
    let small = Limits {
        max_value_depth: 2,
        ..limits
    };
    assert_boundary(&src(2), &src(3), small, Code::ValueDepth);
}

/// ノード数は、小さな上限で境界を確かめる（既定の 1,000,000 では、先に評価ステップの上限に達する）。
#[test]
fn node_count() {
    let doc = format!("{HEAD}export default (<Root><Item name=\"a\" /></Root>);\n");
    // 要素 2 つと props の値 1 つで 3 ノード。
    let limits = |max_nodes| Limits {
        max_nodes,
        ..Limits::default()
    };
    assert!(check(&doc, limits(3)).is_ok());
    assert_eq!(codes(&doc, limits(2)).first(), Some(&Code::NodeLimit));
    assert_eq!(Limits::default().max_nodes, 1_000_000);
}

/// 評価ステップは、成功する最小の上限を探して、その 1 つ下が E5002 になることを確かめる。
#[test]
fn evaluation_steps() {
    let src = format!("{HEAD}const a = [1, 2, 3].map((x) => x + 1);\n{ROOT}");
    let with = |max_steps| Limits {
        max_steps,
        ..Limits::default()
    };
    let least = (0..1000)
        .find(|&s| check(&src, with(s)).is_ok())
        .expect("1000 までに成功する");
    assert!(least > 0);
    assert_eq!(codes(&src, with(least - 1)).first(), Some(&Code::StepLimit));
    assert!(check(&src, with(least + 1)).is_ok());
    assert_eq!(Limits::default().max_steps, 1_000_000);
}
