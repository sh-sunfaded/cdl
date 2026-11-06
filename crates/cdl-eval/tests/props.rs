//! 評価の性質（DECISIONS F5・§4 の P3・P4・P6・P7）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "../../cdl-sema/tests/support/typed_gen.rs"]
mod typed_gen;

use cdl_diag::Code;
use cdl_eval::num::js_number_string;
use cdl_eval::sema::schema::{ComponentDef, Schema};
use cdl_eval::sema::syntax;
use cdl_eval::sema::types::{Field, Ty};
use cdl_eval::{Limits, Value, conforms, evaluate};
use proptest::prelude::*;
use typed_gen::GenTy;

/// import のない 1 つのファイル。
static NO_DEPS: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

fn unit<'a>(
    module: &'a cdl_eval::sema::syntax::ast::Module,
    checked: &'a cdl_eval::sema::Checked,
) -> cdl_eval::Unit<'a> {
    cdl_eval::Unit {
        module,
        checked,
        deps: &NO_DEPS,
    }
}

fn ty(t: &GenTy) -> Ty {
    match t {
        GenTy::Number => Ty::Number,
        GenTy::String => Ty::String,
        GenTy::Boolean => Ty::Boolean,
        GenTy::Length => Ty::Length,
        GenTy::Duration => Ty::Duration,
        GenTy::Color => Ty::Color,
        GenTy::Array(e) => Ty::Array(Box::new(ty(e))),
        GenTy::Object(fs) => {
            Ty::object(fs.iter().map(|(k, t)| (k.clone(), Field::required(ty(t)))))
        }
        GenTy::Maybe(e) => Ty::union(vec![ty(e), Ty::Undefined]),
    }
}

/// ルート要素と、`v` を返すフック `get` の型 `Policy` を持つスキーマ。
fn schema(t: &GenTy) -> Schema {
    let policy = Ty::object([(
        "get",
        Field::optional(Ty::Function(vec![Ty::Number], Box::new(ty(t)))),
    )]);
    Schema::builder()
        .root(typed_gen::ROOT_MODULE, typed_gen::ROOT_COMPONENT)
        .component(
            typed_gen::ROOT_MODULE,
            ComponentDef::new(typed_gen::ROOT_COMPONENT),
        )
        .type_def(typed_gen::ROOT_MODULE, "Policy", policy)
        .build()
        .unwrap()
}

/// 型付きの式を `const v` に置き、`v` を返すフックを通して値を取り出す。
/// 評価の誤りなら、そのコードを返す。型検査を通らない入力（入れ子の上限など）は `None`。
fn value_of(t: &GenTy, expr: &str, limits: &Limits) -> Option<Result<Value, Code>> {
    let extra = format!(
        "import type {{ Policy }} from \"{}\";\nexport const policy = {{ get: (n: number): {} => v }} satisfies Policy;\n",
        typed_gen::ROOT_MODULE,
        t.annotation()
    );
    let src = typed_gen::program_with(t, expr, &extra);
    let module = syntax::parse("main.cdl", &src, &syntax::Limits::default()).ok()?;
    let schema = schema(t);
    let checked = cdl_eval::sema::check(&module, &schema).ok()?;
    let ev = match evaluate(&[unit(&module, &checked)], 0, &schema, limits) {
        Ok(ev) => ev,
        Err(d) => return Some(Err(d.code)),
    };
    let r = cdl_eval::call_hook(
        &[unit(&module, &checked)],
        0,
        &schema,
        &ev,
        limits,
        "get",
        Value::Number(0.0),
    );
    Some(r.map(|v| v.unwrap_or(Value::Undefined)).map_err(|d| d.code))
}

/// 型検査を通った式で起きてよい評価の誤り（DECISIONS D10。E5000 は含まない）。
fn allowed(code: Code) -> bool {
    matches!(
        code,
        Code::StepLimit
            | Code::CallDepth
            | Code::StringLimit
            | Code::CollectionLimit
            | Code::NodeLimit
            | Code::ValueDepth
            | Code::DivisionByZero
            | Code::NonFinite
            | Code::DurationRange
    )
}

proptest! {
    // 回数は環境変数 PROPTEST_CASES で変える（既定 256）。

    /// P3: 型検査を通った式は、評価で型の誤り（E5000）にならず、値は静的な型に合う。
    #[test]
    fn typed_expressions_evaluate_to_values_of_their_type((t, expr) in typed_gen::gen_typed_expr()) {
        match value_of(&t, &expr, &Limits::default()) {
            None => prop_assume!(false),
            Some(Ok(v)) => prop_assert!(conforms(&v, &ty(&t)), "{expr}\n{v:?}\n{t:?}"),
            Some(Err(code)) => prop_assert!(allowed(code), "{expr}: {code:?}"),
        }
    }

    /// P4: 同じプログラムを 2 回評価すると、結果が完全に一致する。
    #[test]
    fn evaluation_is_deterministic((t, expr) in typed_gen::gen_typed_expr()) {
        let a = value_of(&t, &expr, &Limits::default());
        let b = value_of(&t, &expr, &Limits::default());
        prop_assert_eq!(a, b);
    }

    /// P7: 小さな上限をランダムに与えても評価は必ず終わり、超えたら該当する E5 になる。
    #[test]
    fn small_limits_always_stop(
        (t, expr) in typed_gen::gen_typed_expr(),
        steps in 0u64..200,
        depth in 0u32..4,
        len in 0u32..8,
        bytes in 0u32..16,
    ) {
        let limits = Limits {
            max_steps: steps,
            max_call_depth: depth,
            max_collection_len: len,
            max_string_bytes: bytes,
            max_hook_steps: steps,
            ..Limits::default()
        };
        if let Some(Err(code)) = value_of(&t, &expr, &limits) {
            prop_assert!(allowed(code), "{expr}: {code:?}");
        }
    }

    /// P6: 有限の数を文字列にして読み直すと、元の数に戻る（-0 は 0 になる）。
    #[test]
    fn number_strings_round_trip(bits in any::<u64>()) {
        let x = f64::from_bits(bits);
        prop_assume!(x.is_finite());
        let s = js_number_string(x);
        let back: f64 = s.parse().unwrap();
        prop_assert_eq!(back, if x == 0.0 { 0.0 } else { x }, "{}", s);
    }
}
