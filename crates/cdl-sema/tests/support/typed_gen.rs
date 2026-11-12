//! 型付き式のジェネレータ（DECISIONS F5・§4 の P3 の土台）。
//!
//! 先に型を選び、その型を持つ式のソースを組み立てる。生成した式は、構成のしかたから
//! 型検査を通るはずである。評価器ができたら、同じジェネレータで「型検査を通った式は、
//! 評価で型エラーにならない」ことを確かめる（`#[path]` でこのファイルを取り込む）。
//!
//! `&&`（DECISIONS R2）: `boolean` どうしの `&&` / `||` は生成する（型は `boolean` のまま）。
//! 右辺が boolean でない `c && B`（型は `false | B`）は、このジェネレータの型 [`GenTy`] に
//! `false | B` がないので、対象外にする。代わりに、`cdl-sema/tests/check.rs`（型付け）と
//! `cdl-eval/tests/eval.rs`（`false` の除去と短絡評価。条件の列を生成する proptest を含む）、
//! `conformance/5/and-render` が守る。

use proptest::prelude::*;

/// 生成する型。CDL の型の部分集合。
#[derive(Clone, Debug, PartialEq)]
pub enum GenTy {
    Number,
    String,
    Boolean,
    Length,
    Duration,
    Color,
    Array(Box<GenTy>),
    Object(Vec<(String, GenTy)>),
    Maybe(Box<GenTy>),
}

impl GenTy {
    /// 型注釈として書いた形。
    pub fn annotation(&self) -> String {
        match self {
            GenTy::Number => "number".to_owned(),
            GenTy::String => "string".to_owned(),
            GenTy::Boolean => "boolean".to_owned(),
            GenTy::Length => "Length".to_owned(),
            GenTy::Duration => "Duration".to_owned(),
            GenTy::Color => "Color".to_owned(),
            GenTy::Array(t) => format!("({})[]", t.annotation()),
            GenTy::Object(fields) => {
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(k, t)| format!("{k}: {};", t.annotation()))
                    .collect();
                format!("{{ {} }}", fs.join(" "))
            }
            GenTy::Maybe(t) => format!("{} | undefined", t.annotation()),
        }
    }
}

pub fn gen_ty() -> impl Strategy<Value = GenTy> {
    let leaf = prop_oneof![
        Just(GenTy::Number),
        Just(GenTy::String),
        Just(GenTy::Boolean),
        Just(GenTy::Length),
        Just(GenTy::Duration),
        Just(GenTy::Color),
    ];
    leaf.prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|t| GenTy::Array(Box::new(t))),
            proptest::collection::btree_map("[a-z]{1,3}", inner.clone(), 1..3)
                .prop_map(|m| GenTy::Object(m.into_iter().collect())),
            inner
                .prop_filter("Maybe の入れ子にしない", |t| !matches!(
                    t,
                    GenTy::Maybe(_)
                ))
                .prop_map(|t| GenTy::Maybe(Box::new(t))),
        ]
    })
}

fn num() -> BoxedStrategy<String> {
    prop_oneof![
        (0u32..1000).prop_map(|n| n.to_string()),
        (0u32..100).prop_map(|n| format!("{n}.5"))
    ]
    .boxed()
}

/// `t` 型の式（括弧で囲んだソース）。`depth` で入れ子を抑える。
pub fn gen_expr(t: &GenTy, depth: u32) -> BoxedStrategy<String> {
    let leaf = gen_leaf(t);
    if depth == 0 {
        return leaf;
    }
    // コールバックの引数名は深さで区別する（入れ子で名前を隠さない。DECISIONS C6）。
    let d = depth - 1;
    let composite: BoxedStrategy<String> = match t {
        GenTy::Number => prop_oneof![
            (
                gen_expr(t, d),
                gen_expr(t, d),
                prop::sample::select(vec!["+", "-", "*", "%"])
            )
                .prop_map(|(a, b, op)| format!("({a} {op} {b})")),
            gen_expr(t, d).prop_map(|a| format!("(-{a})")),
            (gen_expr(&GenTy::Boolean, d), gen_expr(t, d), gen_expr(t, d))
                .prop_map(|(c, a, b)| format!("({c} ? {a} : {b})")),
            gen_expr(&GenTy::Array(Box::new(GenTy::Number)), d)
                .prop_map(|xs| format!("({xs}.length)")),
            (
                gen_expr(&GenTy::Array(Box::new(GenTy::Number)), d),
                gen_expr(t, d)
            )
                .prop_map(|(xs, b)| format!("({xs}[0] ?? {b})")),
            gen_expr(&GenTy::Array(Box::new(GenTy::Number)), d)
                .prop_map(move |xs| format!("({xs}.reduce((acc{d}, x{d}) => acc{d} + x{d}, 0))")),
        ]
        .boxed(),
        GenTy::String => prop_oneof![
            (gen_expr(t, d), gen_expr(&GenTy::Number, d))
                .prop_map(|(a, n)| format!("(`${{{a}}}-${{{n}}}`)")),
            (gen_expr(&GenTy::Boolean, d), gen_expr(t, d), gen_expr(t, d))
                .prop_map(|(c, a, b)| format!("({c} ? {a} : {b})")),
        ]
        .boxed(),
        GenTy::Boolean => prop_oneof![
            (
                gen_expr(&GenTy::Number, d),
                gen_expr(&GenTy::Number, d),
                prop::sample::select(vec!["<", "<=", ">", ">=", "===", "!=="])
            )
                // リテラル型どうしの `===` は重なりがないと誤りになる（B13）ので、number に広げて比べる。
                .prop_map(|(a, b, op)| format!("(({a} + 0) {op} ({b} + 0))")),
            (
                gen_expr(t, d),
                gen_expr(t, d),
                prop::sample::select(vec!["&&", "||"])
            )
                .prop_map(|(a, b, op)| format!("({a} {op} {b})")),
            gen_expr(t, d).prop_map(|a| format!("(!{a})")),
            (gen_expr(&GenTy::Length, d), gen_expr(&GenTy::Length, d))
                .prop_map(|(a, b)| format!("({a} < {b})")),
            (
                gen_expr(&GenTy::Array(Box::new(GenTy::Number)), d),
                gen_expr(&GenTy::Number, d)
            )
                .prop_map(|(xs, x)| format!("({xs}.includes({x}))")),
        ]
        .boxed(),
        GenTy::Length => prop_oneof![
            (
                gen_expr(t, d),
                gen_expr(t, d),
                prop::sample::select(vec!["+", "-"])
            )
                .prop_map(|(a, b, op)| format!("({a} {op} {b})")),
            (gen_expr(t, d), gen_expr(&GenTy::Number, d)).prop_map(|(a, n)| format!("({a} * {n})")),
            (gen_expr(&GenTy::Number, d), gen_expr(t, d)).prop_map(|(n, a)| format!("({n} * {a})")),
            (gen_expr(t, d), gen_expr(&GenTy::Number, d)).prop_map(|(a, n)| format!("({a} / {n})")),
            gen_expr(t, d).prop_map(|a| format!("(-{a})")),
        ]
        .boxed(),
        GenTy::Duration => prop_oneof![
            (
                gen_expr(t, d),
                gen_expr(t, d),
                prop::sample::select(vec!["+", "-"])
            )
                .prop_map(|(a, b, op)| format!("({a} {op} {b})")),
            (gen_expr(t, d), gen_expr(&GenTy::Number, d)).prop_map(|(a, n)| format!("({a} * {n})")),
        ]
        .boxed(),
        GenTy::Color => (gen_expr(&GenTy::Boolean, d), gen_expr(t, d), gen_expr(t, d))
            .prop_map(|(c, a, b)| format!("({c} ? {a} : {b})"))
            .boxed(),
        GenTy::Array(e) => {
            let e = (**e).clone();
            prop_oneof![
                (gen_expr(t, d), gen_expr(&GenTy::Boolean, d))
                    .prop_map(move |(xs, b)| format!("({xs}.filter((x{d}) => {b}))")),
                (gen_expr(t, d), gen_expr(t, d)).prop_map(|(a, b)| format!("({a}.concat({b}))")),
                (
                    gen_expr(&GenTy::Array(Box::new(GenTy::Number)), d),
                    gen_expr(&e, d)
                )
                    .prop_map(move |(xs, x)| format!("({xs}.map((n{d}) => ({x})))")),
                (gen_expr(t, d), gen_expr(&e, d)).prop_map(|(xs, x)| format!("([...{xs}, {x}])")),
            ]
            .boxed()
        }
        GenTy::Object(_) | GenTy::Maybe(_) => {
            (gen_expr(&GenTy::Boolean, d), gen_leaf(t), gen_leaf(t))
                .prop_map(|(c, a, b)| format!("({c} ? {a} : {b})"))
                .boxed()
        }
    };
    prop_oneof![leaf, composite].boxed()
}

/// 演算を含まない、`t` 型の最小の式。
fn gen_leaf(t: &GenTy) -> BoxedStrategy<String> {
    match t {
        GenTy::Number => num(),
        GenTy::String => "[a-z ]{0,6}".prop_map(|s| format!("\"{s}\"")).boxed(),
        GenTy::Boolean => any::<bool>().prop_map(|b| b.to_string()).boxed(),
        GenTy::Length => num().prop_map(|n| format!("px({n})")).boxed(),
        GenTy::Duration => prop_oneof![
            (0u32..10_000).prop_map(|n| format!("ms({n})")),
            (0u32..100).prop_map(|n| format!("sec({n})")),
        ]
        .boxed(),
        GenTy::Color => prop::sample::select(vec!["#fff", "#89b4fa", "#00000080", "#abcd"])
            .prop_map(|c| format!("color(\"{c}\")"))
            .boxed(),
        GenTy::Array(e) => proptest::collection::vec(gen_leaf(e), 1..3)
            .prop_map(|xs| format!("[{}]", xs.join(", ")))
            .boxed(),
        GenTy::Object(fields) => {
            let parts: Vec<BoxedStrategy<String>> = fields
                .iter()
                .map(|(k, ft)| {
                    let k = k.clone();
                    gen_leaf(ft).prop_map(move |v| format!("{k}: {v}")).boxed()
                })
                .collect();
            parts
                .prop_map(|ps| format!("{{ {} }}", ps.join(", ")))
                .boxed()
        }
        GenTy::Maybe(inner) => prop_oneof![Just("undefined".to_owned()), gen_leaf(inner)].boxed(),
    }
}

/// 型とその型の式の組。
pub fn gen_typed_expr() -> impl Strategy<Value = (GenTy, String)> {
    gen_ty().prop_flat_map(|t| {
        let e = gen_expr(&t, 3);
        (Just(t), e)
    })
}

/// ルート要素を置くためのホストのモジュール。テストは、名前が [`ROOT_MODULE`] で
/// 組み込みコンポーネント [`ROOT_COMPONENT`]（props も子もない）を持つスキーマを用意する。
pub const ROOT_MODULE: &str = "host:gen";
pub const ROOT_COMPONENT: &str = "Root";

/// 式を、型注釈つきの `const` に置いたプログラムにする。
#[allow(dead_code)]
pub fn program(t: &GenTy, expr: &str) -> String {
    program_with(t, expr, "")
}

/// [`program`] に、トップレベルの宣言 `extra` を足したもの。
pub fn program_with(t: &GenTy, expr: &str, extra: &str) -> String {
    format!(
        "\"use cdl 1\";\nimport {{ {ROOT_COMPONENT} }} from \"{ROOT_MODULE}\";\nconst v: {} = {expr};\n{extra}export default (<{ROOT_COMPONENT} />);\n",
        t.annotation()
    )
}
