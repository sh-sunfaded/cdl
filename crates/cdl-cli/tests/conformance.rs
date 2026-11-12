//! 適合テストスイート（`conformance/`）のランナー。`cdl-host` の公開 API だけを使う。
//!
//! - 診断は `code`・`severity`・`span` だけを比べる（メッセージは実装ごとに違ってよい）。
//! - 値木は DECISIONS D7 の JSON の形にして比べる（span は比べない）。
//! - `.calls.json` があれば、フックを呼んで戻り値か診断のコードを比べる（SPEC 第 8 章）。
//! - 決定性: 各テストを 2 回実行し、すべての結果が完全に一致することを確かめる（SPEC 第 12 章）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cdl_cli::contrast_schema;
use cdl_cli::json::{element_json, json_value, value_json};
use cdl_host::diag::Diagnostic;
use cdl_host::{Engine, ModuleResolver, Program, Source};
use serde_json::Value;

/// 1 件のテスト。
struct Case {
    name: String,
    /// モジュールの `id` → ソース。
    files: BTreeMap<String, String>,
    entry: String,
    expected: PathBuf,
    calls: Option<PathBuf>,
    /// 関数の形のルートに渡す `Env`（`<名前>.env.json` / `env.json`）。
    env: Option<PathBuf>,
}

/// 比べる部分だけを取り出した診断。
#[derive(Debug, PartialEq, Eq)]
struct Key {
    code: String,
    severity: String,
    file: String,
    start: u64,
    end: u64,
}

enum Expected {
    Value(Value),
    Diagnostics(Vec<Key>),
}

struct Memory<'a>(&'a BTreeMap<String, String>);

impl ModuleResolver for Memory<'_> {
    /// `id` はテストのディレクトリからの相対パス。`..` でディレクトリの外へは出られない。
    fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        let mut parts: Vec<&str> = match importer {
            Some(from) => {
                let mut p: Vec<&str> = from.split('/').collect();
                p.pop();
                p
            }
            None => Vec::new(),
        };
        for seg in specifier.split('/') {
            match seg {
                "." | "" => {}
                ".." => {
                    parts
                        .pop()
                        .ok_or_else(|| "テストのディレクトリの外は参照できない".to_owned())?;
                }
                s => parts.push(s),
            }
        }
        let id = parts.join("/");
        self.0
            .get(&id)
            .map(|text| Source {
                id: id.clone(),
                text: text.clone(),
            })
            .ok_or_else(|| "スイートにないファイル".to_owned())
    }
}

fn suite_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

/// 節のディレクトリ名か（章番号 1〜2 桁、または禁止構文の `forbidden`）。
fn valid_section(section: &str) -> bool {
    section == "forbidden"
        || (!section.is_empty()
            && section.len() <= 2
            && section.bytes().all(|b| b.is_ascii_digit()))
}

/// テスト名（ASCII の英小文字・数字・`-`）か。
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// ディレクトリの中の `.cdl` を、ディレクトリからの相対パスを `id` にして読む。
fn read_dir_files(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let id = format!("{prefix}{name}");
        if path.is_dir() {
            read_dir_files(&path, &format!("{id}/"), out);
        } else if name.ends_with(".cdl") {
            out.insert(id, fs::read_to_string(&path).unwrap());
        }
    }
}

/// 節のディレクトリ（`<節>/`）を走査して、テストを集める。組になっていないファイルは誤りとして集める。
///
/// - 1 ファイルのテスト: `<節>/<名前>.cdl` と `<節>/<名前>.expected.json`（`<名前>.calls.json` は任意）。
///   モジュールの `id` は、スイートのルートからの相対パス（`3/comments.cdl`）。
/// - 複数ファイルのテスト: `<節>/<名前>/` に、`main.cdl` と `expected.json`（`calls.json` は任意）。
///   `id` は、そのディレクトリからの相対パス。
fn discover_section(root: &Path, section: &str, cases: &mut Vec<Case>, errors: &mut Vec<String>) {
    let dir = root.join(section);
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in &entries {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let label = format!("{section}/{file_name}");
        if path.is_dir() {
            let expected = path.join("expected.json");
            let mut files = BTreeMap::new();
            read_dir_files(path, "", &mut files);
            if !files.contains_key("main.cdl") || !expected.is_file() {
                errors.push(format!("{label}/: main.cdl と expected.json が要る"));
                continue;
            }
            let calls = Some(path.join("calls.json")).filter(|p| p.is_file());
            let env = Some(path.join("env.json")).filter(|p| p.is_file());
            cases.push(Case {
                name: label,
                files,
                entry: "main.cdl".to_owned(),
                expected,
                calls,
                env,
            });
        } else if let Some(stem) = file_name.strip_suffix(".cdl") {
            let expected = path.with_file_name(format!("{stem}.expected.json"));
            if !expected.is_file() {
                errors.push(format!("{label}: {stem}.expected.json がない"));
                continue;
            }
            let id = format!("{section}/{file_name}");
            let files = BTreeMap::from([(id.clone(), fs::read_to_string(path).unwrap())]);
            let calls =
                Some(path.with_file_name(format!("{stem}.calls.json"))).filter(|p| p.is_file());
            let env = Some(path.with_file_name(format!("{stem}.env.json"))).filter(|p| p.is_file());
            cases.push(Case {
                name: format!("{section}/{stem}"),
                files,
                entry: id,
                expected,
                calls,
                env,
            });
        } else if let Some(stem) = file_name
            .strip_suffix(".calls.json")
            .or_else(|| file_name.strip_suffix(".env.json"))
            .or_else(|| file_name.strip_suffix(".expected.json"))
        {
            if !path.with_file_name(format!("{stem}.cdl")).is_file() {
                errors.push(format!("{label}: {stem}.cdl がない"));
            }
        } else {
            errors.push(format!("{label}: スイートに置けないファイル"));
        }
    }
}

/// スイートを走査する。
fn discover(root: &Path) -> (Vec<Case>, Vec<String>) {
    let mut cases = Vec::new();
    let mut errors = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in &entries {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if path.is_dir() {
            // `tools/` はテストではなく、スイートを生成するスクリプトの置き場。
            if file_name == "tools" {
                continue;
            }
            if valid_section(&file_name) {
                discover_section(root, &file_name, &mut cases, &mut errors);
            } else {
                errors.push(format!(
                    "{file_name}/: 節のディレクトリは章番号か forbidden にする"
                ));
            }
        } else if !matches!(file_name.as_str(), "README.md" | "HOST.md") {
            errors.push(format!("{file_name}: スイートに置けないファイル"));
        }
    }
    for case in &cases {
        let name = case.name.rsplit('/').next().unwrap_or(&case.name);
        if !valid_name(name) {
            errors.push(format!("{}: 名前は英小文字・数字・- にする", case.name));
        }
    }
    (cases, errors)
}

fn key_of_expected(v: &Value) -> Option<Key> {
    let span = v.get("span")?;
    Some(Key {
        code: v.get("code")?.as_str()?.to_owned(),
        severity: v.get("severity")?.as_str()?.to_owned(),
        file: span.get("file")?.as_str()?.to_owned(),
        start: span.get("start")?.as_u64()?,
        end: span.get("end")?.as_u64()?,
    })
}

fn key_of(d: &Diagnostic) -> Key {
    Key {
        code: d.code.as_str().to_owned(),
        severity: d.severity.as_str().to_owned(),
        file: d.span.file.clone(),
        start: u64::from(d.span.start),
        end: u64::from(d.span.end),
    }
}

/// 期待結果は `{"value": …}` か `{"diagnostics": […]}` のどちらか。
fn read_expected(case: &Case) -> Result<Expected, String> {
    let text = fs::read_to_string(&case.expected).map_err(|e| format!("{}: {e}", case.name))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", case.name))?;
    let object = value.as_object().filter(|o| o.len() == 1);
    match object.and_then(|o| o.iter().next()) {
        Some((key, v)) if key == "value" => Ok(Expected::Value(v.clone())),
        Some((key, Value::Array(items))) if key == "diagnostics" => items
            .iter()
            .map(|item| {
                key_of_expected(item).ok_or_else(|| {
                    format!(
                        "{}: 診断には code・severity・span が要る: {item}",
                        case.name
                    )
                })
            })
            .collect::<Result<_, _>>()
            .map(Expected::Diagnostics),
        _ => Err(format!(
            "{}: キーは value か diagnostics（配列）の 1 つだけ",
            case.name
        )),
    }
}

/// 数を f64 にそろえる（`16` と `16.0` を同じに扱う）。
fn normalize(v: Value) -> Value {
    match v {
        Value::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Value::Array(xs) => Value::Array(xs.into_iter().map(normalize).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, normalize(v))).collect()),
        other => other,
    }
}

/// 1 回の実行の結果。2 回の実行で完全に一致しなければならない。
#[derive(Debug, PartialEq)]
struct Outcome {
    result: Result<Value, Vec<Key>>,
    calls: Vec<Result<Value, String>>,
}

fn run(case: &Case) -> Outcome {
    let engine = Engine::new().with_schema(contrast_schema());
    let program: Result<Program, _> = engine.load(&Memory(&case.files), &case.entry);
    let calls_spec: Vec<Value> = case
        .calls
        .as_ref()
        .map(|p| serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap())
        .unwrap_or_default();
    match program {
        Err(diags) => Outcome {
            result: Err(diags.iter().map(key_of).collect()),
            calls: Vec::new(),
        },
        Ok(program) => {
            let calls = calls_spec
                .iter()
                .map(|c| {
                    let hook = c["hook"].as_str().unwrap();
                    match program.call_hook(hook, json_value(&c["arg"]).unwrap()) {
                        Ok(Some(v)) => Ok(normalize(value_json(&v))),
                        Ok(None) => Ok(Value::Null),
                        Err(d) => Err(d.code.as_str().to_owned()),
                    }
                })
                .collect();
            let env = case
                .env
                .as_ref()
                .map(|p| {
                    json_value(&serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap())
                        .unwrap()
                })
                .unwrap_or(cdl_host::Value::Undefined);
            let result = match program.render(&env) {
                Ok(el) => Ok(normalize(element_json(&el))),
                Err(ds) => Err(ds.iter().map(key_of).collect()),
            };
            Outcome { result, calls }
        }
    }
}

fn check_case(case: &Case) -> Result<(), String> {
    let expected = read_expected(case)?;
    let first = run(case);
    if first != run(case) {
        return Err(format!("{}: 2 回の実行で結果が違う", case.name));
    }
    match (&expected, &first.result) {
        (Expected::Value(want), Ok(got)) if normalize(want.clone()) == *got => {}
        (Expected::Diagnostics(want), Err(got)) if want == got => {}
        (Expected::Value(want), got) => {
            return Err(format!("{}:\n  期待: {want}\n  実際: {got:?}", case.name));
        }
        (Expected::Diagnostics(want), got) => {
            return Err(format!("{}:\n  期待: {want:?}\n  実際: {got:?}", case.name));
        }
    }
    let Some(path) = &case.calls else {
        return Ok(());
    };
    let spec: Vec<Value> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    for (i, (c, got)) in spec.iter().zip(&first.calls).enumerate() {
        let want = match (c.get("result"), c.get("error")) {
            (Some(r), None) => Ok(normalize(r.clone())),
            (None, Some(e)) => Err(e.as_str().unwrap().to_owned()),
            _ => {
                return Err(format!(
                    "{}: 呼び出し {i} には result か error の一方が要る",
                    case.name
                ));
            }
        };
        if want != *got {
            return Err(format!(
                "{}: 呼び出し {i}:\n  期待: {want:?}\n  実際: {got:?}",
                case.name
            ));
        }
    }
    if spec.len() != first.calls.len() {
        return Err(format!("{}: 呼び出しの数が違う", case.name));
    }
    Ok(())
}

#[test]
fn conformance_suite() {
    let (cases, mut errors) = discover(&suite_root());
    errors.extend(cases.iter().filter_map(|c| check_case(c).err()));
    assert!(
        errors.is_empty(),
        "適合テストの失敗（{} 件中）:\n{}",
        cases.len(),
        errors.join("\n")
    );
}

#[test]
fn names_follow_the_convention() {
    assert!(valid_section("5"));
    assert!(valid_section("14"));
    assert!(valid_section("forbidden"));
    assert!(!valid_section("123"));
    assert!(!valid_section("tools"));
    assert!(!valid_section(""));
    assert!(valid_name("switch-exhaustive"));
    assert!(valid_name("determinism"));
    assert!(!valid_name(""));
    assert!(!valid_name("Switch"));
    assert!(!valid_name("-a"));
    assert!(!valid_name("a-"));
    assert!(!valid_name("a--b"));
}

/// 禁止構文の id（CLAUDE.md）が、すべて `forbidden/` に置いてある。
#[test]
fn every_forbidden_id_has_a_case() {
    const IDS: [&str; 18] = [
        "let",
        "var",
        "class",
        "new",
        "this",
        "null",
        "for",
        "while",
        "try",
        "async",
        "enum",
        "as",
        "hooks",
        "event-handler",
        "ref",
        "destructuring",
        "rest-param",
        "and-nonboolean",
    ];
    let dir = suite_root().join("forbidden");
    for id in IDS {
        assert!(
            dir.join(format!("{id}.cdl")).is_file(),
            "forbidden/{id}.cdl"
        );
        assert!(
            dir.join(format!("{id}.expected.json")).is_file(),
            "forbidden/{id}.expected.json"
        );
    }
}
