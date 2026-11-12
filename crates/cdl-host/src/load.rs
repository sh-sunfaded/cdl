//! ファイルの読み込みと、段階の進め方（SPEC 第 7・9 章、DECISIONS M1–M3）。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cdl_diag::{Code, Diagnostic, Span};
use cdl_eval::Unit;
use cdl_eval::sema::syntax::ast::{ItemKind, Module};
use cdl_eval::sema::{self, Interface, graph, syntax};

use crate::{Engine, ModuleResolver, Program, Stage};

fn error(file: &str, start: u32, end: u32, code: Code, message: impl Into<String>) -> Diagnostic {
    let span = Span {
        file: file.to_owned(),
        start,
        end,
    };
    Diagnostic::error(code, span, message)
}

/// ファイルの import の指定子の形（DECISIONS M1）。
fn valid_specifier(s: &str) -> bool {
    let relative = s.starts_with("./") || s.starts_with("../");
    let stem = s.strip_suffix(".cdl").unwrap_or_default();
    relative && !stem.ends_with('/') && !stem.is_empty() && !s.contains("://")
}

fn sorted(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    cdl_diag::sort(&mut diags);
    diags
}

/// 読み込んだファイルの集合。
struct Files {
    ids: Vec<String>,
    modules: Vec<Module>,
    /// import の指定子 → ファイルの番号。
    deps: Vec<BTreeMap<String, usize>>,
}

/// エントリから import をたどって、すべてのファイルを読み、構文解析する。
fn collect(
    engine: &Engine,
    resolver: &dyn ModuleResolver,
    entry: &str,
) -> Result<Files, Vec<Diagnostic>> {
    let first = resolver.resolve(None, entry).map_err(|reason| {
        vec![error(
            entry,
            0,
            0,
            Code::ModuleNotFound,
            format!("`{entry}` を読めない: {reason}"),
        )]
    })?;
    let mut ids = vec![first.id.clone()];
    let mut texts = vec![first.text];
    let mut index: BTreeMap<String, usize> = BTreeMap::from([(first.id, 0)]);
    let mut modules: Vec<Option<Module>> = Vec::new();
    let mut deps: Vec<BTreeMap<String, usize>> = Vec::new();
    let mut diags = Vec::new();
    let mut queue = VecDeque::from([0usize]);
    let limits = engine.limits.syntax();
    while let Some(i) = queue.pop_front() {
        let (Some(id), Some(text)) = (ids.get(i).cloned(), texts.get(i).cloned()) else {
            continue;
        };
        let parsed = syntax::parse(&id, &text, &limits);
        let mut my_deps = BTreeMap::new();
        if let Ok(module) = &parsed {
            for item in &module.items {
                let ItemKind::Import(import) = &item.kind else {
                    continue;
                };
                let spec = &import.source.value;
                if spec.starts_with("host:") {
                    continue;
                }
                let span = import.source.span;
                if !valid_specifier(spec) {
                    diags.push(error(
                        &id,
                        span.start,
                        span.end,
                        Code::SpecifierForm,
                        format!("`{spec}`: import は `./` か `../` で始まり `.cdl` で終わる相対パスで書く"),
                    ));
                    continue;
                }
                match resolver.resolve(Some(&id), spec) {
                    Err(reason) => diags.push(error(
                        &id,
                        span.start,
                        span.end,
                        Code::ModuleNotFound,
                        format!("`{spec}` を読めない: {reason}"),
                    )),
                    Ok(source) => {
                        let target = match index.get(&source.id) {
                            Some(&t) => t,
                            None if u32::try_from(ids.len()).unwrap_or(u32::MAX)
                                >= engine.limits.max_modules =>
                            {
                                diags.push(error(
                                    &id,
                                    span.start,
                                    span.end,
                                    Code::ModuleLimit,
                                    format!(
                                        "読み込むファイルが上限 {} を超えた",
                                        engine.limits.max_modules
                                    ),
                                ));
                                continue;
                            }
                            None => {
                                let t = ids.len();
                                index.insert(source.id.clone(), t);
                                ids.push(source.id);
                                texts.push(source.text);
                                queue.push_back(t);
                                t
                            }
                        };
                        my_deps.insert(spec.clone(), target);
                    }
                }
            }
        }
        match parsed {
            Ok(m) => set(&mut modules, i, Some(m)),
            Err(d) => {
                diags.extend(d);
                set(&mut modules, i, None);
            }
        }
        set(&mut deps, i, my_deps);
    }
    if !diags.is_empty() {
        return Err(sorted(diags));
    }
    let modules: Vec<Module> = modules.into_iter().flatten().collect();
    Ok(Files { ids, modules, deps })
}

/// `v[i] = x`（足りなければ伸ばす）。
fn set<T: Default>(v: &mut Vec<T>, i: usize, x: T) {
    while v.len() <= i {
        v.push(T::default());
    }
    if let Some(slot) = v.get_mut(i) {
        *slot = x;
    }
}

/// import の循環を 1 つの診断にまとめる（DECISIONS M2）。
fn cycle_diagnostic(files: &Files, adj: &[Vec<usize>]) -> Option<Diagnostic> {
    let cyclic: BTreeSet<usize> = graph::sccs(adj)
        .into_iter()
        .filter(|c| graph::is_cycle(adj, c))
        .flatten()
        .collect();
    if cyclic.is_empty() {
        return None;
    }
    // 循環の中を指す import の位置（ファイルごとに最初のもの）。
    let site = |i: usize| -> Option<Span> {
        let module = files.modules.get(i)?;
        let deps = files.deps.get(i)?;
        module.items.iter().find_map(|item| match &item.kind {
            ItemKind::Import(imp)
                if deps
                    .get(&imp.source.value)
                    .is_some_and(|t| cyclic.contains(t)) =>
            {
                Some(Span {
                    file: module.file.clone(),
                    start: imp.source.span.start,
                    end: imp.source.span.end,
                })
            }
            _ => None,
        })
    };
    let mut members: Vec<usize> = cyclic.iter().copied().collect();
    members.sort_by_key(|&i| files.ids.get(i).cloned());
    let names: Vec<String> = members
        .iter()
        .filter_map(|&i| files.ids.get(i).cloned())
        .collect();
    let (&first, rest) = members.split_first()?;
    let mut d = Diagnostic::error(
        Code::ImportCycle,
        site(first)?,
        format!("import が循環している（{}）", names.join("・")),
    );
    for &i in rest {
        if let Some(s) = site(i) {
            d = d.with_label(s, "循環に含まれる import");
        }
    }
    Some(d)
}

pub(crate) fn load(
    engine: &Engine,
    resolver: &dyn ModuleResolver,
    entry: &str,
) -> Result<Program, Vec<Diagnostic>> {
    load_until(engine, resolver, entry, Stage::Eval)?.ok_or_else(|| {
        vec![error(
            entry,
            0,
            0,
            Code::Internal,
            "評価まで進めたのにプログラムができなかった（実装の不具合）",
        )]
    })
}

/// `stage` の終わりまで進める。その段階で止めたときは `Ok(None)`、評価まで進めたときは `Ok(Some(_))`。
pub(crate) fn load_until(
    engine: &Engine,
    resolver: &dyn ModuleResolver,
    entry: &str,
    stage: Stage,
) -> Result<Option<Program>, Vec<Diagnostic>> {
    let files = collect(engine, resolver, entry)?;
    let adj: Vec<Vec<usize>> = files
        .deps
        .iter()
        .map(|d| d.values().copied().collect())
        .collect();
    if let Some(d) = cycle_diagnostic(&files, &adj) {
        return Err(vec![d]);
    }
    if stage == Stage::Syntax {
        return Ok(None);
    }
    // 依存先が先に来る順に並べ替える。
    let order = graph::topo_order(&adj);
    let mut renumber = vec![0usize; order.len()];
    for (new, &old) in order.iter().enumerate() {
        if let Some(slot) = renumber.get_mut(old) {
            *slot = new;
        }
    }
    let mut modules: Vec<Option<Module>> = files.modules.into_iter().map(Some).collect();
    let modules: Vec<Module> = order
        .iter()
        .filter_map(|&old| modules.get_mut(old).and_then(Option::take))
        .collect();
    let deps: Vec<BTreeMap<String, usize>> = order
        .iter()
        .filter_map(|&old| files.deps.get(old))
        .map(|d| {
            d.iter()
                .map(|(k, &v)| (k.clone(), renumber.get(v).copied().unwrap_or(v)))
                .collect()
        })
        .collect();
    let entry = renumber.first().copied().unwrap_or(0);
    let imports_of = |i: usize, interfaces: &[Interface]| -> BTreeMap<String, Interface> {
        deps.get(i)
            .map(|d| {
                d.iter()
                    .filter_map(|(spec, &t)| interfaces.get(t).map(|x| (spec.clone(), x.clone())))
                    .collect()
            })
            .unwrap_or_default()
    };

    // 名前解決（すべてのファイル。DECISIONS M3）。
    let skeletons: Vec<Interface> = modules.iter().map(sema::interface_skeleton).collect();
    let mut diags = Vec::new();
    for (i, module) in modules.iter().enumerate() {
        diags.extend(sema::resolve_module(
            module,
            &engine.schema,
            &imports_of(i, &skeletons),
            i == entry,
        ));
    }
    if !diags.is_empty() {
        return Err(sorted(diags));
    }

    // 型検査（依存先から順に。公開物の型が決まってから使う）。
    let mut interfaces = skeletons;
    let mut checked = Vec::new();
    for (i, module) in modules.iter().enumerate() {
        match sema::check_module(
            module,
            &engine.schema,
            &imports_of(i, &interfaces),
            i == entry,
        ) {
            Ok(c) => {
                if let Some(slot) = interfaces.get_mut(i) {
                    *slot = c.interface.clone();
                }
                checked.push(c);
            }
            Err(d) => diags.extend(d),
        }
    }
    if !diags.is_empty() {
        return Err(sorted(diags));
    }
    if stage == Stage::Sema {
        return Ok(None);
    }

    // 評価（同じファイルは 1 回だけ）。
    let limits = engine.limits.eval();
    let units: Vec<Unit<'_>> = modules
        .iter()
        .zip(&checked)
        .zip(&deps)
        .map(|((module, checked), deps)| Unit {
            module,
            checked,
            deps,
        })
        .collect();
    let evaluated = cdl_eval::evaluate(&units, entry, &engine.schema, &limits).map_err(sorted)?;
    drop(units);
    Ok(Some(Program {
        modules,
        checked,
        deps,
        entry,
        evaluated,
        schema: engine.schema.clone(),
        limits,
    }))
}

#[cfg(test)]
mod tests {
    use super::valid_specifier;

    #[test]
    fn specifier_forms() {
        for ok in ["./a.cdl", "../a.cdl", "./x/y.cdl", "../../z.cdl"] {
            assert!(valid_specifier(ok), "{ok}");
        }
        for bad in [
            "a.cdl",
            "/a.cdl",
            "./a",
            "./.cdl",
            "https://x/a.cdl",
            "./x/.cdl",
            ".cdl",
        ] {
            assert!(!valid_specifier(bad), "{bad}");
        }
    }
}
