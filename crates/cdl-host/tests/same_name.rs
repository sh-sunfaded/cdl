//! 2 つの host モジュールに同名のコンポーネントがあっても、取り違えないこと（C0 の負債 T8）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cdl_host::diag::Code;
use cdl_host::{ComponentDef, Engine, ModuleResolver, Schema, Source};

struct One(String);

impl ModuleResolver for One {
    fn resolve(&self, _importer: Option<&str>, _specifier: &str) -> Result<Source, String> {
        Ok(Source {
            id: "main.cdl".to_owned(),
            text: self.0.clone(),
        })
    }
}

/// `host:a` の `Box` は一意な子を持たず、`host:b` の `Box` は `Leaf` を 1 つまでにする。
/// モジュール名の順で `host:a` が先に見つかる。
fn schema() -> Schema {
    Schema::builder()
        .root("host:b", "Root")
        .component("host:a", ComponentDef::new("Box").children(&["Leaf"]))
        .component("host:a", ComponentDef::new("Leaf"))
        .component("host:b", ComponentDef::new("Root").children(&["Box"]))
        .component(
            "host:b",
            ComponentDef::new("Box")
                .children(&["Leaf"])
                .unique_child("Leaf"),
        )
        .component("host:b", ComponentDef::new("Leaf"))
        .build()
        .unwrap()
}

/// `imports` の行を先頭に置いて読み込み、出た診断のコードを返す。
fn codes_with(imports: &str, body: &str) -> Vec<Code> {
    let src = format!("\"use cdl 1\";\n{imports}\n{body}\n");
    match Engine::new()
        .with_schema(schema())
        .load(&One(src), "main.cdl")
    {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(|d| d.code).collect(),
    }
}

/// `host:b` の `Root`・`Box`・`Leaf` を import したときの診断。
fn codes(body: &str) -> Vec<Code> {
    codes_with("import { Root, Box, Leaf } from \"host:b\";", body)
}

#[test]
fn a_static_duplicate_uses_the_imported_module() {
    assert_eq!(
        codes("export default (<Root><Box><Leaf /><Leaf /></Box></Root>);"),
        [Code::DuplicateUniqueChild]
    );
}

#[test]
fn a_dynamic_duplicate_uses_the_imported_module() {
    let body = "const xs = [1, 2];\n\
        export default (<Root><Box>{xs.map((x) => <Leaf />)}</Box></Root>);";
    assert_eq!(codes(body), [Code::DuplicateUniqueChild]);
}

/// 逆向き: `host:a` の `Box` を import したら、`host:b` の一意制約は適用されない。
#[test]
fn a_static_duplicate_of_the_other_module_is_not_checked() {
    let imports = "import { Root } from \"host:b\";\nimport { Box, Leaf } from \"host:a\";";
    assert_eq!(
        codes_with(
            imports,
            "export default (<Root><Box><Leaf /><Leaf /></Box></Root>);"
        ),
        Vec::<Code>::new()
    );
}

#[test]
fn a_dynamic_duplicate_of_the_other_module_is_not_checked() {
    let imports = "import { Root } from \"host:b\";\nimport { Box, Leaf } from \"host:a\";";
    let body = "const xs = [1, 2];\n\
        export default (<Root><Box>{xs.map((x) => <Leaf />)}</Box></Root>);";
    assert_eq!(codes_with(imports, body), Vec::<Code>::new());
}
