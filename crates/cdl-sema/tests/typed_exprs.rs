//! 型付き式のジェネレータで作った式が、型検査を通る（P3 の土台）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "support/typed_gen.rs"]
mod typed_gen;

use cdl_sema::schema::{ComponentDef, Schema};
use cdl_sema::syntax::{Limits, parse};
use proptest::prelude::*;

proptest! {
    // 回数は環境変数 PROPTEST_CASES で変える（既定 256）。

    #[test]
    fn generated_typed_expressions_type_check((t, expr) in typed_gen::gen_typed_expr()) {
        let src = typed_gen::program(&t, &expr);
        let module = match parse("main.cdl", &src, &Limits::default()) {
            Ok(m) => m,
            Err(d) => {
                // 生成の大きさによっては入れ子の上限を超えうる。それ以外の構文エラーはジェネレータの誤り。
                prop_assume!(!d.iter().any(|d| d.code == cdl_diag::Code::NestingTooDeep));
                return Err(TestCaseError::fail(format!("{src}\n{d:#?}")));
            }
        };
        let schema = Schema::builder()
            .root(typed_gen::ROOT_MODULE, typed_gen::ROOT_COMPONENT)
            .component(typed_gen::ROOT_MODULE, ComponentDef::new(typed_gen::ROOT_COMPONENT))
            .build()
            .unwrap();
        let result = cdl_sema::check(&module, &schema);
        prop_assert!(result.is_ok(), "{}\n{:#?}", src, result.err());
    }
}
