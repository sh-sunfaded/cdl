//! 評価の値と、要素ツリー（値木。SPEC 第 6・7 章）。
//!
//! マップはすべて `BTreeMap`（キーの順は UTF-8 のバイト順。DECISIONS B16）。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use cdl_diag::Span;

use crate::budget::{Budget, Exhausted};
use cdl_sema::types::{Literal, Ty};

/// 評価の値。`Object` はオブジェクトと `Record` の両方を表す。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Bool(bool),
    Number(f64),
    String(Arc<str>),
    /// px
    Length(f64),
    /// ミリ秒
    Duration(i64),
    /// RGBA
    Color([u8; 4]),
    Array(Arc<Vec<Value>>),
    Object(Arc<BTreeMap<String, Value>>),
    Element(Arc<Element>),
    Function(Function),
}

/// 要素ツリーの 1 ノード。コンポーネントはすべて展開済みで、組み込みコンポーネントだけが残る。
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: String,
    /// 値が `undefined` の props は持たない（DECISIONS B7）。
    pub props: BTreeMap<String, Value>,
    pub children: Vec<Arc<Element>>,
    /// この要素を作った JSX の位置（ファイル名を含む）。
    pub span: Span,
    /// 展開の途中で通ったコンポーネント呼び出しの位置（内側から順。DECISIONS D6）。
    pub expanded_from: Vec<Span>,
}

/// 関数の値。中身は評価器だけが扱う。
#[derive(Clone)]
pub struct Function(pub(crate) FnRepr);

#[derive(Clone)]
pub(crate) enum FnRepr {
    /// トップレベルの `function` 宣言。`unit` は宣言したファイルの番号。
    Named { unit: usize, name: String },
    /// アロー関数。`arrow` はファイルの中のアロー関数の番号（決まった順に振る）。
    Closure {
        unit: usize,
        arrow: usize,
        env: Arc<Scope>,
    },
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            FnRepr::Named { name, .. } => write!(f, "Function({name})"),
            FnRepr::Closure { arrow, .. } => write!(f, "Function(<arrow #{arrow}>)"),
        }
    }
}

impl PartialEq for Function {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (FnRepr::Named { unit: u, name: a }, FnRepr::Named { unit: v, name: b }) => {
                u == v && a == b
            }
            (
                FnRepr::Closure {
                    unit: u,
                    arrow: a,
                    env: x,
                },
                FnRepr::Closure {
                    unit: v,
                    arrow: b,
                    env: y,
                },
            ) => u == v && a == b && Arc::ptr_eq(x, y),
            _ => false,
        }
    }
}

/// 局所の束縛。親をたどって名前を探す。
#[derive(Debug, Default)]
pub(crate) struct Scope {
    pub vars: BTreeMap<String, Value>,
    pub parent: Option<Arc<Scope>>,
}

impl Scope {
    pub fn lookup(&self, name: &str) -> Option<&Value> {
        let mut scope = Some(self);
        while let Some(s) = scope {
            if let Some(v) = s.vars.get(name) {
                return Some(v);
            }
            scope = s.parent.as_deref();
        }
        None
    }
}

impl Value {
    pub fn string(s: &str) -> Value {
        Value::String(Arc::from(s))
    }

    pub fn object<I, S>(fields: I) -> Value
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        Value::Object(Arc::new(
            fields.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        ))
    }

    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Arc::new(items))
    }
}

/// 値が型に合うか（ホストから受け取った値の検査に使う。DECISIONS E5）。
pub fn conforms(v: &Value, ty: &Ty) -> bool {
    match (v, ty) {
        (_, Ty::Error) => true,
        (v, Ty::Union(ms)) => ms.iter().any(|m| conforms(v, m)),
        (Value::Undefined, Ty::Undefined)
        | (Value::Bool(_), Ty::Boolean)
        | (Value::Number(_), Ty::Number)
        | (Value::String(_), Ty::String)
        | (Value::Length(_), Ty::Length)
        | (Value::Duration(_), Ty::Duration)
        | (Value::Color(_), Ty::Color)
        | (Value::Function(_), Ty::Function(..)) => true,
        (Value::Bool(b), Ty::Literal(Literal::Bool(l))) => b == l,
        (Value::Number(n), Ty::Literal(Literal::Number(l))) => n == l,
        (Value::String(s), Ty::Literal(Literal::String(l))) => **s == **l,
        (Value::Array(xs), Ty::Array(t)) => xs.iter().all(|x| conforms(x, t)),
        (Value::Array(xs), Ty::Tuple(ts)) => {
            xs.len() == ts.len() && xs.iter().zip(ts).all(|(x, t)| conforms(x, t))
        }
        (Value::Object(m), Ty::Record(t)) => m.values().all(|x| conforms(x, t)),
        (Value::Object(m), Ty::Object(fields)) => {
            m.keys().all(|k| fields.contains_key(k))
                && fields.iter().all(|(k, f)| match m.get(k) {
                    Some(x) => conforms(x, &f.effective()),
                    // キーがないことは、値が undefined であることと同じ（DECISIONS B7）。
                    None => conforms(&Value::Undefined, &f.effective()),
                })
        }
        (Value::Element(e), Ty::Element(names)) => names.contains(&e.name),
        _ => false,
    }
}

/// 深さを測る対象。
#[derive(Clone, Copy)]
pub(crate) enum Tree<'a> {
    Value(&'a Value),
    Element(&'a Element),
}

/// 値木の深さが `limit` を超えるか（DECISIONS D11）。要素・配列・オブジェクトを 1 段と数え、葉は数えない。
/// 深い木でもスタックを使い切らないよう、明示のスタックで数える。超えた時点で打ち切る。
/// `budget` があれば、たどった値 1 つにつき 1 ステップを使う（共有された値を木として何度もたどる
/// 作業が、ステップ上限をすり抜けないようにする。DECISIONS D13）。使い切ったら `Err`。
pub(crate) fn depth_exceeds(
    root: Tree<'_>,
    limit: u32,
    mut budget: Option<&mut Budget>,
) -> Result<bool, Exhausted> {
    let mut stack: Vec<(Tree<'_>, u32)> = vec![(root, 0)];
    while let Some((tree, above)) = stack.pop() {
        if let Some(b) = budget.as_deref_mut() {
            b.charge(1)?;
        }
        let level = above.saturating_add(1);
        match tree {
            Tree::Value(Value::Array(xs)) => {
                if level > limit {
                    return Ok(true);
                }
                stack.extend(xs.iter().map(|x| (Tree::Value(x), level)));
            }
            Tree::Value(Value::Object(m)) => {
                if level > limit {
                    return Ok(true);
                }
                stack.extend(m.values().map(|x| (Tree::Value(x), level)));
            }
            Tree::Value(Value::Element(e)) => stack.push((Tree::Element(e), above)),
            Tree::Value(_) => {}
            Tree::Element(e) => {
                if level > limit {
                    return Ok(true);
                }
                stack.extend(e.props.values().map(|x| (Tree::Value(x), level)));
                stack.extend(e.children.iter().map(|c| (Tree::Element(c), level)));
            }
        }
    }
    Ok(false)
}

/// 値木のノード数（要素・配列・オブジェクト・葉のすべて。DECISIONS D4）。
pub(crate) fn count_nodes(e: &Element, limit: u64) -> u64 {
    // 深い木でもスタックを使い切らないよう、明示のスタックで数える。
    let mut stack: Vec<&Element> = vec![e];
    let mut values: Vec<&Value> = Vec::new();
    let mut n: u64 = 0;
    while let Some(el) = stack.pop() {
        n = n.saturating_add(1);
        values.extend(el.props.values());
        stack.extend(el.children.iter().map(Arc::as_ref));
        while let Some(v) = values.pop() {
            n = n.saturating_add(1);
            match v {
                Value::Array(xs) => values.extend(xs.iter()),
                Value::Object(m) => values.extend(m.values()),
                Value::Element(el) => stack.push(el),
                _ => {}
            }
            if n > limit {
                return n;
            }
        }
        if n > limit {
            break;
        }
    }
    n
}
