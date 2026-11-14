//! 評価器とリソース上限（SPEC 第 7・8 章）。
#![forbid(unsafe_code)]

mod budget;
mod eval;
pub mod num;
pub mod value;

use std::collections::BTreeMap;
use std::sync::Arc;

use cdl_diag::{Code, Diagnostic};
use cdl_sema::Checked;
use cdl_sema::schema::Schema;
use cdl_sema::syntax::ast::{ItemKind, Module, Span};
use cdl_sema::types::Ty;

/// 意味解析。上のクレートは、このクレートを通して使う（DECISIONS H1）。
pub use cdl_sema as sema;

pub use budget::{Budget, Exhausted};
pub use value::{ByteRange, ChildNode, Element, Function, Value, conforms};

/// 評価の上限（SPEC 第 7・8 章。既定値は暫定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
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
    /// 評価結果の値木の深さ（要素・配列・オブジェクトを 1 段と数える。DECISIONS D11）。
    pub max_value_depth: u32,
    /// フックの呼び出し 1 回あたりの評価ステップ。
    pub max_hook_steps: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_steps: 1_000_000,
            max_call_depth: 64,
            max_eval_depth: 128,
            max_string_bytes: 64 * 1024,
            max_collection_len: 65_536,
            max_nodes: 1_000_000,
            max_value_depth: 64,
            max_hook_steps: 10_000,
        }
    }
}

/// 評価する 1 つのファイル。
#[derive(Clone, Copy, Debug)]
pub struct Unit<'a> {
    pub module: &'a Module,
    pub checked: &'a Checked,
    /// import の指定子 → そのファイルの番号（`units` の添字）。
    pub deps: &'a BTreeMap<String, usize>,
}

/// 評価の結果。`document` は不変の要素ツリー。
#[derive(Clone, Debug)]
pub struct Evaluated {
    /// 静的な形のルートの要素ツリー。関数の形のルートでは `None`（[`render`] で作る。DECISIONS U1）。
    pub document: Option<Arc<Element>>,
    globals: Vec<BTreeMap<String, Value>>,
}

/// 複数の誤りを、位置順に並べた診断にする。
fn diagnostics(units: &[Unit<'_>], entry: usize, es: Vec<eval::EvalError>) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = es
        .into_iter()
        .map(|e| {
            let unit = e.unit.unwrap_or(entry);
            let span = e.span.unwrap_or_default();
            diagnostic(units, entry, eval::fill(e, span, unit))
        })
        .collect();
    cdl_diag::sort(&mut out);
    out
}

fn diagnostic(units: &[Unit<'_>], entry: usize, e: eval::EvalError) -> Diagnostic {
    let span = e.span.unwrap_or_default();
    let file = units
        .get(e.unit.unwrap_or(entry))
        .map(|u| u.module.file.clone())
        .unwrap_or_default();
    Diagnostic::error(
        e.code,
        cdl_diag::Span {
            file,
            start: span.start,
            end: span.end,
        },
        e.message,
    )
}

fn evaluator<'a>(
    units: &[Unit<'a>],
    schema: &'a Schema,
    limits: &Limits,
    steps: u64,
) -> eval::Evaluator<'a> {
    let states = units
        .iter()
        .map(|u| eval::UnitState::new(u.module, u.deps))
        .collect();
    eval::Evaluator::new(states, schema, *limits, steps)
}

/// ファイルを評価する。`units` は依存先が先に来る順で、`entry` がエントリファイル。
/// 同じファイルは 1 回だけ評価する（SPEC 第 9 章）。誤りは最初の 1 つで打ち切る（DECISIONS D9）。
/// ただし、見つかった `key` の重複（E4011）は、すべて報告する（DECISIONS U3）。
pub fn evaluate(
    units: &[Unit<'_>],
    entry: usize,
    schema: &Schema,
    limits: &Limits,
) -> Result<Evaluated, Vec<Diagnostic>> {
    let mut ev = evaluator(units, schema, limits, limits.max_steps);
    for (i, u) in units.iter().enumerate() {
        ev.globals(i, &u.checked.const_order)
            .map_err(|e| vec![diagnostic(units, entry, e)])?;
    }
    let document = if ev.is_function_root(entry) {
        None
    } else {
        Some(
            ev.document(entry, None)
                .map_err(|es| diagnostics(units, entry, es))?,
        )
    };
    Ok(Evaluated {
        document,
        globals: ev.units.into_iter().map(|u| u.globals).collect(),
    })
}

/// ルートの要素ツリーを作る。静的な形のルートは、評価済みの木をそのまま返す（`env` は無視する）。
/// 関数の形のルートは、`env` を引数にして呼ぶ。呼び出しごとに独立したステップ予算（`max_steps`）を使い、
/// 同じ `env` からは必ず同じ木になる（DECISIONS U1）。`env` がホストの `Env` の型に合わなければ E4014。
pub fn render(
    units: &[Unit<'_>],
    entry: usize,
    schema: &Schema,
    evaluated: &Evaluated,
    limits: &Limits,
    env: &Value,
) -> Result<Arc<Element>, Vec<Diagnostic>> {
    if let Some(document) = &evaluated.document {
        return Ok(Arc::clone(document));
    }
    let span = units
        .get(entry)
        .map(|u| {
            u.module
                .items
                .iter()
                .find_map(|i| match &i.kind {
                    ItemKind::ExportDefault(e) => Some(e.span),
                    _ => None,
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let fail = |code: Code, message: String| {
        vec![diagnostic(
            units,
            entry,
            eval::EvalError {
                code,
                span: Some(span),
                unit: Some(entry),
                message,
            },
        )]
    };
    let Some(env_ty) = schema.env() else {
        return Err(fail(
            Code::EnvMismatch,
            "このホストは `Env` を宣言していない".to_owned(),
        ));
    };
    if value::depth_exceeds(value::Tree::Value(env), limits.max_value_depth, None) == Ok(true) {
        return Err(fail(
            Code::ValueDepth,
            format!("`Env` の値の深さが上限 {} を超えた", limits.max_value_depth),
        ));
    }
    if !conforms(env, env_ty) {
        return Err(fail(
            Code::EnvMismatch,
            format!("`Env` の値が `{env_ty}` に合わない"),
        ));
    }
    let mut ev = evaluator(units, schema, limits, limits.max_steps);
    for (state, globals) in ev.units.iter_mut().zip(&evaluated.globals) {
        state.globals = globals.clone();
    }
    ev.reset_budget(limits.max_steps);
    ev.document(entry, Some(env.clone()))
        .map_err(|es| diagnostics(units, entry, es))
}

/// `export const policy` の名前の位置。
fn policy_span(module: &Module) -> Span {
    module
        .items
        .iter()
        .find_map(|i| match &i.kind {
            ItemKind::Const(c) if i.exported && c.name.name == "policy" => Some(c.name.span),
            _ => None,
        })
        .unwrap_or_default()
}

/// フックを呼ぶ（SPEC 第 8 章）。フックが定義されていなければ `Ok(None)`（ホストは既定の動作をとる）。
/// 呼び出しごとに専用のステップ上限を使う。
pub fn call_hook(
    units: &[Unit<'_>],
    entry: usize,
    schema: &Schema,
    evaluated: &Evaluated,
    limits: &Limits,
    hook: &str,
    arg: Value,
) -> Result<Option<Value>, Diagnostic> {
    let Some(unit) = units.get(entry) else {
        return Ok(None);
    };
    let span = policy_span(unit.module);
    let Some(Value::Object(policy)) = evaluated.globals.get(entry).and_then(|g| g.get("policy"))
    else {
        return Ok(None);
    };
    let Some(Value::Function(f)) = policy.get(hook) else {
        return Ok(None);
    };
    let param = match &unit.checked.policy_type {
        Some(Ty::Object(fields)) => match fields.get(hook).map(|f| &f.ty) {
            Some(Ty::Function(params, _)) => params.first().cloned(),
            _ => None,
        },
        _ => None,
    };
    // ホストから渡された値も、再帰する前に深さを抑える。
    if value::depth_exceeds(value::Tree::Value(&arg), limits.max_value_depth, None) == Ok(true) {
        let e = eval::EvalError {
            code: Code::ValueDepth,
            span: Some(span),
            unit: Some(entry),
            message: format!(
                "フック `{hook}` の引数の深さが上限 {} を超えた",
                limits.max_value_depth
            ),
        };
        return Err(diagnostic(units, entry, e));
    }
    if let Some(param) = &param
        && !conforms(&arg, param)
    {
        let e = eval::EvalError {
            code: Code::HookArgument,
            span: Some(span),
            unit: Some(entry),
            message: format!("フック `{hook}` の引数が `{param}` に合わない"),
        };
        return Err(diagnostic(units, entry, e));
    }
    let mut ev = evaluator(units, schema, limits, limits.max_hook_steps);
    for (state, globals) in ev.units.iter_mut().zip(&evaluated.globals) {
        state.globals = globals.clone();
    }
    ev.reset_budget(limits.max_hook_steps);
    ev.call(f, vec![arg], span)
        .map(Some)
        .map_err(|e| diagnostic(units, entry, eval::fill(e, span, entry)))
}
