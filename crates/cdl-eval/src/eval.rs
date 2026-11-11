//! 評価器（SPEC 第 7・8 章）。
//!
//! 型検査を通った AST を評価する。評価は純粋で、入力はソースとホストから渡された値だけ。
//! 誤りは最初の 1 つで打ち切る（DECISIONS D9）。

use std::collections::BTreeMap;
use std::sync::Arc;

use cdl_diag::Code;
use cdl_sema::schema::Schema;
use cdl_sema::syntax::ast::*;

use crate::Limits;
use crate::budget::Budget;
use crate::num::{self, f64_to_i64, js_number_string, normalize};
use crate::value::Element as Node;
use crate::value::{FnRepr, Function, Scope, Tree, Value, count_nodes, depth_exceeds};

/// 評価の誤り。`span` が `None` なら、評価していたトップレベルの宣言の位置にする（DECISIONS D8）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EvalError {
    pub code: Code,
    pub span: Option<Span>,
    /// 誤りが起きたファイル（ユニットの番号）。`None` なら、評価していた宣言のファイル。
    pub unit: Option<usize>,
    pub message: String,
}

type R<T> = Result<T, EvalError>;

fn at(code: Code, span: Span, message: impl Into<String>) -> EvalError {
    EvalError {
        code,
        span: Some(span),
        unit: None,
        message: message.into(),
    }
}

fn limit(code: Code, message: impl Into<String>) -> EvalError {
    EvalError {
        code,
        span: None,
        unit: None,
        message: message.into(),
    }
}

/// 1 つのファイルの評価の状態。
pub(crate) struct UnitState<'a> {
    pub module: &'a Module,
    /// このファイルの中のアロー関数（決まった順に番号を振る）。
    arrows: Vec<&'a Expr>,
    arrow_ids: BTreeMap<*const Expr, usize>,
    functions: BTreeMap<String, &'a FunctionDecl>,
    /// import した名前 → そのファイルの番号。
    imports: BTreeMap<String, usize>,
    pub globals: BTreeMap<String, Value>,
}

/// 型検査で防がれているはずの状態（DECISIONS D10）。
fn internal(span: Span, what: &str) -> EvalError {
    at(Code::Internal, span, format!("内部エラー: {what}"))
}

pub(crate) struct Evaluator<'a> {
    pub(crate) units: Vec<UnitState<'a>>,
    /// いま評価しているファイル。
    current: usize,
    schema: &'a Schema,
    limits: Limits,
    budget: Budget,
    depth: u32,
}

impl<'a> UnitState<'a> {
    /// `deps` は import の指定子 → そのファイルの番号。
    pub(crate) fn new(module: &'a Module, deps: &BTreeMap<String, usize>) -> Self {
        let mut u = UnitState {
            module,
            arrows: Vec::new(),
            arrow_ids: BTreeMap::new(),
            functions: BTreeMap::new(),
            imports: BTreeMap::new(),
            globals: BTreeMap::new(),
        };
        for item in &module.items {
            match &item.kind {
                ItemKind::Const(c) => u.collect_expr(&c.init),
                ItemKind::Function(f) => {
                    u.functions.insert(f.name.name.clone(), f);
                    u.collect_block(&f.body);
                }
                ItemKind::ExportDefault(x) => u.collect_expr(x),
                ItemKind::Import(i) => {
                    if let Some(&t) = deps.get(&i.source.value) {
                        for n in &i.names {
                            u.imports.insert(n.name.clone(), t);
                        }
                    }
                }
                ItemKind::TypeAlias(_) | ItemKind::Interface(_) => {}
            }
        }
        u
    }

    // ---- アロー関数に番号を振る ------------------------------------------------

    fn collect_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Undefined
            | ExprKind::Ident(_) => {}
            ExprKind::Template { exprs, .. } => exprs.iter().for_each(|x| self.collect_expr(x)),
            ExprKind::Array(elems) => {
                for el in elems {
                    match el {
                        ArrayElem::Expr(x) | ArrayElem::Spread(x, _) => self.collect_expr(x),
                    }
                }
            }
            ExprKind::Object(props) => {
                for p in props {
                    match p {
                        ObjProp::KeyValue { value, .. } => self.collect_expr(value),
                        ObjProp::Spread(x, _) => self.collect_expr(x),
                        ObjProp::Shorthand(_) => {}
                    }
                }
            }
            ExprKind::Unary { expr, .. } => self.collect_expr(expr),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Logical { lhs, rhs, .. } => {
                self.collect_expr(lhs);
                self.collect_expr(rhs);
            }
            ExprKind::Conditional { test, cons, alt } => {
                self.collect_expr(test);
                self.collect_expr(cons);
                self.collect_expr(alt);
            }
            ExprKind::Member { object, .. } => self.collect_expr(object),
            ExprKind::Index { object, index } => {
                self.collect_expr(object);
                self.collect_expr(index);
            }
            ExprKind::Call { callee, args } => {
                self.collect_expr(callee);
                args.iter().for_each(|a| self.collect_expr(a));
            }
            ExprKind::Arrow { body, .. } => {
                self.arrow_ids
                    .insert(std::ptr::from_ref(e), self.arrows.len());
                self.arrows.push(e);
                match body {
                    ArrowBody::Expr(x) => self.collect_expr(x),
                    ArrowBody::Block(b) => self.collect_block(b),
                }
            }
            ExprKind::Satisfies { expr, .. } => self.collect_expr(expr),
            ExprKind::Element(el) => self.collect_element(el),
            ExprKind::Fragment(children) => self.collect_children(children),
        }
    }

    fn collect_element(&mut self, el: &'a Element) {
        for a in &el.attrs {
            match a {
                Attr::Named {
                    value: Some(AttrValue::Expr(x)),
                    ..
                }
                | Attr::Spread(x, _) => self.collect_expr(x),
                Attr::Named { .. } => {}
            }
        }
        self.collect_children(&el.children);
    }

    fn collect_children(&mut self, children: &'a [Child]) {
        for c in children {
            match c {
                Child::Element(el, _) => self.collect_element(el),
                Child::Fragment(cs, _) => self.collect_children(cs),
                Child::Expr(x, _) => self.collect_expr(x),
            }
        }
    }

    fn collect_block(&mut self, b: &'a Block) {
        for c in &b.consts {
            self.collect_expr(&c.init);
        }
        self.collect_stmt(&b.stmt);
    }

    fn collect_stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Return(x, _) => self.collect_expr(x),
            Stmt::If {
                cond,
                then,
                otherwise,
                ..
            } => {
                self.collect_expr(cond);
                self.collect_block(then);
                match otherwise {
                    Else::Block(b) => self.collect_block(b),
                    Else::If(s) => self.collect_stmt(s),
                }
            }
            Stmt::Switch {
                scrutinee,
                cases,
                default,
                ..
            } => {
                self.collect_expr(scrutinee);
                for c in cases {
                    self.collect_stmt(&c.body);
                }
                if let Some(d) = default {
                    self.collect_stmt(d);
                }
            }
        }
    }
}

impl<'a> Evaluator<'a> {
    pub(crate) fn new(
        units: Vec<UnitState<'a>>,
        schema: &'a Schema,
        limits: Limits,
        steps: u64,
    ) -> Self {
        Evaluator {
            units,
            current: 0,
            schema,
            limits,
            budget: Budget::new(steps),
            depth: 0,
        }
    }

    pub(crate) fn reset_budget(&mut self, steps: u64) {
        self.budget = Budget::new(steps);
        self.depth = 0;
    }

    // ---- トップレベル --------------------------------------------------------

    /// ファイル `unit` のトップレベルの `const` を、依存先から順に評価する（DECISIONS C7）。
    pub(crate) fn globals(&mut self, unit: usize, order: &[String]) -> R<()> {
        let Some(module) = self.units.get(unit).map(|u| u.module) else {
            return Ok(());
        };
        let consts: BTreeMap<&str, &'a ConstDecl> = module
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::Const(c) => Some((c.name.name.as_str(), c)),
                _ => None,
            })
            .collect();
        let root = Arc::new(Scope::default());
        for name in order {
            let Some(c) = consts.get(name.as_str()).copied() else {
                continue;
            };
            self.current = unit;
            let v = self
                .expr(&c.init, &root)
                .map_err(|e| fill(e, c.name.span, unit))?;
            if let Some(u) = self.units.get_mut(unit) {
                u.globals.insert(name.clone(), v);
            }
        }
        Ok(())
    }

    /// エントリファイルの `export default` を評価して要素ツリーにする。
    pub(crate) fn document(&mut self, entry: usize) -> R<Arc<Node>> {
        self.current = entry;
        let root = self.units.get(entry).and_then(|u| {
            u.module.items.iter().find_map(|i| match &i.kind {
                ItemKind::ExportDefault(e) => Some(e),
                _ => None,
            })
        });
        let Some(root) = root else {
            return Err(fill(
                at(
                    Code::RootElement,
                    Span::default(),
                    "`export default` がない",
                ),
                Span::default(),
                entry,
            ));
        };
        let scope = Arc::new(Scope::default());
        let v = self
            .expr(root, &scope)
            .map_err(|e| fill(e, root.span, entry))?;
        let Value::Element(el) = v else {
            return Err(fill(
                internal(root.span, "ルートが要素でない"),
                root.span,
                entry,
            ));
        };
        if count_nodes(&el, self.limits.max_nodes) > self.limits.max_nodes {
            let e = at(
                Code::NodeLimit,
                root.span,
                format!("値木のノード数が上限 {} を超えた", self.limits.max_nodes),
            );
            return Err(fill(e, root.span, entry));
        }
        Ok(el)
    }

    /// フックを呼ぶ。`f` は `policy` のメンバーの関数。
    pub(crate) fn call(&mut self, f: &Function, args: Vec<Value>, span: Span) -> R<Value> {
        self.call_value(f, args, span)
    }

    fn unit(&self) -> Option<&UnitState<'a>> {
        self.units.get(self.current)
    }

    /// いま評価しているファイルの名前を付けた位置。
    fn file_span(&self, span: Span) -> cdl_diag::Span {
        cdl_diag::Span {
            file: self
                .unit()
                .map(|u| u.module.file.clone())
                .unwrap_or_default(),
            start: span.start,
            end: span.end,
        }
    }

    /// 名前が関数なら、その関数の値（このファイルの宣言か、import したもの）。
    fn function_named(&self, name: &str) -> Option<Function> {
        let u = self.unit()?;
        if u.functions.contains_key(name) {
            return Some(Function(FnRepr::Named {
                unit: self.current,
                name: name.to_owned(),
            }));
        }
        let t = *u.imports.get(name)?;
        let found = self.units.get(t)?.functions.contains_key(name);
        found.then(|| {
            Function(FnRepr::Named {
                unit: t,
                name: name.to_owned(),
            })
        })
    }

    /// 名前がこのファイルで値を持つか（持たなければ組み込み関数）。
    fn is_bound(&self, name: &str, scope: &Scope) -> bool {
        scope.lookup(name).is_some()
            || self.unit().is_some_and(|u| {
                u.globals.contains_key(name)
                    || u.functions.contains_key(name)
                    || u.imports.contains_key(name)
            })
    }

    // ---- 上限 ----------------------------------------------------------------

    fn step(&mut self) -> R<()> {
        self.charge(1)
    }

    fn charge(&mut self, n: u64) -> R<()> {
        self.budget
            .charge(n)
            .map_err(|_| limit(Code::StepLimit, "評価ステップの上限を超えた"))
    }

    /// 値木の深さが上限を超えていないか（DECISIONS D11）。たどった値ごとにステップを使う（D13）。
    fn check_depth(&mut self, tree: Tree<'_>) -> R<()> {
        match depth_exceeds(tree, self.limits.max_value_depth, Some(&mut self.budget)) {
            Ok(false) => Ok(()),
            Ok(true) => Err(limit(
                Code::ValueDepth,
                format!("値木の深さが上限 {} を超えた", self.limits.max_value_depth),
            )),
            Err(_) => Err(limit(Code::StepLimit, "評価ステップの上限を超えた")),
        }
    }

    /// 新しく作った値の深さを検査して、そのまま返す。
    fn bounded(&mut self, v: Value) -> R<Value> {
        self.check_depth(Tree::Value(&v))?;
        Ok(v)
    }

    fn check_len(&self, n: usize) -> R<()> {
        if u64::try_from(n).unwrap_or(u64::MAX) > u64::from(self.limits.max_collection_len) {
            Err(limit(
                Code::CollectionLimit,
                format!(
                    "配列・マップの要素数が上限 {} を超えた",
                    self.limits.max_collection_len
                ),
            ))
        } else {
            Ok(())
        }
    }

    fn string(&self, s: String) -> R<Value> {
        if u64::try_from(s.len()).unwrap_or(u64::MAX) > u64::from(self.limits.max_string_bytes) {
            return Err(limit(
                Code::StringLimit,
                format!(
                    "文字列の長さが上限 {} バイトを超えた",
                    self.limits.max_string_bytes
                ),
            ));
        }
        Ok(Value::String(Arc::from(s)))
    }

    // ---- 式 ------------------------------------------------------------------

    fn expr(&mut self, e: &'a Expr, scope: &Arc<Scope>) -> R<Value> {
        self.step()?;
        let span = e.span;
        match &e.kind {
            ExprKind::Number(n) => Ok(Value::Number(normalize(*n))),
            ExprKind::String(s) => self.string(s.clone()),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Undefined => Ok(Value::Undefined),
            ExprKind::Template { quasis, exprs } => {
                let mut out = String::new();
                for (i, q) in quasis.iter().enumerate() {
                    out.push_str(q);
                    if let Some(x) = exprs.get(i) {
                        match self.expr(x, scope)? {
                            Value::String(s) => out.push_str(&s),
                            Value::Number(n) => out.push_str(&js_number_string(n)),
                            _ => return Err(internal(x.span, "補間できない値")),
                        }
                    }
                }
                self.string(out)
            }
            ExprKind::Ident(name) => self.ident(name, scope, span),
            ExprKind::Array(elems) => {
                let mut out = Vec::new();
                for el in elems {
                    match el {
                        ArrayElem::Expr(x) => out.push(self.expr(x, scope)?),
                        ArrayElem::Spread(x, s) => match self.expr(x, scope)? {
                            Value::Array(xs) => {
                                self.charge(u64::try_from(xs.len()).unwrap_or(u64::MAX))?;
                                out.extend(xs.iter().cloned());
                            }
                            _ => return Err(internal(*s, "配列でない値の展開")),
                        },
                    }
                    self.check_len(out.len())?;
                }
                self.bounded(Value::array(out))
            }
            ExprKind::Object(props) => {
                let mut map = BTreeMap::new();
                for p in props {
                    match p {
                        ObjProp::KeyValue { key, value, .. } => {
                            let v = self.expr(value, scope)?;
                            set_field(&mut map, &key.name, v);
                        }
                        ObjProp::Shorthand(k) => {
                            let v = self.ident(&k.name, scope, k.span)?;
                            set_field(&mut map, &k.name, v);
                        }
                        ObjProp::Spread(x, s) => match self.expr(x, scope)? {
                            Value::Object(m) => {
                                self.charge(u64::try_from(m.len()).unwrap_or(u64::MAX))?;
                                spread_into(&mut map, &m);
                            }
                            _ => return Err(internal(*s, "オブジェクトでない値の展開")),
                        },
                    }
                    self.check_len(map.len())?;
                }
                self.bounded(Value::Object(Arc::new(map)))
            }
            ExprKind::Unary { op, expr } => {
                let v = self.expr(expr, scope)?;
                match (op, v) {
                    (UnaryOp::Neg, Value::Number(n)) => Ok(Value::Number(normalize(-n))),
                    (UnaryOp::Neg, Value::Length(n)) => Ok(Value::Length(normalize(-n))),
                    (UnaryOp::Neg, Value::Duration(d)) => d
                        .checked_neg()
                        .map(Value::Duration)
                        .ok_or_else(|| at(Code::DurationRange, span, "Duration が範囲を超えた")),
                    (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    _ => Err(internal(span, "単項演算子のオペランド")),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.expr(lhs, scope)?;
                let b = self.expr(rhs, scope)?;
                binary(*op, a, b, span)
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let a = self.expr(lhs, scope)?;
                match (op, a) {
                    (LogicalOp::And, Value::Bool(false)) => Ok(Value::Bool(false)),
                    (LogicalOp::Or, Value::Bool(true)) => Ok(Value::Bool(true)),
                    (LogicalOp::And | LogicalOp::Or, Value::Bool(_)) => {
                        match self.expr(rhs, scope)? {
                            b @ Value::Bool(_) => Ok(b),
                            _ => Err(internal(rhs.span, "論理演算子のオペランド")),
                        }
                    }
                    (LogicalOp::Coalesce, Value::Undefined) => self.expr(rhs, scope),
                    (LogicalOp::Coalesce, v) => Ok(v),
                    _ => Err(internal(span, "論理演算子のオペランド")),
                }
            }
            ExprKind::Conditional { test, cons, alt } => match self.expr(test, scope)? {
                Value::Bool(true) => self.expr(cons, scope),
                Value::Bool(false) => self.expr(alt, scope),
                _ => Err(internal(test.span, "条件が boolean でない")),
            },
            ExprKind::Member {
                object,
                prop,
                optional,
            } => {
                let v = self.expr(object, scope)?;
                match v {
                    Value::Undefined if *optional => Ok(Value::Undefined),
                    Value::Object(m) => Ok(m.get(&prop.name).cloned().unwrap_or(Value::Undefined)),
                    Value::Array(xs) if prop.name == "length" => Ok(Value::Number(f64::from(
                        u32::try_from(xs.len()).unwrap_or(u32::MAX),
                    ))),
                    _ => Err(internal(prop.span, "プロパティを持たない値")),
                }
            }
            ExprKind::Index { object, index } => {
                let v = self.expr(object, scope)?;
                let i = self.expr(index, scope)?;
                match (v, i) {
                    (Value::Array(xs), Value::Number(n)) => Ok(f64_to_i64(n)
                        .and_then(|k| usize::try_from(k).ok())
                        .and_then(|k| xs.get(k).cloned())
                        .unwrap_or(Value::Undefined)),
                    (Value::Object(m), Value::String(k)) => {
                        Ok(m.get(&*k).cloned().unwrap_or(Value::Undefined))
                    }
                    _ => Err(internal(span, "添字で引けない値")),
                }
            }
            ExprKind::Call { callee, args } => self.call_expr(callee, args, scope, span),
            ExprKind::Arrow { .. } => {
                let id = self
                    .unit()
                    .and_then(|u| u.arrow_ids.get(&std::ptr::from_ref(e)))
                    .copied()
                    .ok_or_else(|| internal(span, "番号のないアロー関数"))?;
                Ok(Value::Function(Function(FnRepr::Closure {
                    unit: self.current,
                    arrow: id,
                    env: Arc::clone(scope),
                })))
            }
            ExprKind::Satisfies { expr, .. } => self.expr(expr, scope),
            ExprKind::Element(el) => self.element(el, scope).map(Value::Element),
            ExprKind::Fragment(children) => {
                let mut out = Vec::new();
                self.children(children, scope, &mut out)?;
                self.bounded(Value::array(out.into_iter().map(Value::Element).collect()))
            }
        }
    }

    fn ident(&mut self, name: &str, scope: &Arc<Scope>, span: Span) -> R<Value> {
        if let Some(v) = scope.lookup(name) {
            return Ok(v.clone());
        }
        if let Some(v) = self.unit().and_then(|u| u.globals.get(name)) {
            return Ok(v.clone());
        }
        if let Some(f) = self.function_named(name) {
            return Ok(Value::Function(f));
        }
        let imported = self
            .unit()
            .and_then(|u| u.imports.get(name))
            .and_then(|&t| self.units.get(t))
            .and_then(|u| u.globals.get(name));
        if let Some(v) = imported {
            return Ok(v.clone());
        }
        Err(internal(span, &format!("`{name}` の値がない")))
    }

    fn call_expr(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        scope: &Arc<Scope>,
        span: Span,
    ) -> R<Value> {
        if let ExprKind::Member {
            object,
            prop,
            optional: false,
        } = &callee.kind
            && matches!(
                prop.name.as_str(),
                "map" | "filter" | "reduce" | "includes" | "concat"
            )
        {
            let Value::Array(xs) = self.expr(object, scope)? else {
                return Err(internal(object.span, "配列でない値のメソッド"));
            };
            let mut vals = Vec::new();
            for a in args {
                vals.push(self.expr(a, scope)?);
            }
            return self.method(&prop.name, &xs, vals, span);
        }
        if let ExprKind::Ident(name) = &callee.kind
            && !self.is_bound(name, scope)
        {
            let mut vals = Vec::new();
            for a in args {
                vals.push(self.expr(a, scope)?);
            }
            return prelude(name, vals, span);
        }
        let f = self.expr(callee, scope)?;
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.expr(a, scope)?);
        }
        match f {
            Value::Function(f) => self.call_value(&f, vals, span),
            _ => Err(internal(callee.span, "関数でない値の呼び出し")),
        }
    }

    fn call_value(&mut self, f: &Function, args: Vec<Value>, span: Span) -> R<Value> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.limits.max_call_depth {
            return Err(limit(
                Code::CallDepth,
                format!(
                    "呼び出しの深さが上限 {} を超えた",
                    self.limits.max_call_depth
                ),
            ));
        }
        let caller = self.current;
        let callee = match &f.0 {
            FnRepr::Named { unit, .. } | FnRepr::Closure { unit, .. } => *unit,
        };
        self.current = callee;
        let result = match &f.0 {
            FnRepr::Named { name, .. } => {
                let decl = self
                    .unit()
                    .and_then(|u| u.functions.get(name))
                    .copied()
                    .ok_or_else(|| internal(span, "関数がない"))?;
                let vars = decl
                    .params
                    .iter()
                    .map(|p| p.name.name.clone())
                    .zip(args)
                    .collect();
                let scope = Arc::new(Scope { vars, parent: None });
                self.block(&decl.body, &scope)
            }
            FnRepr::Closure { arrow, env, .. } => {
                let Some(ExprKind::Arrow { params, body, .. }) = self
                    .unit()
                    .and_then(|u| u.arrows.get(*arrow))
                    .map(|e| &e.kind)
                else {
                    return Err(internal(span, "アロー関数がない"));
                };
                let vars = params
                    .iter()
                    .map(|p| p.name.name.clone())
                    .zip(args)
                    .collect();
                let scope = Arc::new(Scope {
                    vars,
                    parent: Some(Arc::clone(env)),
                });
                match body {
                    ArrowBody::Expr(x) => self.expr(x, &scope),
                    ArrowBody::Block(b) => self.block(b, &scope),
                }
            }
        };
        self.depth = self.depth.saturating_sub(1);
        self.current = caller;
        // 呼び出し先で起きた誤りは、呼び出し先のファイルの位置。上限の超過（位置なし）は
        // 宣言の位置にする（DECISIONS D8）ので、ここでは付けない。
        result.map_err(|mut e| {
            if e.span.is_some() && e.unit.is_none() {
                e.unit = Some(callee);
            }
            e
        })
    }

    fn method(&mut self, name: &str, xs: &[Value], args: Vec<Value>, span: Span) -> R<Value> {
        let mut args = args.into_iter();
        let fn_arg = |v: Option<Value>| match v {
            Some(Value::Function(f)) => Ok(f),
            _ => Err(internal(span, "関数でない引数")),
        };
        match name {
            "map" => {
                let f = fn_arg(args.next())?;
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    self.step()?;
                    out.push(self.call_value(&f, vec![x.clone()], span)?);
                }
                self.bounded(Value::array(out))
            }
            "filter" => {
                let f = fn_arg(args.next())?;
                let mut out = Vec::new();
                for x in xs {
                    self.step()?;
                    match self.call_value(&f, vec![x.clone()], span)? {
                        Value::Bool(true) => out.push(x.clone()),
                        Value::Bool(false) => {}
                        _ => return Err(internal(span, "filter の関数が boolean を返さない")),
                    }
                }
                Ok(Value::array(out))
            }
            "reduce" => {
                let f = fn_arg(args.next())?;
                let mut acc = args
                    .next()
                    .ok_or_else(|| internal(span, "reduce の初期値がない"))?;
                for x in xs {
                    self.step()?;
                    acc = self.call_value(&f, vec![acc, x.clone()], span)?;
                }
                Ok(acc)
            }
            "includes" => {
                let needle = args
                    .next()
                    .ok_or_else(|| internal(span, "includes の引数がない"))?;
                for x in xs {
                    self.step()?;
                    if strict_eq(x, &needle) {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            "concat" => {
                let Some(Value::Array(ys)) = args.next() else {
                    return Err(internal(span, "concat の引数が配列でない"));
                };
                let total = xs.len().saturating_add(ys.len());
                self.check_len(total)?;
                self.charge(u64::try_from(total).unwrap_or(u64::MAX))?;
                let mut out = Vec::with_capacity(total);
                out.extend(xs.iter().cloned());
                out.extend(ys.iter().cloned());
                Ok(Value::array(out))
            }
            _ => Err(internal(span, "未知のメソッド")),
        }
    }

    // ---- 文 ------------------------------------------------------------------

    fn block(&mut self, b: &'a Block, scope: &Arc<Scope>) -> R<Value> {
        let mut scope = Arc::clone(scope);
        for c in &b.consts {
            let v = self.expr(&c.init, &scope)?;
            let mut vars = BTreeMap::new();
            vars.insert(c.name.name.clone(), v);
            scope = Arc::new(Scope {
                vars,
                parent: Some(scope),
            });
        }
        self.stmt(&b.stmt, &scope)
    }

    fn stmt(&mut self, s: &'a Stmt, scope: &Arc<Scope>) -> R<Value> {
        self.step()?;
        match s {
            Stmt::Return(x, _) => self.expr(x, scope),
            Stmt::If {
                cond,
                then,
                otherwise,
                ..
            } => match self.expr(cond, scope)? {
                Value::Bool(true) => self.block(then, scope),
                Value::Bool(false) => match otherwise {
                    Else::Block(b) => self.block(b, scope),
                    Else::If(s) => self.stmt(s, scope),
                },
                _ => Err(internal(cond.span, "条件が boolean でない")),
            },
            Stmt::Switch {
                scrutinee,
                cases,
                default,
                span,
            } => {
                let v = self.expr(scrutinee, scope)?;
                for c in cases {
                    if strict_eq(&v, &lit_value(&c.test)) {
                        return self.stmt(&c.body, scope);
                    }
                }
                match default {
                    Some(d) => self.stmt(d, scope),
                    None => Err(internal(*span, "どの case にも合わない switch")),
                }
            }
        }
    }

    // ---- JSX -----------------------------------------------------------------

    fn props(&mut self, el: &'a Element, scope: &Arc<Scope>) -> R<BTreeMap<String, Value>> {
        let mut map = BTreeMap::new();
        for a in &el.attrs {
            match a {
                Attr::Named { name, value, .. } => {
                    let v = match value {
                        None => Value::Bool(true),
                        Some(AttrValue::String(s)) => self.string(s.value.clone())?,
                        Some(AttrValue::Expr(x)) => self.expr(x, scope)?,
                    };
                    set_field(&mut map, &name.name, v);
                }
                Attr::Spread(x, s) => match self.expr(x, scope)? {
                    Value::Object(m) => {
                        self.charge(u64::try_from(m.len()).unwrap_or(u64::MAX))?;
                        spread_into(&mut map, &m);
                    }
                    _ => return Err(internal(*s, "オブジェクトでない値の展開")),
                },
            }
        }
        Ok(map)
    }

    fn element(&mut self, el: &'a Element, scope: &Arc<Scope>) -> R<Arc<Node>> {
        let mut props = self.props(el, scope)?;
        let mut children = Vec::new();
        self.children(&el.children, scope, &mut children)?;
        let name = &el.name.name;
        if let Some(f) = self.function_named(name) {
            // ユーザーのコンポーネント: 子は配列として `children` に渡す（DECISIONS B6）。
            if !el.children.is_empty() {
                props.insert(
                    "children".to_owned(),
                    Value::array(children.into_iter().map(Value::Element).collect()),
                );
            }
            let v = self.call_value(&f, vec![Value::Object(Arc::new(props))], el.span)?;
            let Value::Element(node) = v else {
                return Err(internal(el.span, "コンポーネントが要素を返さない"));
            };
            let mut node = (*node).clone();
            node.expanded_from.push(self.file_span(el.span));
            return Ok(Arc::new(node));
        }
        self.check_unique(name, &children)?;
        let node = Arc::new(Node {
            name: name.clone(),
            props,
            children,
            span: self.file_span(el.span),
            expanded_from: Vec::new(),
        });
        self.check_depth(Tree::Element(&node))?;
        Ok(node)
    }

    /// 一意な子の重複（評価後の検査。DECISIONS E2）。
    fn check_unique(&self, name: &str, children: &[Arc<Node>]) -> R<()> {
        let Some(component) = self.schema.component(name) else {
            return Ok(());
        };
        let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
        for c in children {
            if component.unique().contains(&c.name) && seen.insert(&c.name, ()).is_some() {
                // 子を作ったファイルの位置で報告する。
                let span = Span {
                    start: c.span.start,
                    end: c.span.end,
                };
                let mut e = at(
                    Code::DuplicateUniqueChild,
                    span,
                    format!("`<{}>` は `<{name}>` の子に 1 つだけ置ける", c.name),
                );
                e.unit = self.units.iter().position(|u| u.module.file == c.span.file);
                return Err(e);
            }
        }
        Ok(())
    }

    /// 子を平らにする。`undefined` は除く（DECISIONS B6）。
    fn children(
        &mut self,
        children: &'a [Child],
        scope: &Arc<Scope>,
        out: &mut Vec<Arc<Node>>,
    ) -> R<()> {
        for c in children {
            match c {
                Child::Element(el, _) => out.push(self.element(el, scope)?),
                Child::Fragment(cs, _) => self.children(cs, scope, out)?,
                Child::Expr(x, span) => {
                    let v = self.expr(x, scope)?;
                    flatten(v, out, *span)?;
                }
            }
            self.check_len(out.len())?;
        }
        Ok(())
    }
}

/// 子の値を平らにする。配列が深く入れ子になってもスタックを使い切らないよう、明示のスタックを使う。
fn flatten(v: Value, out: &mut Vec<Arc<Node>>, span: Span) -> R<()> {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Element(e) => out.push(e),
            Value::Undefined => {}
            Value::Array(xs) => stack.extend(xs.iter().rev().cloned()),
            _ => return Err(internal(span, "子に置けない値")),
        }
    }
    Ok(())
}

/// 位置のない誤り（上限の超過）に、宣言の位置を付ける（DECISIONS D8）。
pub(crate) fn fill(mut e: EvalError, span: Span, unit: usize) -> EvalError {
    if e.span.is_none() {
        e.span = Some(span);
        e.unit = Some(unit);
    }
    if e.unit.is_none() {
        e.unit = Some(unit);
    }
    e
}

/// 明示のキー。`undefined` はキーがないのと同じ（DECISIONS B7）。
fn set_field(map: &mut BTreeMap<String, Value>, key: &str, v: Value) {
    if v == Value::Undefined {
        map.remove(key);
    } else {
        map.insert(key.to_owned(), v);
    }
}

/// スプレッド。値が `undefined` のフィールドは上書きしない（DECISIONS S1）。
fn spread_into(map: &mut BTreeMap<String, Value>, from: &BTreeMap<String, Value>) {
    for (k, v) in from {
        if *v != Value::Undefined {
            map.insert(k.clone(), v.clone());
        }
    }
}

fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Number(n, _) => Value::Number(normalize(*n)),
        Lit::String(s, _) => Value::string(s),
        Lit::Bool(b, _) => Value::Bool(*b),
    }
}

/// `===`。型の違う値どうしは等しくない（DECISIONS B13）。
fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) | (Value::Length(x), Value::Length(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Color(x), Value::Color(y)) => x == y,
        _ => false,
    }
}

/// 有限でなければ E5008（DECISIONS D2・SPEC 第 7 章）。
fn finite(x: f64, span: Span) -> R<f64> {
    if x.is_finite() {
        Ok(normalize(x))
    } else {
        Err(at(Code::NonFinite, span, "結果が有限の数にならない"))
    }
}

fn nonzero(x: f64, span: Span) -> R<f64> {
    if x == 0.0 {
        Err(at(Code::DivisionByZero, span, "0 で割った"))
    } else {
        Ok(x)
    }
}

fn duration(v: Option<i64>, span: Span) -> R<Value> {
    v.map(Value::Duration).ok_or_else(|| {
        at(
            Code::DurationRange,
            span,
            "Duration が整数ミリ秒にならないか、範囲を超えた",
        )
    })
}

fn binary(op: BinaryOp, a: Value, b: Value, span: Span) -> R<Value> {
    use BinaryOp as B;
    use Value as V;
    Ok(match (op, a, b) {
        (B::Eq, a, b) => V::Bool(strict_eq(&a, &b)),
        (B::Ne, a, b) => V::Bool(!strict_eq(&a, &b)),
        (B::Add, V::Number(x), V::Number(y)) => V::Number(finite(x + y, span)?),
        (B::Sub, V::Number(x), V::Number(y)) => V::Number(finite(x - y, span)?),
        (B::Mul, V::Number(x), V::Number(y)) => V::Number(finite(x * y, span)?),
        (B::Div, V::Number(x), V::Number(y)) => V::Number(finite(x / nonzero(y, span)?, span)?),
        (B::Rem, V::Number(x), V::Number(y)) => V::Number(finite(x % nonzero(y, span)?, span)?),
        (B::Add, V::Length(x), V::Length(y)) => V::Length(finite(x + y, span)?),
        (B::Sub, V::Length(x), V::Length(y)) => V::Length(finite(x - y, span)?),
        (B::Mul, V::Length(x), V::Number(y)) | (B::Mul, V::Number(y), V::Length(x)) => {
            V::Length(finite(x * y, span)?)
        }
        (B::Div, V::Length(x), V::Number(y)) => V::Length(finite(x / nonzero(y, span)?, span)?),
        (B::Add, V::Duration(x), V::Duration(y)) => duration(num::duration_add(x, y), span)?,
        (B::Sub, V::Duration(x), V::Duration(y)) => duration(num::duration_sub(x, y), span)?,
        (B::Mul, V::Duration(x), V::Number(y)) | (B::Mul, V::Number(y), V::Duration(x)) => {
            duration(num::duration_scale(x, y), span)?
        }
        (B::Div, V::Duration(x), V::Number(y)) => {
            duration(num::duration_div(x, nonzero(y, span)?), span)?
        }
        (B::Lt, V::Number(x), V::Number(y)) | (B::Lt, V::Length(x), V::Length(y)) => V::Bool(x < y),
        (B::Le, V::Number(x), V::Number(y)) | (B::Le, V::Length(x), V::Length(y)) => {
            V::Bool(x <= y)
        }
        (B::Gt, V::Number(x), V::Number(y)) | (B::Gt, V::Length(x), V::Length(y)) => V::Bool(x > y),
        (B::Ge, V::Number(x), V::Number(y)) | (B::Ge, V::Length(x), V::Length(y)) => {
            V::Bool(x >= y)
        }
        (B::Lt, V::Duration(x), V::Duration(y)) => V::Bool(x < y),
        (B::Le, V::Duration(x), V::Duration(y)) => V::Bool(x <= y),
        (B::Gt, V::Duration(x), V::Duration(y)) => V::Bool(x > y),
        (B::Ge, V::Duration(x), V::Duration(y)) => V::Bool(x >= y),
        _ => return Err(internal(span, "二項演算子のオペランド")),
    })
}

/// 組み込み関数 `px` `ms` `sec` `color`（DECISIONS E3）。
fn prelude(name: &str, args: Vec<Value>, span: Span) -> R<Value> {
    match (name, args.as_slice()) {
        ("px", [Value::Number(n)]) => Ok(Value::Length(finite(*n, span)?)),
        ("ms", [Value::Number(n)]) => duration(
            f64_to_i64(*n).filter(|v| v.unsigned_abs() <= num::MAX_SAFE.unsigned_abs()),
            span,
        ),
        ("sec", [Value::Number(n)]) => duration(f64_to_i64(finite(*n * 1000.0, span)?), span),
        ("color", [Value::String(s)]) => parse_color(s).map(Value::Color).ok_or_else(|| {
            at(
                Code::InvalidColor,
                span,
                format!("`{s}` は色の書式（#rgb・#rgba・#rrggbb・#rrggbbaa）でない"),
            )
        }),
        _ => Err(internal(span, &format!("組み込み関数 `{name}` の引数"))),
    }
}

/// `#rgb` `#rgba` `#rrggbb` `#rrggbbaa`（DECISIONS B12）。
pub(crate) fn parse_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |c: u8| {
        char::from(c)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    let bytes = hex.as_bytes();
    let short = |i: usize| {
        bytes
            .get(i)
            .copied()
            .and_then(digit)
            .map(|d| d.saturating_mul(17))
    };
    let long = |i: usize| {
        let hi = bytes.get(i).copied().and_then(digit)?;
        let lo = bytes.get(i.saturating_add(1)).copied().and_then(digit)?;
        Some(hi.saturating_mul(16).saturating_add(lo))
    };
    match bytes.len() {
        3 => Some([short(0)?, short(1)?, short(2)?, 255]),
        4 => Some([short(0)?, short(1)?, short(2)?, short(3)?]),
        6 => Some([long(0)?, long(2)?, long(4)?, 255]),
        8 => Some([long(0)?, long(2)?, long(4)?, long(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#89b4fa"), Some([137, 180, 250, 255]));
        assert_eq!(parse_color("#FFF"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("#0008"), Some([0, 0, 0, 136]));
        assert_eq!(parse_color("#00000080"), Some([0, 0, 0, 128]));
        assert_eq!(parse_color("blue"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#ggg"), None);
        assert_eq!(parse_color("#ａｂｃ"), None);
    }
}

/// Kani のハーネス（DECISIONS F5: K7）。
#[cfg(kani)]
mod verification {
    use super::*;

    /// K7: `color()` の解析は、9 バイト以下の任意の入力でパニックしない。
    #[kani::proof]
    #[kani::unwind(10)]
    fn parse_color_never_panics() {
        let bytes: [u8; 9] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        if let Some(Ok(s)) = bytes.get(..len).map(core::str::from_utf8) {
            let _ = parse_color(s);
        }
    }
}
