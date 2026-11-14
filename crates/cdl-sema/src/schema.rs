//! ホストのスキーマ（SPEC 第 6 章、DECISIONS E1・E2・E4）。
//!
//! スキーマは Rust のビルダーで登録する。CDL の中の `declare` は使わない。

use std::collections::{BTreeMap, BTreeSet};

use crate::types::{Field, Ty};

/// 組み込みコンポーネントの型契約。
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub(crate) props: BTreeMap<String, Field>,
    /// 子に置ける組み込みコンポーネント。
    pub(crate) children: BTreeSet<String>,
    /// 子の中に 1 つまでしか置けない組み込みコンポーネント。
    pub(crate) unique: BTreeSet<String>,
    /// 子に string（テキスト子）を受け付ける（DECISIONS R1）。
    pub(crate) text: bool,
}

/// `host:name` で取り込むモジュール。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostModule {
    pub(crate) components: BTreeMap<String, Component>,
    pub(crate) types: BTreeMap<String, Ty>,
}

/// ホストのスキーマ全体。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    pub(crate) modules: BTreeMap<String, HostModule>,
    /// ルート要素（モジュール名, コンポーネント名）。
    pub(crate) root: Option<(String, String)>,
    /// ホストの環境 `Env` の型（ルートを関数の形で書くときの引数。DECISIONS U1）。
    pub(crate) env: Option<Ty>,
}

impl Schema {
    pub fn builder() -> SchemaBuilder {
        SchemaBuilder::default()
    }

    /// ホストが宣言した環境 `Env` の型。
    pub fn env(&self) -> Option<&Ty> {
        self.env.as_ref()
    }

    /// 名前で組み込みコンポーネントを探す（モジュールの順に最初に見つかったもの）。
    /// 同名のコンポーネントが別モジュールにあると、取り違えうる。評価は使わず、
    /// [`Schema::component_in`] を使う（DECISIONS P3）。公開 API のため残す。
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.modules.values().find_map(|m| m.components.get(name))
    }

    /// `module` の組み込みコンポーネントを名前で探す。同名のコンポーネントが別モジュールにあっても、
    /// 取り違えない（DECISIONS P3）。
    pub fn component_in(&self, module: &str, name: &str) -> Option<&Component> {
        self.modules.get(module)?.components.get(name)
    }
}

impl Component {
    /// 子の中に 1 つまでしか置けないコンポーネント。
    pub fn unique(&self) -> &BTreeSet<String> {
        &self.unique
    }

    /// 子にテキスト（string）を置けるか。
    pub fn accepts_text(&self) -> bool {
        self.text
    }
}

/// 組み込みコンポーネントの定義。
#[derive(Clone, Debug)]
pub struct ComponentDef {
    name: String,
    component: Component,
}

impl ComponentDef {
    pub fn new(name: &str) -> Self {
        ComponentDef {
            name: name.to_owned(),
            component: Component {
                props: BTreeMap::new(),
                children: BTreeSet::new(),
                unique: BTreeSet::new(),
                text: false,
            },
        }
    }

    pub fn prop(mut self, name: &str, ty: Ty) -> Self {
        self.component
            .props
            .insert(name.to_owned(), Field::required(ty));
        self
    }

    pub fn optional_prop(mut self, name: &str, ty: Ty) -> Self {
        self.component
            .props
            .insert(name.to_owned(), Field::optional(ty));
        self
    }

    /// 子に置ける組み込みコンポーネント（同じモジュールのもの）。
    pub fn children(mut self, names: &[&str]) -> Self {
        self.component
            .children
            .extend(names.iter().map(|n| (*n).to_owned()));
        self
    }

    /// 子に string（テキスト子 `<Label>text</Label>` と `{"text"}`）を受け付ける（DECISIONS R1）。
    pub fn text_children(mut self) -> Self {
        self.component.text = true;
        self
    }

    /// 子の中に 1 つまでしか置けないコンポーネント。
    pub fn unique_child(mut self, name: &str) -> Self {
        self.component.unique.insert(name.to_owned());
        self
    }
}

/// スキーマのビルダー。誤りは [`SchemaBuilder::build`] でまとめて返す。
#[derive(Clone, Debug, Default)]
pub struct SchemaBuilder {
    modules: BTreeMap<String, HostModule>,
    root: Option<(String, String)>,
    env: Option<Ty>,
    errors: Vec<String>,
}

fn contains_function(ty: &Ty) -> bool {
    match ty {
        Ty::Function(..) => true,
        Ty::Array(t) | Ty::Record(t) => contains_function(t),
        Ty::Tuple(ts) | Ty::Union(ts) => ts.iter().any(contains_function),
        Ty::Object(fields) => fields.values().any(|f| contains_function(&f.ty)),
        _ => false,
    }
}

impl SchemaBuilder {
    /// ルート要素にするコンポーネント。
    pub fn root(mut self, module: &str, component: &str) -> Self {
        self.root = Some((module.to_owned(), component.to_owned()));
        self
    }

    /// ホストの環境 `Env` の型を宣言する。`module` の型 `Env` として CDL から見える
    /// （`import type { Env } from "host:contrast"`）。ルートを関数の形で書けるのは、宣言したときだけ。
    pub fn env(mut self, module: &str, ty: Ty) -> Self {
        if contains_function(&ty) {
            self.errors.push(format!(
                "{module}: `Env` に関数型は使えない（DECISIONS B9）"
            ));
        }
        if !matches!(ty, Ty::Object(_)) {
            self.errors
                .push(format!("{module}: `Env` はオブジェクト型にする"));
        }
        let m = self.modules.entry(module.to_owned()).or_default();
        if m.components.contains_key("Env") || m.types.contains_key("Env") {
            self.errors.push(format!("{module}: `Env` が重複している"));
        }
        m.types.insert("Env".to_owned(), ty.clone());
        self.env = Some(ty);
        self
    }

    pub fn component(mut self, module: &str, def: ComponentDef) -> Self {
        if !def
            .name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_uppercase())
        {
            self.errors.push(format!(
                "{module}: コンポーネント名 `{}` は大文字で始める",
                def.name
            ));
        }
        if let Some((prop, _)) = def
            .component
            .props
            .iter()
            .find(|(_, f)| contains_function(&f.ty))
        {
            self.errors.push(format!(
                "{module}: `{}` の props `{prop}` に関数型は使えない（DECISIONS B9）",
                def.name
            ));
        }
        let m = self.modules.entry(module.to_owned()).or_default();
        if m.components.contains_key(&def.name) || m.types.contains_key(&def.name) {
            self.errors
                .push(format!("{module}: `{}` が重複している", def.name));
        }
        m.components.insert(def.name, def.component);
        self
    }

    /// ホストが公開する型（`interface WindowInfo` など）。
    pub fn type_def(mut self, module: &str, name: &str, ty: Ty) -> Self {
        let m = self.modules.entry(module.to_owned()).or_default();
        if m.components.contains_key(name) || m.types.contains_key(name) {
            self.errors
                .push(format!("{module}: `{name}` が重複している"));
        }
        m.types.insert(name.to_owned(), ty);
        self
    }

    pub fn build(mut self) -> Result<Schema, Vec<String>> {
        for (module, m) in &self.modules {
            for (name, c) in &m.components {
                for child in &c.children {
                    if !m.components.contains_key(child) {
                        self.errors.push(format!(
                            "{module}: `{name}` の子 `{child}` が同じモジュールにない"
                        ));
                    }
                }
                for u in &c.unique {
                    if !c.children.contains(u) {
                        self.errors.push(format!(
                            "{module}: `{name}` の一意な子 `{u}` が子の一覧にない"
                        ));
                    }
                }
            }
        }
        if let Some((module, component)) = &self.root {
            let found = self
                .modules
                .get(module)
                .is_some_and(|m| m.components.contains_key(component));
            if !found {
                self.errors
                    .push(format!("ルート要素 `{module}` の `{component}` がない"));
            }
        }
        if self.errors.is_empty() {
            Ok(Schema {
                modules: self.modules,
                root: self.root,
                env: self.env,
            })
        } else {
            Err(self.errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_validates_references() {
        let err = Schema::builder()
            .root("host:x", "Missing")
            .component(
                "host:x",
                ComponentDef::new("A").children(&["B"]).unique_child("C"),
            )
            .component("host:x", ComponentDef::new("lower"))
            .component(
                "host:x",
                ComponentDef::new("F").prop("f", Ty::Function(vec![], Box::new(Ty::Number))),
            )
            .build()
            .unwrap_err();
        assert_eq!(err.len(), 5, "{err:#?}");
    }

    #[test]
    fn builder_accepts_a_valid_schema() {
        let schema = Schema::builder()
            .root("host:x", "A")
            .component(
                "host:x",
                ComponentDef::new("A").children(&["B"]).unique_child("B"),
            )
            .component("host:x", ComponentDef::new("B").prop("n", Ty::Number))
            .type_def("host:x", "T", Ty::Number)
            .build()
            .unwrap();
        assert_eq!(schema.root, Some(("host:x".to_owned(), "A".to_owned())));
    }
}
