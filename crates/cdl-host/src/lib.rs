//! CDL をホストに組み込むための公開 API。互換性を約束するのはこのクレートだけ（SPEC 第 11 章）。
//!
//! 使い方:
//! 1. [`Schema::builder`] で、組み込みコンポーネントとホストの型を登録する。
//! 2. [`ModuleResolver`] を実装する（ソースを読む唯一の経路）。
//! 3. [`Engine::load`] でエントリファイルを読み、[`Program`] を得る。
//! 4. [`Program::root`] から値木を読み、[`Program::call_hook`] でポリシーを呼ぶ。
#![forbid(unsafe_code)]

mod convert;
mod load;

use std::collections::BTreeMap;

use cdl_eval::sema::Checked;
use cdl_eval::sema::syntax::ast::Module;
use cdl_eval::{Evaluated, Unit};

pub use cdl_diag as diag;
pub use cdl_diag::Diagnostic;
pub use cdl_eval::sema::schema::{ComponentDef, Schema, SchemaBuilder};
pub use cdl_eval::sema::types::{Field, Literal, Ty};
pub use cdl_eval::{ByteRange, ChildNode, Element, Value};
pub use convert::{Child, FromNode, FromValue, Millis, Node, Px, Rgba};

/// リゾルバが返すソース。`id` は正準な名前で、同じファイルなら同じ `id` を返す（DECISIONS E6）。
/// 診断の `file` には `id` が入る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub text: String,
}

/// ソースを読む唯一の経路（SPEC 第 9 章）。ファイルシステムなどの実装はホストが用意する。
pub trait ModuleResolver {
    /// `importer` は読み込み元の `id`。エントリを解決するときは `None`。
    /// `specifier` は `./theme.cdl` のような相対パス（形は検査済み。DECISIONS M1）。
    /// 見つからなければ、利用者向けの理由を返す。
    /// import の書かれた場所ごとに呼ばれる。同じ `id` を返したファイルは 1 回だけ構文解析・評価する。
    fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String>;
}

/// リソース上限（SPEC 第 7 章、DECISIONS C10）。既定値は暫定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// 1 ファイルのソースサイズ（バイト）。
    pub max_source_bytes: u32,
    /// 構文の入れ子の深さ。
    pub max_nesting: u32,
    /// 1 回の読み込みで開くファイルの数（エントリを含む。E5013）。
    pub max_modules: u32,
    /// Document 全体の評価ステップ。
    pub max_steps: u64,
    /// 関数・コンポーネント・コールバックの呼び出しの深さ。
    pub max_call_depth: u32,
    /// 評価の再帰（式と要素の入れ子。呼び出しをまたいで数える）の深さ。
    pub max_eval_depth: u32,
    /// 1 つの文字列の長さ（UTF-8 のバイト数）。
    pub max_string_bytes: u32,
    /// 1 つの配列・マップの要素数。
    pub max_collection_len: u32,
    /// 評価結果の値木のノード数。
    pub max_nodes: u64,
    /// 評価結果の値木の深さ（要素・配列・オブジェクトを 1 段と数える）。
    pub max_value_depth: u32,
    /// フックの呼び出し 1 回あたりの評価ステップ。
    pub max_hook_steps: u64,
}

impl Default for Limits {
    fn default() -> Self {
        let syntax = cdl_eval::sema::syntax::Limits::default();
        let eval = cdl_eval::Limits::default();
        Limits {
            max_source_bytes: syntax.max_source_bytes,
            max_nesting: syntax.max_nesting,
            max_modules: 256,
            max_steps: eval.max_steps,
            max_call_depth: eval.max_call_depth,
            max_eval_depth: eval.max_eval_depth,
            max_string_bytes: eval.max_string_bytes,
            max_collection_len: eval.max_collection_len,
            max_nodes: eval.max_nodes,
            max_value_depth: eval.max_value_depth,
            max_hook_steps: eval.max_hook_steps,
        }
    }
}

impl Limits {
    fn syntax(&self) -> cdl_eval::sema::syntax::Limits {
        cdl_eval::sema::syntax::Limits {
            max_source_bytes: self.max_source_bytes,
            max_nesting: self.max_nesting,
        }
    }

    fn eval(&self) -> cdl_eval::Limits {
        cdl_eval::Limits {
            max_steps: self.max_steps,
            max_call_depth: self.max_call_depth,
            max_eval_depth: self.max_eval_depth,
            max_string_bytes: self.max_string_bytes,
            max_collection_len: self.max_collection_len,
            max_nodes: self.max_nodes,
            max_value_depth: self.max_value_depth,
            max_hook_steps: self.max_hook_steps,
        }
    }
}

/// CDL の処理系。スキーマと上限を持つ。
#[derive(Clone, Debug, Default)]
pub struct Engine {
    limits: Limits,
    schema: Schema,
}

impl Engine {
    pub fn new() -> Self {
        Engine::default()
    }

    /// ホストのスキーマ（組み込みコンポーネント・ホストの型・ルート要素）。
    pub fn with_schema(mut self, schema: Schema) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// エントリファイルを読み、import をたどって検査し、評価したプログラムを返す。
    /// 誤りがあれば、位置順に並べた診断を返す。
    pub fn load(
        &self,
        resolver: &dyn ModuleResolver,
        entry: &str,
    ) -> Result<Program, Vec<Diagnostic>> {
        load::load(self, resolver, entry)
    }

    /// [`Engine::load`] と同じ検査をして、診断だけを返す。
    pub fn check(&self, resolver: &dyn ModuleResolver, entry: &str) -> Result<(), Vec<Diagnostic>> {
        self.check_until(resolver, entry, Stage::Eval)
    }

    /// `stage` の終わりまでで止めて検査し、診断だけを返す（`cdl check --stage`）。
    pub fn check_until(
        &self,
        resolver: &dyn ModuleResolver,
        entry: &str,
        stage: Stage,
    ) -> Result<(), Vec<Diagnostic>> {
        load::load_until(self, resolver, entry, stage).map(|_| ())
    }
}

/// 検査を進める段階（SPEC 第 7 章）。前の段階に誤りがあれば、後の段階へは進まない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// すべてのファイルの構文解析と、import の解決（循環の検査を含む）。
    Syntax,
    /// 名前解決・型検査・スキーマ適合。
    Sema,
    /// 評価（リソース上限と評価の誤りを含む）。
    Eval,
}

/// 評価を終えたプログラム。要素ツリーとポリシーを 1 つの不変な値として持つ（DECISIONS E5）。
/// ホストはリロードのとき、新しい `Program` に成功したら丸ごと差し替える（SPEC 第 8 章）。
#[derive(Debug)]
pub struct Program {
    /// 依存先が先に来る順のファイル。
    modules: Vec<Module>,
    checked: Vec<Checked>,
    deps: Vec<BTreeMap<String, usize>>,
    entry: usize,
    evaluated: Evaluated,
    schema: Schema,
    limits: cdl_eval::Limits,
}

impl Program {
    /// 静的な形のルートの要素ツリー（コンポーネントはすべて展開済み）。
    /// ルートが関数の形のときは `None`（[`Program::render`] を使う）。
    pub fn document(&self) -> Option<&Element> {
        self.evaluated.document.as_deref()
    }

    /// 静的な形のルートを、値を読み出すための [`Node`] として返す。関数の形のルートでは `None`。
    pub fn root(&self) -> Option<Node<'_>> {
        self.document().map(Node::new)
    }

    /// ルートが関数の形（`export default function App(env: Env): Element<…>`）か。
    pub fn is_function_root(&self) -> bool {
        self.evaluated.document.is_none()
    }

    /// ホストの環境 `env`（`Value::Object`。型は、スキーマの `Env`）から、要素ツリーを作る（DECISIONS U1）。
    /// 評価は純粋で、同じ `env` からは必ず同じ木になる。呼び出しごとに独立したステップ予算を使うので、
    /// 1 つの `Program` を何度でも（複数のスレッドから）render できる。
    /// 静的な形のルートでは、`env` は無視する。`env` が `Env` の型に合わなければ E4014。
    /// 誤りは複数返りうる（見つかった `key` の重複はすべて。DECISIONS U3）。
    pub fn render(&self, env: &Value) -> Result<std::sync::Arc<Element>, Vec<Diagnostic>> {
        cdl_eval::render(
            &self.units(),
            self.entry,
            &self.schema,
            &self.evaluated,
            &self.limits,
            env,
        )
    }

    /// 読み込んだファイルの `id`（依存先が先）。
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.modules.iter().map(|m| m.file.as_str())
    }

    fn units(&self) -> Vec<Unit<'_>> {
        self.modules
            .iter()
            .zip(&self.checked)
            .zip(&self.deps)
            .map(|((module, checked), deps)| Unit {
                module,
                checked,
                deps,
            })
            .collect()
    }

    /// ポリシーのフックを呼ぶ（SPEC 第 8 章）。フックが定義されていなければ `Ok(None)`。
    /// 失敗したら診断を返す。そのときホストは、そのフックの既定の動作に戻す。
    pub fn call_hook(&self, hook: &str, arg: Value) -> Result<Option<Value>, Diagnostic> {
        cdl_eval::call_hook(
            &self.units(),
            self.entry,
            &self.schema,
            &self.evaluated,
            &self.limits,
            hook,
            arg,
        )
    }
}
