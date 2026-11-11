//! 既定の上限に、`examples/contrast/` の設定が十分な余裕で収まること（AUDIT: 既定値の妥当性。実測は小さな例だけ）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
use cdl_host::{Engine, Limits, ModuleResolver, Source};
struct Fs;
impl ModuleResolver for Fs {
    fn resolve(&self, importer: Option<&str>, spec: &str) -> Result<Source, String> {
        let base = match importer {
            Some(i) => std::path::Path::new(i).parent().unwrap().to_path_buf(),
            None => std::path::PathBuf::new(),
        };
        let p = base.join(spec);
        Ok(Source {
            id: p.to_string_lossy().into_owned(),
            text: std::fs::read_to_string(&p).map_err(|e| e.to_string())?,
        })
    }
}
#[test]
fn usage() {
    let schema = cdl_cli::contrast_schema();
    let entry = "../../examples/contrast/main.cdl";
    let min = |f: &dyn Fn(u64) -> Limits| {
        let (mut lo, mut hi) = (0u64, 1_000_000u64);
        while lo < hi {
            let m = (lo + hi) / 2;
            if Engine::new()
                .with_schema(schema.clone())
                .with_limits(f(m))
                .check(&Fs, entry)
                .is_ok()
            {
                hi = m
            } else {
                lo = m + 1
            }
        }
        lo
    };
    let d = Limits::default();
    // 2026-11-11 の実測: ステップ 140、評価の再帰 10、ノード 22。既定値の 1/10 未満であること。
    assert!(
        min(&|m| Limits {
            max_steps: m,
            ..Limits::default()
        }) < d.max_steps / 10
    );
    assert!(
        min(&|m| Limits {
            max_eval_depth: u32::try_from(m).unwrap(),
            ..Limits::default()
        }) < u64::from(d.max_eval_depth) / 10 + 1
    );
    assert!(
        min(&|m| Limits {
            max_nodes: m,
            ..Limits::default()
        }) < d.max_nodes / 10
    );
}
