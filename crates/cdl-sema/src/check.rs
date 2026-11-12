//! 名前解決と型検査（SPEC 第 4・6・7 章）。
//!
//! 2 つの段階に分ける（DECISIONS D5）。
//! 1. 名前解決: 未定義・重複・隠蔽・参照グラフの循環・再帰型（E2xxx）。誤りがあればここで止める。
//! 2. 型検査とスキーマ適合（E3xxx・E4xxx）。誤りのあとも [`Ty::Error`] で続け、複数報告する。

use std::collections::{BTreeMap, BTreeSet};

use cdl_diag::{Code, Diagnostic};
use cdl_syntax::ast::*;

use crate::graph;
use crate::schema::{Component, Schema};
use crate::types::{Field, Literal, Ty, assignable};
use crate::{ExportedValue, Interface as ModuleInterface};

/// import なしで使える値と型（DECISIONS E3）。
const PRELUDE_VALUES: &[&str] = &["px", "ms", "sec", "color"];
const PRELUDE_TYPES: &[&str] = &["Length", "Duration", "Color", "Element", "Array", "Record"];
/// 配列の組み込みメソッド（DECISIONS B15）。
/// 型の深さ・参照の深さ・ノード数の上限（DECISIONS D12。AUDIT P2）。
const MAX_TYPE_DEPTH: u32 = 128;
const MAX_TYPE_NODES: u32 = 4096;

/// `{...x}` の中の `key` を、props として渡したときのメッセージ（DECISIONS U3）。
const KEY_IN_SPREAD: &str =
    "`key` は props ではないので、`{...x}` では渡せない（`key=\"…\"` と属性で書く）";

const METHODS: &[&str] = &["map", "filter", "reduce", "includes", "concat"];

fn is_component_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
}

#[derive(Clone, Copy)]
enum ValueDecl<'a> {
    Const(&'a ConstDecl),
    Function,
    HostComponent(&'a Component),
    /// ほかのファイルから import した値。
    Imported(&'a ExportedValue),
}

#[derive(Clone, Copy)]
struct ValueEntry<'a> {
    decl: ValueDecl<'a>,
    span: Span,
    /// 参照グラフのノード番号（const と function だけ）。
    node: Option<usize>,
}

#[derive(Clone)]
enum TypeDecl<'a> {
    Alias(&'a TypeAlias),
    Interface(&'a Interface),
    Host(Ty),
}

#[derive(Clone)]
struct TypeEntry<'a> {
    decl: TypeDecl<'a>,
    span: Span,
    node: Option<usize>,
}

/// 子 1 つぶんの情報。
struct ChildInfo {
    span: Span,
    /// 子に現れうる組み込みコンポーネント。`None` なら子に置けない値。
    names: Option<BTreeSet<String>>,
    /// 静的に 1 つの組み込みコンポーネントに定まる直接の子（DECISIONS S3）。
    single: Option<String>,
    /// テキスト（string）の子を含む（DECISIONS R1）。
    text: bool,
    /// 要素の `key` が文字列リテラルのときの値（静的な重複の検査。DECISIONS U3）。
    key: Option<String>,
    ty: Ty,
}

/// 属性・オブジェクトのフィールドを与えたもの 1 つ。
struct Provided {
    ty: Ty,
    /// 値が省略されうる（省略可能なフィールドのスプレッド）。
    maybe_absent: bool,
}

/// 条件から分かる、真のときと偽のときの絞り込み（DECISIONS B14）。
#[derive(Default)]
struct Facts {
    truthy: Vec<(String, Ty)>,
    falsy: Vec<(String, Ty)>,
}

pub(crate) struct Checker<'a> {
    file: &'a str,
    module: &'a Module,
    schema: &'a Schema,
    imports: &'a BTreeMap<String, ModuleInterface>,
    entry: bool,
    diags: Vec<Diagnostic>,
    values: BTreeMap<String, ValueEntry<'a>>,
    types: BTreeMap<String, TypeEntry<'a>>,
    /// 参照グラフのノード（const と function）の名前。
    value_nodes: Vec<String>,
    type_nodes: Vec<String>,
    resolved_types: BTreeMap<String, Ty>,
    resolving: BTreeSet<String>,
    const_types: BTreeMap<String, Ty>,
    signatures: BTreeMap<String, (Vec<Ty>, Ty)>,
    /// 型検査の局所スコープ。
    scopes: Vec<BTreeMap<String, Ty>>,
    narrowed: Vec<BTreeMap<String, Ty>>,
}

/// 意味解析の結果。
pub(crate) struct Outcome {
    pub const_order: Vec<String>,
    pub policy_type: Option<Ty>,
    pub interface: ModuleInterface,
    pub diagnostics: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    pub(crate) fn run(
        module: &'a Module,
        schema: &'a Schema,
        imports: &'a BTreeMap<String, ModuleInterface>,
        entry: bool,
        names_only: bool,
    ) -> Outcome {
        let mut c = Checker {
            file: &module.file,
            module,
            schema,
            imports,
            entry,
            diags: Vec::new(),
            values: BTreeMap::new(),
            types: BTreeMap::new(),
            value_nodes: Vec::new(),
            type_nodes: Vec::new(),
            resolved_types: BTreeMap::new(),
            resolving: BTreeSet::new(),
            const_types: BTreeMap::new(),
            signatures: BTreeMap::new(),
            scopes: Vec::new(),
            narrowed: Vec::new(),
        };
        let order = c.resolve_names();
        if c.diags.is_empty() && !names_only {
            c.check_types(&order);
        }
        let interface = c.interface();
        let const_order = order
            .iter()
            .filter_map(|&i| c.value_nodes.get(i).cloned())
            .filter(|n| matches!(c.values.get(n).map(|e| e.decl), Some(ValueDecl::Const(_))))
            .collect();
        // `export const policy` の型（ホストがフックを呼ぶときの引数の検査に使う）。
        let policy_type = module
            .items
            .iter()
            .any(|i| i.exported && matches!(&i.kind, ItemKind::Const(c) if c.name.name == "policy"))
            .then(|| c.const_types.get("policy").cloned())
            .flatten();
        Outcome {
            const_order,
            policy_type,
            interface,
            diagnostics: c.diags,
        }
    }

    /// このファイルの公開物（型検査のあと）。
    fn interface(&mut self) -> ModuleInterface {
        let mut i = ModuleInterface::default();
        for item in self.module.items.iter().filter(|i| i.exported) {
            match &item.kind {
                ItemKind::Const(c) => {
                    let ty = self
                        .const_types
                        .get(&c.name.name)
                        .cloned()
                        .unwrap_or(Ty::Error);
                    i.values
                        .insert(c.name.name.clone(), ExportedValue::Const(ty));
                }
                ItemKind::Function(f) => {
                    let (ps, r) = self
                        .signatures
                        .get(&f.name.name)
                        .cloned()
                        .unwrap_or((Vec::new(), Ty::Error));
                    i.values
                        .insert(f.name.name.clone(), ExportedValue::Function(ps, r));
                }
                ItemKind::TypeAlias(t) => {
                    let ty = self
                        .resolved_types
                        .get(&t.name.name)
                        .cloned()
                        .unwrap_or(Ty::Error);
                    i.types.insert(t.name.name.clone(), ty);
                }
                ItemKind::Interface(t) => {
                    let ty = self
                        .resolved_types
                        .get(&t.name.name)
                        .cloned()
                        .unwrap_or(Ty::Error);
                    i.types.insert(t.name.name.clone(), ty);
                }
                ItemKind::Import(_) | ItemKind::ExportDefault(_) => {}
            }
        }
        i
    }

    // ---- 診断 --------------------------------------------------------------

    fn span(&self, span: Span) -> cdl_diag::Span {
        cdl_diag::Span {
            file: self.file.to_owned(),
            start: span.start,
            end: span.end,
        }
    }

    fn err(&mut self, code: Code, span: Span, message: impl Into<String>) {
        let d = Diagnostic::error(code, self.span(span), message);
        self.diags.push(d);
    }

    fn err_help(&mut self, code: Code, span: Span, message: impl Into<String>, help: &str) {
        let d = Diagnostic::error(code, self.span(span), message).with_help(help);
        self.diags.push(d);
    }

    fn err_label(
        &mut self,
        code: Code,
        span: Span,
        message: impl Into<String>,
        other: Span,
        note: &str,
    ) {
        let d =
            Diagnostic::error(code, self.span(span), message).with_label(self.span(other), note);
        self.diags.push(d);
    }

    // ==== 1. 名前解決 ========================================================

    /// 名前を集めて解決し、参照グラフを検査する。戻り値は値のノードのトポロジカル順。
    fn resolve_names(&mut self) -> Vec<usize> {
        let module = self.module;
        for item in &module.items {
            if let ItemKind::Import(i) = &item.kind {
                self.import(i);
            }
        }
        for item in &module.items {
            match &item.kind {
                ItemKind::Const(c) => self.declare_value(&c.name, ValueDecl::Const(c)),
                ItemKind::Function(f) => self.declare_value(&f.name, ValueDecl::Function),
                ItemKind::TypeAlias(t) => self.declare_type(&t.name, TypeDecl::Alias(t)),
                ItemKind::Interface(i) => self.declare_type(&i.name, TypeDecl::Interface(i)),
                ItemKind::Import(_) | ItemKind::ExportDefault(_) => {}
            }
        }

        // ライブラリのファイルは export default と policy を持てない（DECISIONS E6）。
        if !self.entry {
            for item in &module.items {
                match &item.kind {
                    ItemKind::ExportDefault(_) => self.err(
                        Code::LibraryExport,
                        item.span,
                        "ライブラリのファイルは `export default` を持てない",
                    ),
                    ItemKind::Const(c) if item.exported && c.name.name == "policy" => self.err(
                        Code::LibraryExport,
                        c.name.span,
                        "`export const policy` はエントリファイルにだけ置ける",
                    ),
                    _ => {}
                }
            }
        }

        // 値の参照グラフ。
        let mut value_adj = vec![Vec::new(); self.value_nodes.len()];
        let mut type_adj = vec![Vec::new(); self.type_nodes.len()];
        for item in &module.items {
            let mut refs = BTreeSet::new();
            let mut type_refs = BTreeSet::new();
            let mut locals: Vec<BTreeMap<String, Span>> = Vec::new();
            let owner = match &item.kind {
                ItemKind::Const(c) => {
                    if let Some(t) = &c.ty {
                        self.resolve_type_names(t, &mut type_refs);
                    }
                    self.resolve_expr(&c.init, &mut locals, &mut refs);
                    self.values
                        .get(&c.name.name)
                        .and_then(|e| e.node)
                        .map(|n| (true, n))
                }
                ItemKind::Function(f) => {
                    self.resolve_function(f, &mut locals, &mut refs, &mut type_refs);
                    self.values
                        .get(&f.name.name)
                        .and_then(|e| e.node)
                        .map(|n| (true, n))
                }
                ItemKind::TypeAlias(t) => {
                    self.resolve_type_names(&t.ty, &mut type_refs);
                    self.types
                        .get(&t.name.name)
                        .and_then(|e| e.node)
                        .map(|n| (false, n))
                }
                ItemKind::Interface(i) => {
                    for m in &i.members {
                        self.resolve_type_names(&m.ty, &mut type_refs);
                    }
                    self.types
                        .get(&i.name.name)
                        .and_then(|e| e.node)
                        .map(|n| (false, n))
                }
                ItemKind::ExportDefault(e) => {
                    self.resolve_expr(e, &mut locals, &mut refs);
                    None
                }
                ItemKind::Import(_) => None,
            };
            match owner {
                Some((true, n)) => {
                    let edges = refs
                        .iter()
                        .filter_map(|r| self.values.get(r).and_then(|e| e.node))
                        .collect();
                    if let Some(slot) = value_adj.get_mut(n) {
                        *slot = edges;
                    }
                }
                Some((false, n)) => {
                    let edges = type_refs
                        .iter()
                        .filter_map(|r| self.types.get(r).and_then(|e| e.node))
                        .collect();
                    if let Some(slot) = type_adj.get_mut(n) {
                        *slot = edges;
                    }
                }
                None => {}
            }
        }
        self.report_cycles(&value_adj, true);
        self.report_cycles(&type_adj, false);
        graph::topo_order(&value_adj)
    }

    fn import(&mut self, i: &'a Import) {
        let source = &i.source.value;
        if !source.starts_with("host:") {
            let imports = self.imports;
            let Some(interface) = imports.get(source) else {
                self.err(
                    Code::ModuleNotFound,
                    i.source.span,
                    format!("`{source}` を解決できない"),
                );
                return;
            };
            for name in &i.names {
                let mut found = false;
                if let Some(v) = interface.values.get(&name.name) {
                    found = true;
                    if !i.type_only {
                        self.declare_value(name, ValueDecl::Imported(v));
                    }
                }
                if let Some(t) = interface.types.get(&name.name) {
                    found = true;
                    self.declare_type(name, TypeDecl::Host(t.clone()));
                }
                if !found {
                    self.err(
                        Code::ImportNotFound,
                        name.span,
                        format!("`{source}` は `{}` を export していない", name.name),
                    );
                }
            }
            return;
        }
        let Some(host) = self.schema.modules.get(source) else {
            self.err(
                Code::ModuleNotFound,
                i.source.span,
                format!("ホストのモジュール `{source}` はない"),
            );
            return;
        };
        for name in &i.names {
            if let Some(component) = host.components.get(&name.name) {
                self.declare_value(name, ValueDecl::HostComponent(component));
            } else if let Some(ty) = host.types.get(&name.name) {
                self.declare_type(name, TypeDecl::Host(ty.clone()));
            } else {
                self.err(
                    Code::ImportNotFound,
                    name.span,
                    format!("`{source}` に `{}` はない", name.name),
                );
            }
        }
    }

    fn declare_value(&mut self, name: &Ident, decl: ValueDecl<'a>) {
        if PRELUDE_VALUES.contains(&name.name.as_str()) {
            self.err(
                Code::Shadowing,
                name.span,
                format!("`{}` は組み込みの関数と同じ名前", name.name),
            );
            return;
        }
        if let Some(prev) = self.values.get(&name.name) {
            let prev = prev.span;
            self.err_label(
                Code::DuplicateName,
                name.span,
                format!("`{}` はすでに宣言されている", name.name),
                prev,
                "最初の宣言",
            );
            return;
        }
        let node = match decl {
            ValueDecl::Const(_) | ValueDecl::Function => {
                self.value_nodes.push(name.name.clone());
                Some(self.value_nodes.len().saturating_sub(1))
            }
            ValueDecl::HostComponent(_) | ValueDecl::Imported(_) => None,
        };
        self.values.insert(
            name.name.clone(),
            ValueEntry {
                decl,
                span: name.span,
                node,
            },
        );
    }

    fn declare_type(&mut self, name: &Ident, decl: TypeDecl<'a>) {
        if PRELUDE_TYPES.contains(&name.name.as_str()) {
            self.err(
                Code::Shadowing,
                name.span,
                format!("`{}` は組み込みの型と同じ名前", name.name),
            );
            return;
        }
        if let Some(prev) = self.types.get(&name.name) {
            let prev = prev.span;
            self.err_label(
                Code::DuplicateName,
                name.span,
                format!("型 `{}` はすでに宣言されている", name.name),
                prev,
                "最初の宣言",
            );
            return;
        }
        let node = match decl {
            TypeDecl::Alias(_) | TypeDecl::Interface(_) => {
                self.type_nodes.push(name.name.clone());
                Some(self.type_nodes.len().saturating_sub(1))
            }
            TypeDecl::Host(_) => None,
        };
        self.types.insert(
            name.name.clone(),
            TypeEntry {
                decl,
                span: name.span,
                node,
            },
        );
    }

    fn report_cycles(&mut self, adj: &[Vec<usize>], values: bool) {
        for component in graph::sccs(adj) {
            if !graph::is_cycle(adj, &component) {
                continue;
            }
            let names: Vec<String> = component
                .iter()
                .filter_map(|&i| {
                    if values {
                        self.value_nodes.get(i)
                    } else {
                        self.type_nodes.get(i)
                    }
                    .cloned()
                })
                .collect();
            let spans: Vec<Span> = names
                .iter()
                .filter_map(|n| {
                    if values {
                        self.values.get(n).map(|e| e.span)
                    } else {
                        self.types.get(n).map(|e| e.span)
                    }
                })
                .collect();
            let (Some(first), Some(&first_span)) = (names.first(), spans.first()) else {
                continue;
            };
            let (code, message) = if !values {
                (
                    Code::RecursiveType,
                    format!("型 `{first}` が自分自身を参照している（再帰型）"),
                )
            } else if names.iter().any(|n| {
                matches!(
                    self.values.get(n).map(|e| e.decl),
                    Some(ValueDecl::Const(_))
                )
            }) {
                (
                    Code::ConstCycle,
                    format!("`{first}` の値が循環して依存している"),
                )
            } else if names.len() == 1 {
                (Code::Recursion, format!("`{first}` が再帰している"))
            } else {
                (Code::Recursion, format!("`{first}` が相互再帰している"))
            };
            let mut d = Diagnostic::error(code, self.span(first_span), message);
            for (name, span) in names.iter().zip(&spans).skip(1) {
                d = d.with_label(self.span(*span), format!("`{name}` も循環に含まれる"));
            }
            self.diags.push(d);
        }
    }

    /// 局所の名前を束縛する。隠蔽は誤り（DECISIONS C6）。
    fn bind_local(&mut self, name: &Ident, locals: &mut [BTreeMap<String, Span>]) {
        let outer = locals
            .iter()
            .rev()
            .find_map(|s| s.get(&name.name).copied())
            .or_else(|| self.values.get(&name.name).map(|e| e.span));
        if let Some(outer) = outer {
            self.err_label(
                Code::Shadowing,
                name.span,
                format!("`{}` は外側の名前を隠す", name.name),
                outer,
                "外側の宣言",
            );
        } else if PRELUDE_VALUES.contains(&name.name.as_str()) {
            self.err(
                Code::Shadowing,
                name.span,
                format!("`{}` は組み込みの関数と同じ名前", name.name),
            );
        }
        if let Some(scope) = locals.last_mut() {
            scope.insert(name.name.clone(), name.span);
        }
    }

    fn resolve_function(
        &mut self,
        f: &FunctionDecl,
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
        type_refs: &mut BTreeSet<String>,
    ) {
        locals.push(BTreeMap::new());
        for p in &f.params {
            if let Some(t) = &p.ty {
                self.resolve_type_names(t, type_refs);
            }
        }
        // 既定値は引数を束縛する前に解決する（同じ関数の引数は見えない。DECISIONS R4）。
        self.resolve_param_defaults(&f.params, locals, refs);
        for p in &f.params {
            for name in p.bound_names() {
                self.bind_local(name, locals);
            }
        }
        if let Some(t) = &f.ret {
            self.resolve_type_names(t, type_refs);
        }
        self.resolve_block(&f.body, locals, refs);
        locals.pop();
    }

    /// 引数パターンの既定値の式を解決する。
    fn resolve_param_defaults(
        &mut self,
        params: &[Param],
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        for p in params {
            if let ParamPattern::Object(props, _) = &p.pattern {
                for pp in props {
                    if let Some(d) = &pp.default {
                        self.resolve_expr(d, locals, refs);
                    }
                }
            }
        }
    }

    fn resolve_block(
        &mut self,
        b: &Block,
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        locals.push(BTreeMap::new());
        for c in &b.consts {
            if let Some(t) = &c.ty {
                self.resolve_type_names(t, &mut BTreeSet::new());
            }
            self.resolve_expr(&c.init, locals, refs);
            self.bind_local(&c.name, locals);
        }
        self.resolve_stmt(&b.stmt, locals, refs);
        locals.pop();
    }

    fn resolve_stmt(
        &mut self,
        s: &Stmt,
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        match s {
            Stmt::Return(e, _) => self.resolve_expr(e, locals, refs),
            Stmt::If {
                cond,
                then,
                otherwise,
                ..
            } => {
                self.resolve_expr(cond, locals, refs);
                self.resolve_block(then, locals, refs);
                match otherwise {
                    Else::Block(b) => self.resolve_block(b, locals, refs),
                    Else::If(s) => self.resolve_stmt(s, locals, refs),
                }
            }
            Stmt::Switch {
                scrutinee,
                cases,
                default,
                ..
            } => {
                self.resolve_expr(scrutinee, locals, refs);
                for c in cases {
                    self.resolve_stmt(&c.body, locals, refs);
                }
                if let Some(d) = default {
                    self.resolve_stmt(d, locals, refs);
                }
            }
        }
    }

    fn resolve_reference(
        &mut self,
        name: &str,
        span: Span,
        locals: &[BTreeMap<String, Span>],
        refs: &mut BTreeSet<String>,
    ) {
        if locals.iter().any(|s| s.contains_key(name)) || PRELUDE_VALUES.contains(&name) {
            return;
        }
        if self.values.contains_key(name) {
            refs.insert(name.to_owned());
        } else {
            self.err(
                Code::UndefinedName,
                span,
                format!("`{name}` は定義されていない"),
            );
        }
    }

    fn resolve_expr(
        &mut self,
        e: &Expr,
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        match &e.kind {
            ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Bool(_) | ExprKind::Undefined => {
            }
            ExprKind::Ident(name) => self.resolve_reference(name, e.span, locals, refs),
            ExprKind::Template { exprs, .. } => {
                for x in exprs {
                    self.resolve_expr(x, locals, refs);
                }
            }
            ExprKind::Array(elems) => {
                for el in elems {
                    match el {
                        ArrayElem::Expr(x) | ArrayElem::Spread(x, _) => {
                            self.resolve_expr(x, locals, refs)
                        }
                    }
                }
            }
            ExprKind::Object(props) => {
                for p in props {
                    match p {
                        ObjProp::KeyValue { value, .. } => self.resolve_expr(value, locals, refs),
                        ObjProp::Shorthand(k) => {
                            self.resolve_reference(&k.name, k.span, locals, refs)
                        }
                        ObjProp::Spread(x, _) => self.resolve_expr(x, locals, refs),
                    }
                }
            }
            ExprKind::Unary { expr, .. } => self.resolve_expr(expr, locals, refs),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Logical { lhs, rhs, .. } => {
                self.resolve_expr(lhs, locals, refs);
                self.resolve_expr(rhs, locals, refs);
            }
            ExprKind::Conditional { test, cons, alt } => {
                self.resolve_expr(test, locals, refs);
                self.resolve_expr(cons, locals, refs);
                self.resolve_expr(alt, locals, refs);
            }
            ExprKind::Member { object, .. } => self.resolve_expr(object, locals, refs),
            ExprKind::Index { object, index } => {
                self.resolve_expr(object, locals, refs);
                self.resolve_expr(index, locals, refs);
            }
            ExprKind::Call { callee, args } => {
                self.resolve_expr(callee, locals, refs);
                for a in args {
                    self.resolve_expr(a, locals, refs);
                }
            }
            ExprKind::Arrow { params, ret, body } => {
                locals.push(BTreeMap::new());
                for p in params {
                    if let Some(t) = &p.ty {
                        self.resolve_type_names(t, &mut BTreeSet::new());
                    }
                }
                self.resolve_param_defaults(params, locals, refs);
                for p in params {
                    for name in p.bound_names() {
                        self.bind_local(name, locals);
                    }
                }
                if let Some(t) = ret {
                    self.resolve_type_names(t, &mut BTreeSet::new());
                }
                match body {
                    ArrowBody::Expr(x) => self.resolve_expr(x, locals, refs),
                    ArrowBody::Block(b) => self.resolve_block(b, locals, refs),
                }
                locals.pop();
            }
            ExprKind::Satisfies { expr, ty } => {
                self.resolve_expr(expr, locals, refs);
                self.resolve_type_names(ty, &mut BTreeSet::new());
            }
            ExprKind::Element(el) => self.resolve_element(el, locals, refs),
            ExprKind::Fragment(children) => self.resolve_children(children, locals, refs),
        }
    }

    fn resolve_element(
        &mut self,
        el: &Element,
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        if self.values.contains_key(&el.name.name) {
            refs.insert(el.name.name.clone());
        } else {
            self.err(
                Code::UndefinedName,
                el.name.span,
                format!("コンポーネント `{}` は定義されていない", el.name.name),
            );
        }
        for a in &el.attrs {
            match a {
                Attr::Named {
                    value: Some(AttrValue::Expr(x)),
                    ..
                }
                | Attr::Spread(x, _) => self.resolve_expr(x, locals, refs),
                Attr::Named { .. } => {}
            }
        }
        self.resolve_children(&el.children, locals, refs);
    }

    fn resolve_children(
        &mut self,
        children: &[Child],
        locals: &mut Vec<BTreeMap<String, Span>>,
        refs: &mut BTreeSet<String>,
    ) {
        for c in children {
            match c {
                Child::Element(el, _) => self.resolve_element(el, locals, refs),
                Child::Fragment(cs, _) => self.resolve_children(cs, locals, refs),
                Child::Expr(x, _) => self.resolve_expr(x, locals, refs),
                Child::Text(..) => {}
            }
        }
    }

    fn resolve_type_names(&mut self, t: &Type, refs: &mut BTreeSet<String>) {
        match t {
            Type::Boolean(_)
            | Type::Number(_)
            | Type::String(_)
            | Type::Undefined(_)
            | Type::Literal(_) => {}
            Type::Named { name, args, .. } => {
                if name.name == "Element" {
                    // 型引数はコンポーネント名（DECISIONS B5）。
                    for a in args {
                        self.resolve_component_type_arg(a);
                    }
                    return;
                }
                if !PRELUDE_TYPES.contains(&name.name.as_str()) {
                    if self.types.contains_key(&name.name) {
                        refs.insert(name.name.clone());
                    } else if !self.values.contains_key(&name.name) {
                        self.err(
                            Code::UndefinedName,
                            name.span,
                            format!("型 `{}` は定義されていない", name.name),
                        );
                    }
                }
                for a in args {
                    self.resolve_type_names(a, refs);
                }
            }
            Type::Array(t, _) => self.resolve_type_names(t, refs),
            Type::Tuple(ts, _) | Type::Union(ts, _) => {
                for t in ts {
                    self.resolve_type_names(t, refs);
                }
            }
            Type::Object(members, _) => {
                for m in members {
                    self.resolve_type_names(&m.ty, refs);
                }
            }
            Type::Function { params, ret, .. } => {
                for (_, t) in params {
                    self.resolve_type_names(t, refs);
                }
                self.resolve_type_names(ret, refs);
            }
        }
    }

    fn resolve_component_type_arg(&mut self, t: &Type) {
        match t {
            Type::Named { name, .. } if !self.values.contains_key(&name.name) => {
                self.err(
                    Code::UndefinedName,
                    name.span,
                    format!("コンポーネント `{}` は定義されていない", name.name),
                );
            }
            Type::Union(ts, _) => {
                for t in ts {
                    self.resolve_component_type_arg(t);
                }
            }
            _ => {}
        }
    }

    // ==== 2. 型検査 ==========================================================

    fn check_types(&mut self, order: &[usize]) {
        let module = self.module;
        // 型の宣言。
        let names: Vec<String> = self.types.keys().cloned().collect();
        for name in names {
            self.named_type(&name);
        }
        // 関数のシグネチャ（注釈だけから決まる）。
        for item in &module.items {
            if let ItemKind::Function(f) = &item.kind {
                let sig = self.signature(f);
                self.signatures.insert(f.name.name.clone(), sig);
            }
        }
        // const は依存先から順に（DECISIONS C7）。
        let ordered: Vec<String> = order
            .iter()
            .filter_map(|&i| self.value_nodes.get(i).cloned())
            .collect();
        for name in ordered {
            if let Some(ValueEntry {
                decl: ValueDecl::Const(c),
                ..
            }) = self.values.get(&name).copied()
            {
                let ty = self.top_const(c);
                let ty = self.bounded_type(&c.name, ty);
                self.const_types.insert(name, ty);
            }
        }
        for item in &module.items {
            if let ItemKind::Function(f) = &item.kind {
                self.function_body(f);
            }
        }
        if self.entry {
            self.check_root();
        }
    }

    // ---- 型注釈 -------------------------------------------------------------

    /// `const` の型が大きすぎれば E3013 にして `Ty::Error` にする（共有された値の型は、木として展開されるため。AUDIT P2）。
    fn bounded_type(&mut self, name: &Ident, ty: Ty) -> Ty {
        if ty.exceeds(MAX_TYPE_DEPTH, MAX_TYPE_NODES) {
            self.err(
                Code::TypeTooLarge,
                name.span,
                format!(
                    "`{}` の型が大きすぎる（深さ {MAX_TYPE_DEPTH}・ノード数 {MAX_TYPE_NODES} まで）",
                    name.name
                ),
            );
            Ty::Error
        } else {
            ty
        }
    }

    fn named_type(&mut self, name: &str) -> Ty {
        if let Some(t) = self.resolved_types.get(name) {
            return t.clone();
        }
        if !self.resolving.insert(name.to_owned()) {
            return Ty::Error;
        }
        if u32::try_from(self.resolving.len()).unwrap_or(u32::MAX) > MAX_TYPE_DEPTH {
            self.resolving.remove(name);
            if let Some(span) = self.types.get(name).map(|e| e.span) {
                self.err(
                    Code::TypeTooLarge,
                    span,
                    format!("型 `{name}` の参照が深すぎる（上限 {MAX_TYPE_DEPTH}）"),
                );
            }
            return Ty::Error;
        }
        let ty = match self.types.get(name).map(|e| e.decl.clone()) {
            Some(TypeDecl::Alias(a)) => self.ty(&a.ty),
            Some(TypeDecl::Interface(i)) => self.object_type(&i.members),
            Some(TypeDecl::Host(t)) => t,
            None => Ty::Error,
        };
        self.resolving.remove(name);
        let ty = if ty.exceeds(MAX_TYPE_DEPTH, MAX_TYPE_NODES) {
            if let Some(span) = self.types.get(name).map(|e| e.span) {
                self.err(
                    Code::TypeTooLarge,
                    span,
                    format!(
                        "型 `{name}` が大きすぎる（深さ {MAX_TYPE_DEPTH}・ノード数 {MAX_TYPE_NODES} まで）"
                    ),
                );
            }
            Ty::Error
        } else {
            ty
        };
        self.resolved_types.insert(name.to_owned(), ty.clone());
        ty
    }

    fn object_type(&mut self, members: &[PropSig]) -> Ty {
        let mut fields = BTreeMap::new();
        for m in members {
            let ty = self.ty(&m.ty);
            if fields.contains_key(&m.key.name) {
                self.err(
                    Code::DuplicateKey,
                    m.key.span,
                    format!("プロパティ `{}` が重複している", m.key.name),
                );
                continue;
            }
            fields.insert(
                m.key.name.clone(),
                Field {
                    ty,
                    optional: m.optional,
                },
            );
        }
        Ty::Object(fields)
    }

    fn ty(&mut self, t: &Type) -> Ty {
        match t {
            Type::Boolean(_) => Ty::Boolean,
            Type::Number(_) => Ty::Number,
            Type::String(_) => Ty::String,
            Type::Undefined(_) => Ty::Undefined,
            Type::Literal(l) => Ty::Literal(lit(l)),
            Type::Array(t, _) => Ty::Array(Box::new(self.ty(t))),
            Type::Tuple(ts, _) => Ty::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            Type::Object(members, _) => self.object_type(members),
            Type::Union(ts, span) => {
                let u = Ty::union(ts.iter().map(|t| self.ty(t)).collect());
                if u.is_valid_union() {
                    u
                } else {
                    self.err(
                        Code::InvalidType,
                        *span,
                        format!("`{u}`: ユニオンにできるのは、リテラルどうし・`T | undefined`・`Element` だけ"),
                    );
                    Ty::Error
                }
            }
            Type::Function { params, ret, .. } => Ty::Function(
                params.iter().map(|(_, t)| self.ty(t)).collect(),
                Box::new(self.ty(ret)),
            ),
            Type::Named { name, args, span } => self.named(name, args, *span),
        }
    }

    fn named(&mut self, name: &Ident, args: &[Type], span: Span) -> Ty {
        let n = name.name.as_str();
        let arity = |c: &mut Self, want: usize| {
            if args.len() == want {
                true
            } else {
                c.err(
                    Code::InvalidType,
                    span,
                    format!("`{n}` の型引数は {want} 個"),
                );
                false
            }
        };
        match n {
            "Length" if arity(self, 0) => Ty::Length,
            "Duration" if arity(self, 0) => Ty::Duration,
            "Color" if arity(self, 0) => Ty::Color,
            "Array" if arity(self, 1) => match args.first() {
                Some(t) => Ty::Array(Box::new(self.ty(t))),
                None => Ty::Error,
            },
            "Record" if arity(self, 2) => {
                let key = args.first().map(|t| self.ty(t));
                let value = args.get(1).map(|t| self.ty(t));
                match (key, value) {
                    (Some(Ty::String), Some(v)) => Ty::Record(Box::new(v)),
                    _ => {
                        self.err(
                            Code::InvalidType,
                            span,
                            "`Record` のキーの型は `string` だけ",
                        );
                        Ty::Error
                    }
                }
            }
            "Element" => {
                let mut names = BTreeSet::new();
                if args.is_empty() {
                    self.err(
                        Code::InvalidType,
                        span,
                        "`Element` には型引数（コンポーネント名）が要る",
                    );
                    return Ty::Error;
                }
                for a in args {
                    if !self.component_names(a, &mut names) {
                        return Ty::Error;
                    }
                }
                Ty::Element(names)
            }
            "Length" | "Duration" | "Color" | "Array" | "Record" => Ty::Error,
            _ => {
                if self.types.contains_key(n) {
                    if !args.is_empty() {
                        self.err(Code::InvalidType, span, format!("`{n}` は型引数をとらない"));
                    }
                    self.named_type(n)
                } else {
                    self.err(
                        Code::InvalidType,
                        name.span,
                        format!("`{n}` は型ではない（要素の型は `Element<{n}>` と書く）"),
                    );
                    Ty::Error
                }
            }
        }
    }

    /// `Element<…>` の型引数（組み込みコンポーネント名とそのユニオン）。
    fn component_names(&mut self, t: &Type, out: &mut BTreeSet<String>) -> bool {
        match t {
            Type::Named { name, args, .. }
                if args.is_empty()
                    && matches!(
                        self.values.get(&name.name).map(|e| e.decl),
                        Some(ValueDecl::HostComponent(_))
                    ) =>
            {
                out.insert(name.name.clone());
                true
            }
            Type::Union(ts, _) => ts.iter().all(|t| self.component_names(t, out)),
            other => {
                self.err(
                    Code::InvalidType,
                    type_span(other),
                    "`Element<…>` の型引数は組み込みコンポーネントの名前だけ",
                );
                false
            }
        }
    }

    // ---- 宣言 ---------------------------------------------------------------

    fn signature(&mut self, f: &FunctionDecl) -> (Vec<Ty>, Ty) {
        let mut params = Vec::new();
        for p in &f.params {
            match &p.ty {
                Some(t) => params.push(self.ty(t)),
                None => {
                    self.err(
                        Code::AnnotationRequired,
                        param_span(p),
                        format!("引数 {} には型注釈が要る", param_label(p)),
                    );
                    params.push(Ty::Error);
                }
            }
        }
        let ret = match &f.ret {
            Some(t) => self.ty(t),
            None => {
                self.err(
                    Code::AnnotationRequired,
                    f.name.span,
                    format!("関数 `{}` には戻り値の型注釈が要る", f.name.name),
                );
                Ty::Error
            }
        };
        if is_component_name(&f.name.name) {
            if params.len() != 1 {
                self.err(
                    Code::NotCallable,
                    f.name.span,
                    "コンポーネントは props を 1 つだけ受け取る",
                );
            } else if let Some(p) = params.first()
                && !matches!(p, Ty::Object(_) | Ty::Error)
            {
                self.err(
                    Code::TypeMismatch,
                    f.params.first().map_or(f.name.span, |p| p.span),
                    "コンポーネントの props の型はオブジェクト型",
                );
            }
            if let Some(Ty::Object(fields)) = params.first()
                && fields.contains_key("key")
            {
                self.err(
                    Code::UnknownProperty,
                    f.params.first().map_or(f.name.span, |p| p.span),
                    "`key` は props ではないので、props の型には書けない",
                );
            }
            if !matches!(ret, Ty::Element(_) | Ty::Error) {
                self.err(
                    Code::TypeMismatch,
                    f.ret.as_ref().map_or(f.name.span, type_span),
                    "コンポーネントの戻り値の型は `Element<…>`",
                );
            }
        }
        (params, ret)
    }

    /// エントリファイルの `export const policy` か。
    fn is_policy(&self, c: &ConstDecl) -> bool {
        self.entry
            && c.name.name == "policy"
            && self
                .module
                .items
                .iter()
                .any(|i| i.exported && matches!(&i.kind, ItemKind::Const(d) if std::ptr::eq(d, c)))
    }

    /// `export const policy` を検査する（SPEC 第 8 章、DECISIONS A6・E5）。
    /// `satisfies Policy`（ホストの型）が要り、`Policy` にないフックは E4010。
    fn policy(&mut self, c: &ConstDecl) -> Ty {
        let ExprKind::Satisfies {
            expr,
            ty: Type::Named { name, args, .. },
        } = &c.init.kind
        else {
            self.err(
                Code::PolicyNotSatisfied,
                c.name.span,
                "`export const policy` は `… satisfies Policy` と書く",
            );
            return self.expr(&c.init, None);
        };
        let is_host_policy = name.name == "Policy"
            && args.is_empty()
            && matches!(
                self.types.get("Policy").map(|t| &t.decl),
                Some(TypeDecl::Host(_))
            );
        if !is_host_policy {
            self.err(
                Code::PolicyNotSatisfied,
                c.name.span,
                "`export const policy` は、ホストの `Policy` 型で `satisfies Policy` と書く",
            );
            return self.expr(&c.init, None);
        }
        // `Policy` にないフックの名前。オブジェクトリテラルの未知のキー（E3002）を E4010 に読み替える。
        let hooks = match self.named_type("Policy") {
            Ty::Object(fields) => fields,
            _ => BTreeMap::new(),
        };
        let unknown: Vec<Span> = match &expr.kind {
            ExprKind::Object(props) => props
                .iter()
                .filter_map(|p| match p {
                    ObjProp::KeyValue { key, .. } | ObjProp::Shorthand(key)
                        if !hooks.contains_key(&key.name) =>
                    {
                        Some(key.span)
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let before = self.diags.len();
        let ty = self.expr(&c.init, None);
        for d in self.diags.iter_mut().skip(before) {
            let at = Span {
                start: d.span.start,
                end: d.span.end,
            };
            if d.code == Code::UnknownProperty && unknown.contains(&at) {
                d.code = Code::UnknownHook;
                d.message = "ホストの `Policy` にこのフックはない".to_owned();
            }
        }
        ty
    }

    fn top_const(&mut self, c: &ConstDecl) -> Ty {
        if self.is_policy(c) {
            return self.policy(c);
        }
        match &c.ty {
            Some(t) => {
                let ty = self.ty(t);
                self.check_against(&c.init, &ty, Code::TypeMismatch);
                ty
            }
            None => {
                if let ExprKind::Arrow { ret: None, .. } = &c.init.kind {
                    self.err(
                        Code::AnnotationRequired,
                        c.name.span,
                        format!("`{}` のアロー関数には戻り値の型注釈が要る", c.name.name),
                    );
                }
                self.expr(&c.init, None)
            }
        }
    }

    fn function_body(&mut self, f: &FunctionDecl) {
        let Some((params, ret)) = self.signatures.get(&f.name.name).cloned() else {
            return;
        };
        let mut scope = BTreeMap::new();
        for (p, t) in f.params.iter().zip(params) {
            self.bind_param(p, &t, &mut scope);
        }
        self.scopes.push(scope);
        self.block(&f.body, Some(&ret), &mut Vec::new());
        self.scopes.pop();
    }

    /// 引数を束縛する。オブジェクトパターンなら、型 `ty`（オブジェクト型）の各 prop を取り出し、
    /// 既定値を型付けする（DECISIONS R4）。既定値は、この関数の引数が見えない外側の場所で検査する。
    fn bind_param(&mut self, p: &Param, ty: &Ty, scope: &mut BTreeMap<String, Ty>) {
        let (props, pspan) = match &p.pattern {
            ParamPattern::Ident(id) => {
                scope.insert(id.name.clone(), ty.clone());
                return;
            }
            ParamPattern::Object(props, span) => (props, *span),
        };
        let fields = match ty {
            Ty::Object(f) => Some(f),
            Ty::Error => None,
            other => {
                self.err(
                    Code::TypeMismatch,
                    pspan,
                    format!("分割代入できるのはオブジェクト型（`{other}` が来ている）"),
                );
                None
            }
        };
        let mut seen = BTreeSet::new();
        for pp in props {
            let name = &pp.name.name;
            if !seen.insert(name.clone()) {
                self.err(
                    Code::DuplicateKey,
                    pp.name.span,
                    format!("分割代入の `{name}` が重複している"),
                );
                continue;
            }
            let field = fields.and_then(|f| f.get(name));
            if fields.is_some() && field.is_none() {
                self.err(
                    Code::UnknownProperty,
                    pp.name.span,
                    format!("`{ty}` にプロパティ `{name}` はない"),
                );
            }
            let bound = match (&pp.default, field) {
                (Some(d), Some(f)) if f.optional => {
                    let want = f.effective().without_undefined();
                    let got = self.expr(d, Some(&want));
                    if !assignable(&got, &want) {
                        self.err(
                            Code::TypeMismatch,
                            d.span,
                            format!("既定値の型は `{want}`（`{got}` を書いている）"),
                        );
                    }
                    want
                }
                (Some(d), Some(_)) => {
                    self.expr(d, None);
                    self.err_help(
                        Code::DefaultOnRequired,
                        d.span,
                        format!("`{name}` は省略可能（`?`）でないので、既定値は付けられない"),
                        "props の型で `name?: T` と書いた prop にだけ既定値を付けられる",
                    );
                    Ty::Error
                }
                (Some(d), None) => {
                    self.expr(d, None);
                    Ty::Error
                }
                (None, Some(f)) => f.effective(),
                (None, None) => Ty::Error,
            };
            scope.insert(name.clone(), bound);
        }
    }

    fn check_root(&mut self) {
        let roots: Vec<(&Expr, Span)> = self
            .module
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::ExportDefault(e) => Some((e, i.span)),
                _ => None,
            })
            .collect();
        let mut iter = roots.iter();
        let Some((root, _)) = iter.next() else {
            self.err(
                Code::RootElement,
                Span { start: 0, end: 0 },
                "エントリファイルに `export default`（ルート要素）がない",
            );
            return;
        };
        for (_, span) in iter {
            self.err(Code::RootElement, *span, "`export default` は 1 つだけ");
        }
        if let ExprKind::Arrow { params, ret, .. } = &root.kind {
            self.check_root_function(root, params, ret.as_ref());
            return;
        }
        let ty = self.expr(root, None);
        match &self.schema.root {
            Some((_, component)) if !assignable(&ty, &Ty::element(component)) => self.err(
                Code::RootElement,
                root.span,
                format!("ルート要素は `<{component}>`（この式の型は `{ty}`）"),
            ),
            // ルートの指定がなくても、ルートは 1 つの要素（SPEC 第 6 章）。
            None if !matches!(&ty, Ty::Element(names) if names.len() == 1) && ty != Ty::Error => {
                self.err(
                    Code::RootElement,
                    root.span,
                    format!("ルートは 1 つの要素（この式の型は `{ty}`）"),
                )
            }
            _ => {}
        }
    }

    /// ルートを関数の形で書いたとき（DECISIONS U1）。引数は `Env` 1 つ（注釈必須で、型は完全一致）、
    /// 戻り値の型の注釈は必須で、ホストが指定したルート要素。
    fn check_root_function(&mut self, root: &Expr, params: &[Param], ret: Option<&Type>) {
        let Some(env) = self.schema.env().cloned() else {
            self.err_help(
                Code::RootFunctionWithoutEnv,
                root.span,
                "このホストは `Env` を宣言していないので、ルートを関数の形で書けない",
                "ルートは要素で書く（`export default (<Root>…</Root>);`）",
            );
            return;
        };
        if ret.is_none() {
            self.err(
                Code::AnnotationRequired,
                root.span,
                "ルート関数には戻り値の型注釈が要る（`: Element<…>`）",
            );
        }
        let ty = self.expr(root, None);
        let Ty::Function(ps, r) = ty else {
            return;
        };
        match (params, ps.as_slice()) {
            ([p], [t]) if *t != env && *t != Ty::Error => self.err(
                Code::RootFunctionSignature,
                param_span(p),
                format!("ルート関数の引数の型は `Env`（`{env}`）。`{t}` と書いてある"),
            ),
            ([_], [_]) => {}
            _ => self.err(
                Code::RootFunctionSignature,
                root.span,
                format!("ルート関数の引数は `Env` 1 つ（{} 個ある）", params.len()),
            ),
        }
        if let Some((_, component)) = &self.schema.root
            && !assignable(&r, &Ty::element(component))
            && *r != Ty::Error
        {
            self.err(
                Code::RootElement,
                ret.map_or(root.span, type_span),
                format!("ルート関数の戻り値は `Element<{component}>`（`{r}` と書いてある）"),
            );
        }
    }

    // ---- 文 ------------------------------------------------------------------

    /// ブロックを検査する。`ret` がなければ、`returns` に戻り値の型を集める。
    fn block(&mut self, b: &Block, ret: Option<&Ty>, returns: &mut Vec<Ty>) {
        self.scopes.push(BTreeMap::new());
        for c in &b.consts {
            let ty = match &c.ty {
                Some(t) => {
                    let ty = self.ty(t);
                    self.check_against(&c.init, &ty, Code::TypeMismatch);
                    ty
                }
                None => self.expr(&c.init, None),
            };
            let ty = self.bounded_type(&c.name, ty);
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(c.name.name.clone(), ty);
            }
        }
        self.stmt(&b.stmt, ret, returns);
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &Stmt, ret: Option<&Ty>, returns: &mut Vec<Ty>) {
        match s {
            Stmt::Return(e, _) => match ret {
                Some(r) => {
                    self.check_against(e, r, Code::TypeMismatch);
                }
                None => {
                    let t = self.expr(e, None);
                    returns.push(t);
                }
            },
            Stmt::If {
                cond,
                then,
                otherwise,
                ..
            } => {
                self.condition(cond);
                let facts = self.facts(cond);
                self.narrowed.push(facts.truthy.into_iter().collect());
                self.block(then, ret, returns);
                self.narrowed.pop();
                self.narrowed.push(facts.falsy.into_iter().collect());
                match otherwise {
                    Else::Block(b) => self.block(b, ret, returns),
                    Else::If(s) => self.stmt(s, ret, returns),
                }
                self.narrowed.pop();
            }
            Stmt::Switch {
                scrutinee,
                cases,
                default,
                ..
            } => self.switch(scrutinee, cases, default.as_deref(), ret, returns),
        }
    }

    fn switch(
        &mut self,
        scrutinee: &Expr,
        cases: &[Case],
        default: Option<&Stmt>,
        ret: Option<&Ty>,
        returns: &mut Vec<Ty>,
    ) {
        let st = self.expr(scrutinee, None);
        let path = path_key(scrutinee);
        // 網羅すべきリテラルの集合（リテラルのユニオンか boolean のときだけ）。
        let mut domain: Option<Vec<Literal>> = Some(Vec::new());
        for m in st.members() {
            match (m, domain.as_mut()) {
                (Ty::Literal(l), Some(d)) => d.push(l.clone()),
                (Ty::Boolean, Some(d)) => {
                    d.push(Literal::Bool(true));
                    d.push(Literal::Bool(false));
                }
                _ => domain = None,
            }
        }
        if st == Ty::Error {
            domain = None;
        }
        let mut seen: Vec<Literal> = Vec::new();
        for c in cases {
            let l = lit(&c.test);
            let lspan = lit_span(&c.test);
            if seen.contains(&l) {
                self.err(
                    Code::DuplicateKey,
                    lspan,
                    format!("`case {l}` が重複している"),
                );
            } else {
                let fits = match &domain {
                    Some(d) => d.contains(&l),
                    None => assignable(&Ty::Literal(l.clone()), &st.without_undefined()),
                };
                if !fits && st != Ty::Error {
                    self.err(
                        Code::TypeMismatch,
                        lspan,
                        format!("`{l}` は `{st}` の値にならない"),
                    );
                }
                seen.push(l.clone());
            }
            let mut frame = BTreeMap::new();
            if let Some(p) = &path {
                frame.insert(p.clone(), Ty::Literal(l));
            }
            self.narrowed.push(frame);
            self.stmt(&c.body, ret, returns);
            self.narrowed.pop();
        }
        if let Some(d) = default {
            self.stmt(d, ret, returns);
        } else if st != Ty::Error {
            match &domain {
                Some(d) => {
                    let missing: Vec<String> = d
                        .iter()
                        .filter(|l| !seen.contains(l))
                        .map(|l| l.to_string())
                        .collect();
                    if !missing.is_empty() {
                        self.err(
                            Code::NonExhaustive,
                            scrutinee.span,
                            format!("`switch` が {} を網羅していない", missing.join("・")),
                        );
                    }
                }
                None => self.err(
                    Code::NonExhaustive,
                    scrutinee.span,
                    format!("`{st}` の `switch` には `default` が要る"),
                ),
            }
        }
    }

    fn condition(&mut self, cond: &Expr) {
        let t = self.expr(cond, Some(&Ty::Boolean));
        if !assignable(&t, &Ty::Boolean) {
            self.err(
                Code::TypeMismatch,
                cond.span,
                format!("条件は `boolean`（この式の型は `{t}`）"),
            );
        }
    }

    // ---- 絞り込み ----------------------------------------------------------

    fn narrowed_type(&self, key: &str) -> Option<Ty> {
        self.narrowed.iter().rev().find_map(|f| f.get(key).cloned())
    }

    /// 誤りを報告せずに、パスの今の型を求める。
    fn path_type(&self, e: &Expr) -> Option<Ty> {
        let key = path_key(e)?;
        if let Some(t) = self.narrowed_type(&key) {
            return Some(t);
        }
        match &e.kind {
            ExprKind::Ident(name) => self.variable_type(name),
            ExprKind::Member { object, prop, .. } => match self.path_type(object)? {
                Ty::Object(fields) => fields.get(&prop.name).map(Field::effective),
                _ => None,
            },
            _ => None,
        }
    }

    fn variable_type(&self, name: &str) -> Option<Ty> {
        if let Some(t) = self.scopes.iter().rev().find_map(|s| s.get(name)) {
            return Some(t.clone());
        }
        match self.values.get(name)?.decl {
            ValueDecl::Const(_) => Some(self.const_types.get(name).cloned().unwrap_or(Ty::Error)),
            ValueDecl::Function => {
                let (ps, r) = self.signatures.get(name)?;
                Some(Ty::Function(ps.clone(), Box::new(r.clone())))
            }
            ValueDecl::Imported(ExportedValue::Const(t)) => Some(t.clone()),
            ValueDecl::Imported(ExportedValue::Function(ps, r)) => {
                Some(Ty::Function(ps.clone(), Box::new(r.clone())))
            }
            ValueDecl::HostComponent(_) => None,
        }
    }

    fn facts(&self, cond: &Expr) -> Facts {
        match &cond.kind {
            ExprKind::Binary {
                op: op @ (BinaryOp::Eq | BinaryOp::Ne),
                lhs,
                rhs,
            } => {
                let path = match (&lhs.kind, &rhs.kind) {
                    (_, ExprKind::Undefined) => lhs,
                    (ExprKind::Undefined, _) => rhs,
                    _ => return Facts::default(),
                };
                let (Some(key), Some(t)) = (path_key(path), self.path_type(path)) else {
                    return Facts::default();
                };
                let is_undef = vec![(key.clone(), Ty::Undefined)];
                let not_undef = vec![(key, t.without_undefined())];
                if *op == BinaryOp::Eq {
                    Facts {
                        truthy: is_undef,
                        falsy: not_undef,
                    }
                } else {
                    Facts {
                        truthy: not_undef,
                        falsy: is_undef,
                    }
                }
            }
            ExprKind::Unary {
                op: UnaryOp::Not,
                expr,
            } => {
                let f = self.facts(expr);
                Facts {
                    truthy: f.falsy,
                    falsy: f.truthy,
                }
            }
            ExprKind::Logical {
                op: LogicalOp::And,
                lhs,
                rhs,
            } => {
                let mut truthy = self.facts(lhs).truthy;
                truthy.extend(self.facts(rhs).truthy);
                Facts {
                    truthy,
                    falsy: Vec::new(),
                }
            }
            ExprKind::Logical {
                op: LogicalOp::Or,
                lhs,
                rhs,
            } => {
                let mut falsy = self.facts(lhs).falsy;
                falsy.extend(self.facts(rhs).falsy);
                Facts {
                    truthy: Vec::new(),
                    falsy,
                }
            }
            _ => Facts::default(),
        }
    }

    // ---- 式 ------------------------------------------------------------------

    /// `e` を検査し、`target` に置けなければ `code` で報告する。式の型を返す。
    fn check_against(&mut self, e: &Expr, target: &Ty, code: Code) -> Ty {
        let t = self.expr(e, Some(target));
        if !assignable(&t, target) {
            self.err(
                code,
                e.span,
                format!("`{target}` が要るところに `{t}` がある"),
            );
        }
        t
    }

    /// 値として使う前に、`undefined` を含まないことを確かめる（SPEC 第 4 章）。
    fn defined(&mut self, t: Ty, span: Span) -> Ty {
        if t.contains_undefined() && t != Ty::Undefined {
            self.err(
                Code::MaybeUndefined,
                span,
                format!("`{t}` は undefined かもしれない（`??` か `!== undefined` で絞り込む）"),
            );
            Ty::Error
        } else {
            t
        }
    }

    fn expr(&mut self, e: &Expr, expected: Option<&Ty>) -> Ty {
        let span = e.span;
        match &e.kind {
            ExprKind::Number(n) => Ty::Literal(Literal::number(*n)),
            ExprKind::String(s) => Ty::Literal(Literal::String(s.clone())),
            ExprKind::Bool(b) => Ty::Literal(Literal::Bool(*b)),
            ExprKind::Undefined => Ty::Undefined,
            ExprKind::Template { exprs, .. } => {
                for x in exprs {
                    let t = self.expr(x, None);
                    let t = self.defined(t, x.span);
                    if !assignable(&t, &Ty::String) && !assignable(&t, &Ty::Number) {
                        self.err(
                            Code::TypeMismatch,
                            x.span,
                            format!("テンプレート文字列に補間できるのは `string` と `number` だけ（`{t}`）"),
                        );
                    }
                }
                Ty::String
            }
            ExprKind::Ident(name) => self.ident(name, span),
            ExprKind::Array(elems) => self.array(elems, span, expected),
            ExprKind::Object(props) => self.object(props, span, expected),
            ExprKind::Unary { op, expr } => {
                let t = self.expr(expr, None);
                let t = self.defined(t, expr.span).widen();
                match (op, &t) {
                    (_, Ty::Error) => Ty::Error,
                    (UnaryOp::Neg, Ty::Number | Ty::Length | Ty::Duration) => t,
                    (UnaryOp::Not, Ty::Boolean) => Ty::Boolean,
                    _ => {
                        let o = if *op == UnaryOp::Neg { "-" } else { "!" };
                        self.err(Code::BadOperand, span, format!("`{o}` は `{t}` に使えない"));
                        Ty::Error
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, span),
            ExprKind::Logical { op, lhs, rhs } => self.logical(*op, lhs, rhs, span, expected),
            ExprKind::Conditional { test, cons, alt } => {
                self.condition(test);
                let facts = self.facts(test);
                self.narrowed.push(facts.truthy.into_iter().collect());
                let a = self.expr(cons, expected);
                self.narrowed.pop();
                self.narrowed.push(facts.falsy.into_iter().collect());
                let b = self.expr(alt, expected);
                self.narrowed.pop();
                let u = join(a, b);
                if u.is_valid_union() {
                    u
                } else {
                    self.err(
                        Code::TypeMismatch,
                        span,
                        format!("条件演算子の両枝の型が合わない（`{u}`）"),
                    );
                    Ty::Error
                }
            }
            ExprKind::Member {
                object,
                prop,
                optional,
            } => self.member(e, object, prop, *optional),
            ExprKind::Index { object, index } => self.index(object, index),
            ExprKind::Call { callee, args } => self.call(callee, args, span),
            ExprKind::Arrow { params, ret, body } => {
                self.arrow(params, ret.as_ref(), body, expected)
            }
            ExprKind::Satisfies { expr, ty } => {
                let target = self.ty(ty);
                self.check_against(expr, &target, Code::TypeMismatch)
            }
            ExprKind::Element(el) => self.element(el),
            ExprKind::Fragment(children) => {
                let infos = self.children(children);
                let mut names = BTreeSet::new();
                let mut text_span = None;
                let mut mixed_span = None;
                for info in infos {
                    if info.text && text_span.is_none() {
                        text_span = Some(info.span);
                    }
                    match info.names {
                        Some(n) => {
                            if !n.is_empty() && mixed_span.is_none() {
                                mixed_span = Some(info.span);
                            }
                            names.extend(n);
                        }
                        None => self.err(
                            Code::TypeMismatch,
                            info.span,
                            format!("`{}` は要素の子に置けない", info.ty),
                        ),
                    }
                }
                // テキストを含むフラグメントは `string[]`（要素もあれば `(Element<…> | string)[]`。DECISIONS U2）。
                match (text_span, mixed_span) {
                    (Some(_), Some(_)) => {
                        Ty::Array(Box::new(Ty::union(vec![Ty::Element(names), Ty::String])))
                    }
                    (Some(_), None) => Ty::Array(Box::new(Ty::String)),
                    (None, _) => Ty::Array(Box::new(Ty::Element(names))),
                }
            }
        }
    }

    /// 名前がユーザーのコンポーネント（このファイルの関数か、import した関数で、大文字で始まる）か。
    fn is_user_component(&self, name: &str) -> bool {
        is_component_name(name)
            && matches!(
                self.values.get(name).map(|e| e.decl),
                Some(ValueDecl::Function | ValueDecl::Imported(ExportedValue::Function(..)))
            )
    }

    fn ident(&mut self, name: &str, span: Span) -> Ty {
        if let Some(t) = self.narrowed_type(name) {
            return t;
        }
        if let Some(t) = self.variable_type(name) {
            if self.is_user_component(name) && !self.scopes.iter().any(|s| s.contains_key(name)) {
                self.err(
                    Code::NotCallable,
                    span,
                    format!("コンポーネント `{name}` は JSX でだけ使える"),
                );
                return Ty::Error;
            }
            return t;
        }
        if matches!(
            self.values.get(name).map(|e| e.decl),
            Some(ValueDecl::HostComponent(_))
        ) {
            self.err(
                Code::NotCallable,
                span,
                format!("コンポーネント `{name}` は JSX でだけ使える"),
            );
        } else if PRELUDE_VALUES.contains(&name) {
            self.err(
                Code::NotCallable,
                span,
                format!("組み込み関数 `{name}` は呼び出して使う"),
            );
        }
        Ty::Error
    }

    fn member(&mut self, e: &Expr, object: &Expr, prop: &Ident, optional: bool) -> Ty {
        if let Some(key) = path_key(e)
            && let Some(t) = self.narrowed_type(&key)
        {
            return t;
        }
        let ot = self.expr(object, None);
        let (base, add_undefined) = if optional {
            (ot.without_undefined(), ot.contains_undefined())
        } else {
            (self.defined(ot, object.span), false)
        };
        let t = match &base {
            Ty::Error => Ty::Error,
            Ty::Object(fields) => match fields.get(&prop.name) {
                Some(f) => f.effective(),
                None => {
                    self.err(
                        Code::UnknownProperty,
                        prop.span,
                        format!("`{base}` にプロパティ `{}` はない", prop.name),
                    );
                    Ty::Error
                }
            },
            Ty::Array(_) | Ty::Tuple(_) if prop.name == "length" => Ty::Number,
            Ty::Array(_) | Ty::Tuple(_) if METHODS.contains(&prop.name.as_str()) => {
                self.err(
                    Code::NotCallable,
                    prop.span,
                    format!("`{}` は呼び出して使う", prop.name),
                );
                Ty::Error
            }
            Ty::Record(_) => {
                self.err(
                    Code::UnknownProperty,
                    prop.span,
                    "`Record` の値は `r[\"k\"]` で引く",
                );
                Ty::Error
            }
            other => {
                self.err(
                    Code::UnknownProperty,
                    prop.span,
                    format!("`{other}` にプロパティ `{}` はない", prop.name),
                );
                Ty::Error
            }
        };
        if add_undefined {
            Ty::union(vec![t, Ty::Undefined])
        } else {
            t
        }
    }

    fn index(&mut self, object: &Expr, index: &Expr) -> Ty {
        let ot = self.expr(object, None);
        let ot = self.defined(ot, object.span);
        let it = self.expr(index, None);
        let it = self.defined(it, index.span);
        let maybe = |t: Ty| Ty::union(vec![t, Ty::Undefined]);
        match &ot {
            Ty::Error => Ty::Error,
            _ if it == Ty::Error => Ty::Error,
            Ty::Array(t) if assignable(&it, &Ty::Number) => maybe((**t).clone()),
            Ty::Tuple(ts) if assignable(&it, &Ty::Number) => {
                let exact = match &it {
                    Ty::Literal(Literal::Number(n)) if n.fract() == 0.0 && *n >= 0.0 => {
                        usize::try_from(n.to_string().parse::<u64>().unwrap_or(u64::MAX))
                            .ok()
                            .and_then(|i| ts.get(i).cloned())
                    }
                    _ => None,
                };
                exact.unwrap_or_else(|| maybe(Ty::union(ts.clone())))
            }
            Ty::Record(t) if assignable(&it, &Ty::String) => maybe((**t).clone()),
            Ty::Array(_) | Ty::Tuple(_) | Ty::Record(_) => {
                self.err(
                    Code::TypeMismatch,
                    index.span,
                    format!("この添字には使えない型（`{it}`）"),
                );
                Ty::Error
            }
            other => {
                self.err(
                    Code::BadOperand,
                    object.span,
                    format!("`{other}` は添字で引けない"),
                );
                Ty::Error
            }
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, span: Span) -> Ty {
        let lt = self.expr(lhs, None);
        let rt = self.expr(rhs, None);
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            if lt == Ty::Error || rt == Ty::Error {
                return Ty::Boolean;
            }
            if !lt.is_comparable_kind() || !rt.is_comparable_kind() {
                self.err(
                    Code::BadOperand,
                    span,
                    format!("`{lt}` と `{rt}` は `===` で比べられない"),
                );
            } else if !lt.overlaps(&rt) {
                self.err(
                    Code::BadOperand,
                    span,
                    format!("`{lt}` と `{rt}` は等しくなりえない"),
                );
            }
            return Ty::Boolean;
        }
        let lt = self.defined(lt, lhs.span).widen();
        let rt = self.defined(rt, rhs.span).widen();
        if lt == Ty::Error || rt == Ty::Error {
            return Ty::Error;
        }
        use BinaryOp as B;
        use Ty as T;
        let result = match (op, &lt, &rt) {
            (B::Add | B::Sub, T::Number, T::Number) => Some(T::Number),
            (B::Add | B::Sub, T::Length, T::Length) => Some(T::Length),
            (B::Add | B::Sub, T::Duration, T::Duration) => Some(T::Duration),
            (B::Mul, T::Number, T::Number) => Some(T::Number),
            (B::Mul, T::Length, T::Number) | (B::Mul, T::Number, T::Length) => Some(T::Length),
            (B::Mul, T::Duration, T::Number) | (B::Mul, T::Number, T::Duration) => {
                Some(T::Duration)
            }
            (B::Div, T::Number, T::Number) => Some(T::Number),
            (B::Div, T::Length, T::Number) => Some(T::Length),
            (B::Div, T::Duration, T::Number) => Some(T::Duration),
            (B::Rem, T::Number, T::Number) => Some(T::Number),
            (B::Lt | B::Le | B::Gt | B::Ge, a, b)
                if a == b && matches!(a, T::Number | T::Length | T::Duration) =>
            {
                Some(T::Boolean)
            }
            _ => None,
        };
        result.unwrap_or_else(|| {
            self.err(
                Code::BadOperand,
                span,
                format!("`{}` は `{lt}` と `{rt}` に使えない", binary_op(op)),
            );
            Ty::Error
        })
    }

    fn logical(
        &mut self,
        op: LogicalOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        match op {
            LogicalOp::Coalesce => {
                let lt = self.expr(lhs, None);
                let rt = self.expr(rhs, expected);
                if lt == Ty::Error || rt == Ty::Error {
                    return Ty::Error;
                }
                let u = join(lt.without_undefined(), rt);
                if u.is_valid_union() {
                    u
                } else {
                    self.err(
                        Code::TypeMismatch,
                        rhs.span,
                        format!("`??` の両辺の型が合わない（`{u}`）"),
                    );
                    Ty::Error
                }
            }
            LogicalOp::And => {
                let lt = self.expr(lhs, None);
                let facts = self.facts(lhs);
                self.narrowed.push(facts.truthy.into_iter().collect());
                let rt = self.expr(rhs, None);
                self.narrowed.pop();
                // 左辺は boolean だけ。右辺は何でもよく、式の型は `false | B`（DECISIONS R2）。
                if !assignable(&lt, &Ty::Boolean) {
                    self.err_help(
                        Code::BadOperand,
                        span,
                        format!("`&&` の左辺は `boolean`（`{lt}` が来ている）"),
                        "`x !== undefined && …` のように、boolean にしてから使う",
                    );
                    return Ty::Error;
                }
                Ty::union(vec![Ty::Literal(Literal::Bool(false)), rt])
            }
            LogicalOp::Or => {
                let lt = self.expr(lhs, None);
                let facts = self.facts(lhs);
                self.narrowed.push(facts.falsy.into_iter().collect());
                let rt = self.expr(rhs, None);
                self.narrowed.pop();
                if !assignable(&lt, &Ty::Boolean) || !assignable(&rt, &Ty::Boolean) {
                    self.err(
                        Code::BadOperand,
                        span,
                        format!("`||` のオペランドは `boolean`（`{lt}` と `{rt}`）"),
                    );
                    return Ty::Error;
                }
                Ty::Boolean
            }
        }
    }

    // ---- 配列とオブジェクト --------------------------------------------------

    fn array(&mut self, elems: &[ArrayElem], span: Span, expected: Option<&Ty>) -> Ty {
        let context = expected.and_then(|t| {
            t.members()
                .iter()
                .find(|m| matches!(m, Ty::Array(_) | Ty::Tuple(_)))
                .cloned()
        });
        match context {
            Some(Ty::Tuple(ts))
                if ts.len() == elems.len()
                    && elems.iter().all(|e| matches!(e, ArrayElem::Expr(_))) =>
            {
                let mut ok = true;
                for (el, t) in elems.iter().zip(&ts) {
                    if let ArrayElem::Expr(x) = el {
                        let got = self.expr(x, Some(t));
                        if !assignable(&got, t) {
                            self.err(
                                Code::TypeMismatch,
                                x.span,
                                format!("`{t}` が要るところに `{got}` がある"),
                            );
                            ok = false;
                        }
                    }
                }
                if ok { Ty::Tuple(ts) } else { Ty::Error }
            }
            Some(Ty::Array(t)) => {
                let mut ok = true;
                for el in elems {
                    match el {
                        ArrayElem::Expr(x) => {
                            let got = self.expr(x, Some(&t));
                            if !assignable(&got, &t) {
                                let code = if is_slot_mismatch(&t, &got) {
                                    Code::SlotType
                                } else {
                                    Code::TypeMismatch
                                };
                                self.err(
                                    code,
                                    x.span,
                                    format!("`{t}` が要るところに `{got}` がある"),
                                );
                                ok = false;
                            }
                        }
                        ArrayElem::Spread(x, sspan) => {
                            let got = self.expr(x, None);
                            if !assignable(&got, &Ty::Array(t.clone())) {
                                self.err(
                                    Code::TypeMismatch,
                                    *sspan,
                                    format!("`{t}[]` が要るところに `{got}` を展開している"),
                                );
                                ok = false;
                            }
                        }
                    }
                }
                if ok { Ty::Array(t) } else { Ty::Error }
            }
            _ => {
                if elems.is_empty() {
                    self.err(Code::AnnotationRequired, span, "空の配列には型注釈が要る");
                    return Ty::Error;
                }
                let mut tys = Vec::new();
                for el in elems {
                    match el {
                        ArrayElem::Expr(x) => tys.push(self.expr(x, None).widen()),
                        ArrayElem::Spread(x, sspan) => match self.expr(x, None) {
                            Ty::Array(t) => tys.push(t.widen()),
                            Ty::Tuple(ts) => tys.extend(ts.iter().map(Ty::widen)),
                            Ty::Error => tys.push(Ty::Error),
                            other => {
                                self.err(
                                    Code::TypeMismatch,
                                    *sspan,
                                    format!("`{other}` は配列でないので展開できない"),
                                );
                                tys.push(Ty::Error);
                            }
                        },
                    }
                }
                let u = tys.into_iter().reduce(join).unwrap_or(Ty::Error);
                if u.is_valid_union() {
                    Ty::Array(Box::new(u))
                } else {
                    self.err(
                        Code::TypeMismatch,
                        span,
                        format!("配列の要素の型がそろわない（`{u}`）"),
                    );
                    Ty::Error
                }
            }
        }
    }

    /// スプレッドと明示のキーを、あとに書いたものが勝つように合わせる（DECISIONS S1）。
    fn provide(fields: &mut BTreeMap<String, Provided>, key: &str, p: Provided) {
        let merged = match fields.remove(key) {
            Some(prev) if p.maybe_absent => Provided {
                ty: Ty::union(vec![prev.ty, p.ty.without_undefined()]),
                maybe_absent: prev.maybe_absent,
            },
            _ => p,
        };
        fields.insert(key.to_owned(), merged);
    }

    /// スプレッドの値のフィールド。オブジェクト型でなければ `None`。
    fn spread_fields(t: &Ty) -> Option<Vec<(String, Provided)>> {
        match t {
            Ty::Object(fields) => Some(
                fields
                    .iter()
                    .map(|(k, f)| {
                        (
                            k.clone(),
                            Provided {
                                ty: f.ty.clone(),
                                maybe_absent: f.optional,
                            },
                        )
                    })
                    .collect(),
            ),
            Ty::Error => Some(Vec::new()),
            _ => None,
        }
    }

    fn object(&mut self, props: &[ObjProp], span: Span, expected: Option<&Ty>) -> Ty {
        let context = expected.and_then(|t| {
            let found: Vec<&Ty> = t
                .members()
                .iter()
                .filter(|m| matches!(m, Ty::Object(_) | Ty::Record(_)))
                .collect();
            match found.as_slice() {
                [only] => Some((*only).clone()),
                _ => None,
            }
        });
        if let Some(Ty::Record(t)) = &context {
            let mut ok = true;
            for p in props {
                match p {
                    ObjProp::KeyValue { value, .. } => {
                        ok &= assignable(&self.check_against(value, t, Code::TypeMismatch), t);
                    }
                    ObjProp::Shorthand(k) => {
                        let got = self.ident(&k.name, k.span);
                        if !assignable(&got, t) {
                            self.err(
                                Code::TypeMismatch,
                                k.span,
                                format!("`{t}` が要るところに `{got}` がある"),
                            );
                            ok = false;
                        }
                    }
                    ObjProp::Spread(x, sspan) => {
                        let got = self.expr(x, None);
                        let fits = match &got {
                            Ty::Record(u) => assignable(u, t),
                            Ty::Object(fields) => {
                                fields.values().all(|f| assignable(&f.effective(), t))
                            }
                            Ty::Error => true,
                            _ => false,
                        };
                        if !fits {
                            self.err(
                                Code::TypeMismatch,
                                *sspan,
                                format!("`{got}` は `Record<string, {t}>` に展開できない"),
                            );
                            ok = false;
                        }
                    }
                }
            }
            return if ok { Ty::Record(t.clone()) } else { Ty::Error };
        }
        let target = match &context {
            Some(Ty::Object(f)) => Some(f.clone()),
            _ => None,
        };
        let mut fields: BTreeMap<String, Provided> = BTreeMap::new();
        let mut explicit: BTreeSet<String> = BTreeSet::new();
        let mut ok = true;
        for p in props {
            let (key, kspan, value_ty) = match p {
                ObjProp::KeyValue { key, value, .. } => {
                    let want = target
                        .as_ref()
                        .and_then(|t| t.get(&key.name))
                        .map(Field::effective);
                    let got = match &want {
                        Some(w) => {
                            let got = self.check_against(value, w, Code::TypeMismatch);
                            ok &= assignable(&got, w);
                            got
                        }
                        None => self.expr(value, None),
                    };
                    (key.name.clone(), key.span, got)
                }
                ObjProp::Shorthand(k) => {
                    let got = self.ident(&k.name, k.span);
                    if let Some(w) = target
                        .as_ref()
                        .and_then(|t| t.get(&k.name))
                        .map(Field::effective)
                        && !assignable(&got, &w)
                    {
                        self.err(
                            Code::TypeMismatch,
                            k.span,
                            format!("`{w}` が要るところに `{got}` がある"),
                        );
                        ok = false;
                    }
                    (k.name.clone(), k.span, got)
                }
                ObjProp::Spread(x, sspan) => {
                    let got = self.expr(x, None);
                    match Self::spread_fields(&got) {
                        Some(fs) => {
                            for (k, pf) in fs {
                                explicit.remove(&k);
                                if let Some(t) = &target {
                                    match t.get(&k) {
                                        None => {
                                            self.err(
                                                Code::UnknownProperty,
                                                *sspan,
                                                format!(
                                                    "展開した `{k}` は `{}` にない",
                                                    Ty::Object(t.clone())
                                                ),
                                            );
                                            ok = false;
                                        }
                                        Some(w) if !assignable(&pf.ty, &w.effective()) => {
                                            self.err(
                                                Code::TypeMismatch,
                                                *sspan,
                                                format!(
                                                    "展開した `{k}` の型 `{}` は `{}` に合わない",
                                                    pf.ty,
                                                    w.effective()
                                                ),
                                            );
                                            ok = false;
                                        }
                                        Some(_) => {}
                                    }
                                }
                                Self::provide(&mut fields, &k, pf);
                            }
                        }
                        None => {
                            self.err(
                                Code::TypeMismatch,
                                *sspan,
                                format!("`{got}` はオブジェクトでないので展開できない"),
                            );
                            ok = false;
                        }
                    }
                    continue;
                }
            };
            if !explicit.insert(key.clone()) {
                self.err(
                    Code::DuplicateKey,
                    kspan,
                    format!("キー `{key}` が重複している"),
                );
                ok = false;
                continue;
            }
            if let Some(t) = &target
                && !t.contains_key(&key)
            {
                self.err(
                    Code::UnknownProperty,
                    kspan,
                    format!("`{}` にキー `{key}` はない", Ty::Object(t.clone())),
                );
                ok = false;
            }
            Self::provide(
                &mut fields,
                &key,
                Provided {
                    ty: value_ty,
                    maybe_absent: false,
                },
            );
        }
        if let Some(t) = &target {
            let missing: Vec<&String> = t
                .iter()
                .filter(|(k, f)| !f.optional && fields.get(*k).is_none_or(|p| p.maybe_absent))
                .map(|(k, _)| k)
                .collect();
            if !missing.is_empty() {
                let names: Vec<String> = missing.iter().map(|k| format!("`{k}`")).collect();
                self.err(
                    Code::MissingProperty,
                    span,
                    format!("{} がない", names.join("・")),
                );
                ok = false;
            }
            // 文脈の型に照らして誤りがなければ、文脈の型を値の型にする（TypeScript の文脈型と同じ）。
            return if ok { Ty::Object(t.clone()) } else { Ty::Error };
        }
        Ty::Object(
            fields
                .into_iter()
                .map(|(k, p)| {
                    let ty = p.ty.widen();
                    (
                        k,
                        Field {
                            ty: if p.maybe_absent {
                                ty.without_undefined()
                            } else {
                                ty
                            },
                            optional: p.maybe_absent,
                        },
                    )
                })
                .collect(),
        )
    }

    // ---- 呼び出しと関数 ------------------------------------------------------

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Ty {
        if let ExprKind::Member {
            object,
            prop,
            optional: false,
        } = &callee.kind
            && METHODS.contains(&prop.name.as_str())
        {
            let ot = self.expr(object, None);
            let ot = self.defined(ot, object.span);
            let elem = match &ot {
                Ty::Array(t) => Some((**t).clone()),
                Ty::Tuple(ts) => Some(Ty::union(ts.clone())),
                Ty::Error => return Ty::Error,
                _ => None,
            };
            if let Some(elem) = elem {
                return self.method(&prop.name, &elem, args, span);
            }
            self.err(
                Code::NotCallable,
                prop.span,
                format!("`{ot}` にメソッド `{}` はない", prop.name),
            );
            return Ty::Error;
        }
        if let ExprKind::Ident(name) = &callee.kind
            && !self.scopes.iter().any(|s| s.contains_key(name))
        {
            if self.is_user_component(name) {
                self.err(
                    Code::NotCallable,
                    span,
                    format!(
                        "コンポーネント `{name}` は関数として呼べない（`<{name} … />` と書く）"
                    ),
                );
                return Ty::Error;
            }
            if matches!(
                self.values.get(name).map(|e| e.decl),
                Some(ValueDecl::HostComponent(_))
            ) {
                self.err(
                    Code::NotCallable,
                    span,
                    format!(
                        "コンポーネント `{name}` は関数として呼べない（`<{name} … />` と書く）"
                    ),
                );
                return Ty::Error;
            }
            if PRELUDE_VALUES.contains(&name.as_str()) && !self.values.contains_key(name) {
                return self.prelude_call(name, args, span);
            }
        }
        let ct = self.expr(callee, None);
        let ct = self.defined(ct, callee.span);
        match ct {
            Ty::Error => Ty::Error,
            Ty::Function(params, ret) => {
                if params.len() != args.len() {
                    self.err(
                        Code::NotCallable,
                        span,
                        format!("引数は {} 個（{} 個ある）", params.len(), args.len()),
                    );
                    return *ret;
                }
                for (a, p) in args.iter().zip(&params) {
                    self.check_against(a, p, Code::TypeMismatch);
                }
                *ret
            }
            other => {
                self.err(Code::NotCallable, span, format!("`{other}` は呼び出せない"));
                Ty::Error
            }
        }
    }

    fn prelude_call(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
        let (param, ret) = match name {
            "px" => (Ty::Number, Ty::Length),
            "ms" | "sec" => (Ty::Number, Ty::Duration),
            _ => (Ty::String, Ty::Color),
        };
        match args {
            [a] => {
                self.check_against(a, &param, Code::TypeMismatch);
            }
            _ => self.err(Code::NotCallable, span, format!("`{name}` の引数は 1 個")),
        }
        ret
    }

    fn method(&mut self, name: &str, elem: &Ty, args: &[Expr], span: Span) -> Ty {
        let arity = |c: &mut Self, n: usize| {
            if args.len() == n {
                true
            } else {
                c.err(Code::NotCallable, span, format!("`{name}` の引数は {n} 個"));
                false
            }
        };
        match name {
            "map" if arity(self, 1) => {
                let u = args.first().map_or(Ty::Error, |cb| {
                    self.callback(cb, std::slice::from_ref(elem), None)
                });
                Ty::Array(Box::new(u.widen()))
            }
            "filter" if arity(self, 1) => {
                if let Some(cb) = args.first() {
                    let r = self.callback(cb, std::slice::from_ref(elem), Some(&Ty::Boolean));
                    if !assignable(&r, &Ty::Boolean) {
                        self.err(
                            Code::TypeMismatch,
                            cb.span,
                            format!("`filter` の関数は `boolean` を返す（`{r}`）"),
                        );
                    }
                }
                Ty::Array(Box::new(elem.clone()))
            }
            "reduce" if arity(self, 2) => {
                let (Some(cb), Some(init)) = (args.first(), args.get(1)) else {
                    return Ty::Error;
                };
                let acc = self.expr(init, None).widen();
                let r = self.callback(cb, &[acc.clone(), elem.clone()], Some(&acc));
                if !assignable(&r, &acc) {
                    self.err(
                        Code::TypeMismatch,
                        cb.span,
                        format!("`reduce` の関数は `{acc}` を返す（`{r}`）"),
                    );
                }
                acc
            }
            "includes" if arity(self, 1) => {
                if let Some(x) = args.first() {
                    let t = self.expr(x, None);
                    if !elem.is_comparable_kind() || !t.is_comparable_kind() || !t.overlaps(elem) {
                        self.err(
                            Code::TypeMismatch,
                            x.span,
                            format!("`{elem}` の配列から `{t}` は探せない"),
                        );
                    }
                }
                Ty::Boolean
            }
            "concat" if arity(self, 1) => {
                let Some(x) = args.first() else {
                    return Ty::Error;
                };
                let other = match self.expr(x, None) {
                    Ty::Array(t) => (*t).clone(),
                    Ty::Tuple(ts) => Ty::union(ts),
                    Ty::Error => return Ty::Error,
                    t => {
                        self.err(
                            Code::TypeMismatch,
                            x.span,
                            format!("`concat` の引数は配列（`{t}`）"),
                        );
                        return Ty::Error;
                    }
                };
                let u = join(elem.clone(), other);
                if u.is_valid_union() {
                    Ty::Array(Box::new(u))
                } else {
                    self.err(
                        Code::TypeMismatch,
                        x.span,
                        format!("配列の要素の型がそろわない（`{u}`）"),
                    );
                    Ty::Error
                }
            }
            _ => Ty::Error,
        }
    }

    /// 組み込みメソッドに渡す関数。引数の型は文脈から決まる（DECISIONS A3）。戻り値の型を返す。
    fn callback(&mut self, cb: &Expr, params: &[Ty], ret_context: Option<&Ty>) -> Ty {
        if let ExprKind::Arrow {
            params: ps,
            ret,
            body,
        } = &cb.kind
        {
            if ps.len() != params.len() {
                self.err(
                    Code::NotCallable,
                    cb.span,
                    format!("この関数の引数は {} 個", params.len()),
                );
                return Ty::Error;
            }
            let mut scope = BTreeMap::new();
            for (p, want) in ps.iter().zip(params) {
                let ty = match &p.ty {
                    Some(t) => {
                        let t = self.ty(t);
                        if !assignable(want, &t) {
                            self.err(
                                Code::TypeMismatch,
                                p.span,
                                format!("引数の型は `{want}`（`{t}` と書いてある）"),
                            );
                        }
                        t
                    }
                    None if matches!(p.pattern, ParamPattern::Object(..)) => {
                        self.err(
                            Code::AnnotationRequired,
                            param_span(p),
                            "分割代入の引数には型注釈が要る",
                        );
                        Ty::Error
                    }
                    None => want.clone(),
                };
                self.bind_param(p, &ty, &mut scope);
            }
            self.scopes.push(scope);
            let r = self.arrow_body(ret.as_ref(), body, ret_context);
            self.scopes.pop();
            return r;
        }
        match self.expr(cb, None) {
            Ty::Function(ps, r)
                if ps.len() == params.len()
                    && params.iter().zip(&ps).all(|(a, p)| assignable(a, p)) =>
            {
                *r
            }
            Ty::Error => Ty::Error,
            t => {
                let want: Vec<String> = params.iter().map(Ty::to_string).collect();
                self.err(
                    Code::TypeMismatch,
                    cb.span,
                    format!(
                        "引数 ({}) をとる関数が要るところに `{t}` がある",
                        want.join(", ")
                    ),
                );
                Ty::Error
            }
        }
    }

    /// アロー関数の本体。戻り値の注釈がなければ推論する。
    fn arrow_body(&mut self, ret: Option<&Type>, body: &ArrowBody, context: Option<&Ty>) -> Ty {
        let declared = ret.map(|t| self.ty(t));
        let want = declared.as_ref();
        match body {
            ArrowBody::Expr(x) => match want {
                Some(w) => {
                    self.check_against(x, w, Code::TypeMismatch);
                    w.clone()
                }
                None => self.expr(x, context),
            },
            ArrowBody::Block(b) => {
                let mut returns = Vec::new();
                self.block(b, want, &mut returns);
                match declared {
                    Some(w) => w,
                    None => {
                        let u = returns.into_iter().reduce(join).unwrap_or(Ty::Error);
                        if u.is_valid_union() {
                            u
                        } else {
                            self.err(
                                Code::TypeMismatch,
                                b.span,
                                format!("戻り値の型がそろわない（`{u}`）"),
                            );
                            Ty::Error
                        }
                    }
                }
            }
        }
    }

    fn arrow(
        &mut self,
        params: &[Param],
        ret: Option<&Type>,
        body: &ArrowBody,
        expected: Option<&Ty>,
    ) -> Ty {
        let context = expected.and_then(|t| {
            t.members()
                .iter()
                .find(|m| matches!(m, Ty::Function(..)))
                .cloned()
        });
        let (want_params, want_ret) = match context {
            Some(Ty::Function(ps, r)) if ps.len() == params.len() => (Some(ps), Some(*r)),
            _ => (None, None),
        };
        let mut scope = BTreeMap::new();
        let mut tys = Vec::new();
        // 文脈の型と違う注釈は、ここで報告して全体を Error にする（外側で重ねて報告しない）。
        let mut mismatch = false;
        for (i, p) in params.iter().enumerate() {
            let want = want_params.as_ref().and_then(|ps| ps.get(i));
            let ty = match (&p.ty, want) {
                (Some(t), want) => {
                    let t = self.ty(t);
                    if let Some(w) = want
                        && *w != t
                        && t != Ty::Error
                    {
                        self.err(
                            Code::TypeMismatch,
                            p.span,
                            format!("引数の型は `{w}`（`{t}` と書いてある）"),
                        );
                        mismatch = true;
                    }
                    t
                }
                // 分割代入の引数は、文脈から型を推論しない（注釈が必須。DECISIONS R4）。
                (None, Some(_)) if matches!(p.pattern, ParamPattern::Object(..)) => {
                    self.err(
                        Code::AnnotationRequired,
                        param_span(p),
                        "分割代入の引数には型注釈が要る",
                    );
                    Ty::Error
                }
                (None, Some(w)) => w.clone(),
                (None, None) => {
                    self.err(
                        Code::AnnotationRequired,
                        param_span(p),
                        format!("引数 {} には型注釈が要る", param_label(p)),
                    );
                    Ty::Error
                }
            };
            self.bind_param(p, &ty, &mut scope);
            tys.push(ty);
        }
        self.scopes.push(scope);
        let r = match (ret, &want_ret) {
            (None, Some(w)) => {
                match body {
                    ArrowBody::Expr(x) => {
                        self.check_against(x, w, Code::TypeMismatch);
                    }
                    ArrowBody::Block(b) => self.block(b, Some(w), &mut Vec::new()),
                }
                w.clone()
            }
            _ => self.arrow_body(ret, body, None),
        };
        self.scopes.pop();
        if mismatch {
            return Ty::Error;
        }
        Ty::Function(tys, Box::new(r))
    }

    // ---- JSX -----------------------------------------------------------------

    fn element(&mut self, el: &Element) -> Ty {
        let name = el.name.name.as_str();
        match self.values.get(name).map(|e| e.decl) {
            Some(ValueDecl::HostComponent(c)) => {
                self.host_element(el, c);
                Ty::element(name)
            }
            Some(ValueDecl::Function) if is_component_name(name) => {
                let (params, ret) = self
                    .signatures
                    .get(name)
                    .cloned()
                    .unwrap_or((Vec::new(), Ty::Error));
                let props = params.first().cloned().unwrap_or(Ty::Error);
                self.user_element(el, &props, name);
                ret
            }
            Some(ValueDecl::Imported(ExportedValue::Function(params, ret)))
                if is_component_name(name) =>
            {
                let props = params.first().cloned().unwrap_or(Ty::Error);
                self.user_element(el, &props, name);
                ret.clone()
            }
            _ => {
                self.err(
                    Code::NotAComponent,
                    el.name.span,
                    format!("`{name}` はコンポーネントではない"),
                );
                self.attrs(el, None);
                self.children(&el.children);
                Ty::Error
            }
        }
    }

    /// 属性を集める。`target` があれば、各値をその型の文脈で検査する。
    /// 戻り値は、名前 → (与えたもの, 誤りを示す位置)。
    fn attrs(
        &mut self,
        el: &Element,
        target: Option<&BTreeMap<String, Field>>,
    ) -> BTreeMap<String, (Provided, Span)> {
        let mut out: BTreeMap<String, (Provided, Span)> = BTreeMap::new();
        let mut explicit = BTreeSet::new();
        for a in &el.attrs {
            match a {
                Attr::Named { name, value, .. } => {
                    if !explicit.insert(name.name.clone()) {
                        self.err(
                            Code::DuplicateKey,
                            name.span,
                            format!("属性 `{}` が重複している", name.name),
                        );
                        continue;
                    }
                    // `key` は props ではない（DECISIONS R3）。値の型だけ検査して、props には入れない。
                    if name.name == "key" {
                        self.check_key(value.as_ref(), name.span);
                        continue;
                    }
                    let want = target.and_then(|t| t.get(&name.name)).map(Field::effective);
                    let (ty, at) = match value {
                        None => (Ty::Literal(Literal::Bool(true)), name.span),
                        Some(AttrValue::String(s)) => {
                            (Ty::Literal(Literal::String(s.value.clone())), s.span)
                        }
                        Some(AttrValue::Expr(x)) => (self.expr(x, want.as_ref()), x.span),
                    };
                    let prev = out.remove(&name.name).map(|(p, _)| p);
                    let mut fields: BTreeMap<String, Provided> =
                        prev.into_iter().map(|p| (name.name.clone(), p)).collect();
                    Self::provide(
                        &mut fields,
                        &name.name,
                        Provided {
                            ty,
                            maybe_absent: false,
                        },
                    );
                    if let Some(p) = fields.remove(&name.name) {
                        out.insert(name.name.clone(), (p, at));
                    }
                }
                Attr::Spread(x, sspan) => {
                    let got = self.expr(x, None);
                    match Self::spread_fields(&got) {
                        Some(fs) => {
                            for (k, pf) in fs {
                                explicit.remove(&k);
                                let prev = out.remove(&k).map(|(p, _)| p);
                                let mut fields: BTreeMap<String, Provided> =
                                    prev.into_iter().map(|p| (k.clone(), p)).collect();
                                Self::provide(&mut fields, &k, pf);
                                if let Some(p) = fields.remove(&k) {
                                    out.insert(k, (p, *sspan));
                                }
                            }
                        }
                        None => self.err(
                            Code::TypeMismatch,
                            *sspan,
                            format!("`{got}` はオブジェクトでないので展開できない"),
                        ),
                    }
                }
            }
        }
        out
    }

    /// `key` 属性の値は string か number（DECISIONS R3）。
    fn check_key(&mut self, value: Option<&AttrValue>, name_span: Span) {
        let (ty, at) = match value {
            None => (Ty::Literal(Literal::Bool(true)), name_span),
            Some(AttrValue::String(_)) => return,
            Some(AttrValue::Expr(x)) => (self.expr(x, None), x.span),
        };
        let is_number = |m: &Ty| matches!(m, Ty::Number | Ty::Literal(Literal::Number(_)));
        let ok = ty
            .members()
            .iter()
            .all(|m| matches!(m, Ty::String | Ty::Error | Ty::Literal(Literal::String(_))));
        if ok {
            return;
        }
        if ty.members().iter().any(is_number) {
            // 移行のメッセージ（DECISIONS U3）。
            self.err_help(
                Code::KeyNotString,
                at,
                format!("`key` は string だけ（`{ty}` を渡している）"),
                "number は、テンプレート文字列で文字列にする（`key={`${n}`}`）",
            );
        } else {
            self.err(
                Code::TypeMismatch,
                at,
                format!("`key` は string（`{ty}` を渡している）"),
            );
        }
    }

    fn attr_name_span(el: &Element, name: &str, fallback: Span) -> Span {
        el.attrs
            .iter()
            .find_map(|a| match a {
                Attr::Named { name: n, .. } if n.name == name => Some(n.span),
                _ => None,
            })
            .unwrap_or(fallback)
    }

    fn host_element(&mut self, el: &Element, c: &Component) {
        let provided = self.attrs(el, Some(&c.props));
        for (k, (p, at)) in &provided {
            match c.props.get(k) {
                None => {
                    let span = Self::attr_name_span(el, k, *at);
                    let message = if k == "key" {
                        KEY_IN_SPREAD.to_owned()
                    } else {
                        format!("`<{}>` に props `{k}` はない", el.name.name)
                    };
                    self.err(Code::UnknownHostProp, span, message);
                }
                Some(f) if !assignable(&p.ty, &f.effective()) => {
                    let code = if is_slot_mismatch(&f.effective(), &p.ty) {
                        Code::SlotType
                    } else {
                        Code::HostPropType
                    };
                    self.err(
                        code,
                        *at,
                        format!("`{k}` は `{}`（`{}` を渡している）", f.effective(), p.ty),
                    );
                }
                Some(_) => {}
            }
        }
        let missing: Vec<String> = c
            .props
            .iter()
            .filter(|(k, f)| !f.optional && provided.get(*k).is_none_or(|(p, _)| p.maybe_absent))
            .map(|(k, _)| format!("`{k}`"))
            .collect();
        if !missing.is_empty() {
            self.err(
                Code::MissingHostProp,
                el.name.span,
                format!(
                    "`<{}>` に必須の props {} がない",
                    el.name.name,
                    missing.join("・")
                ),
            );
        }
        let infos = self.children(&el.children);
        // 文字列リテラルの key の重複は、評価の前に、すべて報告する（DECISIONS U3）。
        let mut keys: BTreeMap<&str, Span> = BTreeMap::new();
        for info in &infos {
            let Some(key) = info.key.as_deref() else {
                continue;
            };
            match keys.get(key) {
                Some(first) => {
                    let first = *first;
                    self.err_label(
                        Code::DuplicateChildKey,
                        info.span,
                        format!("同じ親の子に、同じ key `{key}` がある"),
                        first,
                        "最初の子",
                    );
                }
                None => {
                    keys.insert(key, info.span);
                }
            }
        }
        let mut first_seen: BTreeMap<String, Span> = BTreeMap::new();
        for info in &infos {
            if info.text && !c.accepts_text() {
                self.err_help(
                    Code::TextChildNotAccepted,
                    info.span,
                    format!("`<{}>` はテキスト子を受け付けない", el.name.name),
                    "子に string を置けるのは、ホストが宣言したコンポーネントだけ",
                );
                continue;
            }
            let allowed = info
                .names
                .as_ref()
                .is_some_and(|n| n.is_subset(&c.children));
            if !allowed {
                self.err(
                    Code::HostChildType,
                    info.span,
                    format!("`<{}>` の子に `{}` は置けない", el.name.name, info.ty),
                );
                continue;
            }
            if let Some(single) = &info.single
                && c.unique.contains(single)
            {
                if let Some(first) = first_seen.get(single) {
                    let first = *first;
                    self.err_label(
                        Code::DuplicateUniqueChild,
                        info.span,
                        format!("`<{single}>` は `<{}>` の子に 1 つだけ置ける", el.name.name),
                        first,
                        "最初の子",
                    );
                } else {
                    first_seen.insert(single.clone(), info.span);
                }
            }
        }
    }

    fn user_element(&mut self, el: &Element, props: &Ty, component: &str) {
        let target = match props {
            Ty::Object(fields) => Some(fields.clone()),
            _ => None,
        };
        let mut provided = self.attrs(el, target.as_ref());
        let infos = self.children(&el.children);
        let mut text_span: Option<Span> = None;
        if !infos.is_empty() {
            if provided.contains_key("children") {
                self.err(
                    Code::ChildrenTwice,
                    el.name.span,
                    "`children` 属性と子を両方は書けない",
                );
            }
            let mut names = BTreeSet::new();
            for info in &infos {
                match &info.names {
                    Some(n) => names.extend(n.iter().cloned()),
                    None => self.err(
                        Code::TypeMismatch,
                        info.span,
                        format!("`{}` は要素の子に置けない", info.ty),
                    ),
                }
            }
            text_span = infos.iter().find(|i| i.text).map(|i| i.span);
            let span = infos.first().map_or(el.name.span, |i| i.span);
            let children_ty = match (text_span.is_some(), names.is_empty()) {
                (true, true) => Ty::Array(Box::new(Ty::String)),
                (true, false) => {
                    Ty::Array(Box::new(Ty::union(vec![Ty::Element(names), Ty::String])))
                }
                (false, _) => Ty::Array(Box::new(Ty::Element(names))),
            };
            provided.insert(
                "children".to_owned(),
                (
                    Provided {
                        ty: children_ty,
                        maybe_absent: false,
                    },
                    span,
                ),
            );
        }
        let Some(target) = target else {
            return;
        };
        for (k, (p, at)) in &provided {
            match target.get(k) {
                None if k == "children" && text_span.is_some() => {
                    self.err(
                        Code::TextChildNotAccepted,
                        text_span.unwrap_or(*at),
                        format!("`<{component}>` はテキスト子を受け付けない（props に `children` がない）"),
                    );
                }
                None => {
                    let span = Self::attr_name_span(el, k, *at);
                    let message = if k == "key" {
                        KEY_IN_SPREAD.to_owned()
                    } else {
                        format!("`<{component}>` の props に `{k}` はない")
                    };
                    self.err(Code::UnknownProperty, span, message);
                }
                Some(fd) if !assignable(&p.ty, &fd.effective()) => {
                    if k == "children" && text_span.is_some() {
                        self.err(
                            Code::TextChildNotAccepted,
                            text_span.unwrap_or(*at),
                            format!(
                                "`<{component}>` の `children` は `{}` で、テキスト子は置けない",
                                fd.effective()
                            ),
                        );
                    } else {
                        let code = if k != "children" && is_slot_mismatch(&fd.effective(), &p.ty) {
                            Code::SlotType
                        } else {
                            Code::TypeMismatch
                        };
                        self.err(
                            code,
                            *at,
                            format!("`{k}` は `{}`（`{}` を渡している）", fd.effective(), p.ty),
                        );
                    }
                }
                Some(_) => {}
            }
        }
        let missing: Vec<String> = target
            .iter()
            .filter(|(k, fd)| !fd.optional && provided.get(*k).is_none_or(|(p, _)| p.maybe_absent))
            .map(|(k, _)| format!("`{k}`"))
            .collect();
        if !missing.is_empty() {
            self.err(
                Code::MissingProperty,
                el.name.span,
                format!(
                    "`<{}>` に必須の props {} がない",
                    component,
                    missing.join("・")
                ),
            );
        }
    }

    /// 子を平らにして、それぞれの型を調べる（DECISIONS B6）。
    fn children(&mut self, children: &[Child]) -> Vec<ChildInfo> {
        let mut out = Vec::new();
        for c in children {
            match c {
                Child::Element(el, span) => {
                    let ty = self.element(el);
                    let single = match &ty {
                        Ty::Element(names) if names.len() == 1 => names.first().cloned(),
                        _ => None,
                    };
                    let kinds = child_kinds(&ty);
                    out.push(ChildInfo {
                        span: *span,
                        names: kinds.as_ref().map(|k| k.names.clone()),
                        single,
                        text: false,
                        key: literal_key(el),
                        ty,
                    });
                }
                Child::Fragment(cs, _) => out.extend(self.children(cs)),
                Child::Expr(x, span) => {
                    let ty = self.expr(x, None);
                    let kinds = child_kinds(&ty);
                    out.push(ChildInfo {
                        span: *span,
                        names: kinds.as_ref().map(|k| k.names.clone()),
                        single: None,
                        text: kinds.as_ref().is_some_and(|k| k.text),
                        key: None,
                        ty,
                    });
                }
                Child::Text(_, span) => out.push(ChildInfo {
                    span: *span,
                    names: Some(BTreeSet::new()),
                    single: None,
                    text: true,
                    key: None,
                    ty: Ty::String,
                }),
            }
        }
        out
    }
}

fn param_span(p: &Param) -> Span {
    match &p.pattern {
        ParamPattern::Ident(id) => id.span,
        ParamPattern::Object(_, span) => *span,
    }
}

fn param_label(p: &Param) -> String {
    match &p.pattern {
        ParamPattern::Ident(id) => format!("`{}`", id.name),
        ParamPattern::Object(..) => "（分割代入）".to_owned(),
    }
}

/// 要素の `key` が文字列リテラル（`key="a"` / `key={"a"}`）なら、その値。
fn literal_key(el: &Element) -> Option<String> {
    el.attrs.iter().find_map(|a| match a {
        Attr::Named {
            name,
            value: Some(v),
            ..
        } if name.name == "key" => match v {
            AttrValue::String(s) => Some(s.value.clone()),
            AttrValue::Expr(Expr {
                kind: ExprKind::String(s),
                ..
            }) => Some(s.clone()),
            AttrValue::Expr(_) => None,
        },
        _ => None,
    })
}

/// 型に要素（`Element<…>`、またはその配列・ユニオン）が含まれるか。
fn contains_element(t: &Ty) -> bool {
    match t {
        Ty::Element(_) => true,
        Ty::Array(x) => contains_element(x),
        Ty::Union(ms) | Ty::Tuple(ms) => ms.iter().any(contains_element),
        _ => false,
    }
}

/// スロット（要素を渡す props）に、型の違う要素を渡したか（DECISIONS U2）。
fn is_slot_mismatch(want: &Ty, got: &Ty) -> bool {
    contains_element(want) && contains_element(got)
}

/// 2 つの型を合わせる。片方がもう片方に置けるなら広いほう、そうでなければユニオン。
fn join(a: Ty, b: Ty) -> Ty {
    if a == Ty::Error || b == Ty::Error {
        Ty::Error
    } else if assignable(&a, &b) {
        b
    } else if assignable(&b, &a) {
        a
    } else {
        Ty::union(vec![a, b])
    }
}

/// 子に置ける型の中身。
struct ChildKinds {
    /// 現れうる組み込みコンポーネント。
    names: BTreeSet<String>,
    /// テキスト（string）が現れうる。
    text: bool,
}

/// 子として置ける型なら、その中身。`false` と `undefined` は、どの子の位置にも置けて、
/// 評価結果から取り除かれる（DECISIONS R2）。`true` と `boolean` は置けない。
fn child_kinds(t: &Ty) -> Option<ChildKinds> {
    let nothing = || {
        Some(ChildKinds {
            names: BTreeSet::new(),
            text: false,
        })
    };
    match t {
        Ty::Element(names) => Some(ChildKinds {
            names: names.clone(),
            text: false,
        }),
        Ty::Undefined | Ty::Error | Ty::Literal(Literal::Bool(false)) => nothing(),
        Ty::String | Ty::Literal(Literal::String(_)) => Some(ChildKinds {
            names: BTreeSet::new(),
            text: true,
        }),
        Ty::Array(t) => child_kinds(t),
        Ty::Tuple(ts) | Ty::Union(ts) => {
            let mut out = ChildKinds {
                names: BTreeSet::new(),
                text: false,
            };
            for t in ts {
                let k = child_kinds(t)?;
                out.names.extend(k.names);
                out.text |= k.text;
            }
            Some(out)
        }
        _ => None,
    }
}

/// 絞り込みに使うパス（`a` や `a.b.c`）。
fn path_key(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Member {
            object,
            prop,
            optional: false,
        } => Some(format!("{}.{}", path_key(object)?, prop.name)),
        _ => None,
    }
}

fn lit(l: &Lit) -> Literal {
    match l {
        Lit::Number(n, _) => Literal::number(*n),
        Lit::String(s, _) => Literal::String(s.clone()),
        Lit::Bool(b, _) => Literal::Bool(*b),
    }
}

fn lit_span(l: &Lit) -> Span {
    match l {
        Lit::Number(_, s) | Lit::String(_, s) | Lit::Bool(_, s) => *s,
    }
}

fn type_span(t: &Type) -> Span {
    match t {
        Type::Boolean(s)
        | Type::Number(s)
        | Type::String(s)
        | Type::Undefined(s)
        | Type::Array(_, s)
        | Type::Tuple(_, s)
        | Type::Object(_, s)
        | Type::Union(_, s) => *s,
        Type::Literal(l) => lit_span(l),
        Type::Named { span, .. } | Type::Function { span, .. } => *span,
    }
}

fn binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "===",
        BinaryOp::Ne => "!==",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
    }
}
