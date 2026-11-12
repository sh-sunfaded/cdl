//! `cdl` コマンドの中身。ファイルシステムのリゾルバ・参照ホスト・値木の JSON を持つ。
#![forbid(unsafe_code)]

pub mod json;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use cdl_host::{
    ComponentDef, Diagnostic, Engine, Field, ModuleResolver, Schema, Source, Stage, Ty,
};

/// 終了コード: 成功・診断あり・使い方の誤り。
pub const EXIT_OK: u8 = 0;
pub const EXIT_DIAGNOSTICS: u8 = 1;
pub const EXIT_USAGE: u8 = 2;

const USAGE: &str =
    "使い方: cdl <check|eval> <file.cdl> [--json] [--preset contrast] [--stage <syntax|sema|eval>]
  check   検査して、診断だけを出す。誤りがあれば終了コード 1
  eval    評価して、値木を JSON で出す
  --json  診断を JSON で出す（既定は人間向け）
  --preset contrast  参照ホスト host:contrast のスキーマを使う（既定。いまはこれだけ）
  --stage  その段階までで止める（check のみ。既定は eval）
  --env    ルートが関数の形のとき、引数 Env の JSON ファイル（eval のみ）";

/// パスを字面のまま正規化する（`.` を除き、`..` を前の要素と打ち消す）。
fn normalize(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match parts.last().map(String::as_str) {
                Some(last) if last != ".." && last != "/" => {
                    parts.pop();
                }
                _ => parts.push("..".to_owned()),
            },
            Component::RootDir => parts.push("/".to_owned()),
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::Prefix(p) => parts.push(p.as_os_str().to_string_lossy().into_owned()),
        }
    }
    let joined = parts.join("/");
    joined
        .strip_prefix("//")
        .map_or(joined.clone(), |rest| format!("/{rest}"))
}

/// ファイルシステムからソースを読むリゾルバ（DECISIONS E6）。`id` は正規化したパス。
/// 読んだソースは、診断の表示のために覚えておく。
#[derive(Default)]
pub struct FsResolver {
    sources: RefCell<BTreeMap<String, String>>,
}

impl FsResolver {
    pub fn new() -> Self {
        FsResolver::default()
    }

    /// 読んだソース（`id` → 本文）。
    pub fn into_sources(self) -> BTreeMap<String, String> {
        self.sources.into_inner()
    }
}

impl ModuleResolver for FsResolver {
    fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        let path: PathBuf = match importer {
            None => PathBuf::from(specifier),
            Some(from) => Path::new(from)
                .parent()
                .unwrap_or(Path::new(""))
                .join(specifier),
        };
        let id = normalize(&path);
        let text = std::fs::read_to_string(&id).map_err(|e| format!("{id}: {e}"))?;
        self.sources.borrow_mut().insert(id.clone(), text.clone());
        Ok(Source { id, text })
    }
}

struct Sources(BTreeMap<String, String>);

impl cdl_host::diag::SourceMap for Sources {
    fn source(&self, file: &str) -> Option<&str> {
        self.0.get(file).map(String::as_str)
    }
}

/// 参照ホスト `host:contrast`（`conformance/HOST.md`。DECISIONS H2）。
pub fn contrast_schema() -> Schema {
    let window_info = Ty::object([
        ("appId", Field::required(Ty::String)),
        ("workspace", Field::required(Ty::Number)),
    ]);
    let placement = Ty::object([
        ("workspace", Field::required(Ty::Number)),
        ("floating", Field::required(Ty::Boolean)),
    ]);
    let policy = Ty::object([(
        "windowOpen",
        Field::optional(Ty::Function(
            vec![window_info.clone()],
            Box::new(placement.clone()),
        )),
    )]);
    let env = Ty::object([(
        "output",
        Field::required(Ty::object([
            ("name", Field::required(Ty::String)),
            ("width", Field::required(Ty::Number)),
            ("height", Field::required(Ty::Number)),
            ("scale", Field::required(Ty::Number)),
        ])),
    )]);
    let m = "host:contrast";
    let built = Schema::builder()
        .root(m, "Contrast")
        .env(m, env)
        .component(
            m,
            ComponentDef::new("Contrast")
                .children(&["Appearance", "Animation", "Keybind", "Label", "Group"])
                .unique_child("Appearance"),
        )
        .component(
            m,
            ComponentDef::new("Appearance")
                .prop("gap", Ty::Length)
                .prop("radius", Ty::Length)
                .optional_prop("focusColor", Ty::Color),
        )
        .component(
            m,
            ComponentDef::new("Animation")
                .prop("duration", Ty::Duration)
                .optional_prop("enabled", Ty::Boolean),
        )
        .component(
            m,
            ComponentDef::new("Keybind")
                .prop("keys", Ty::String)
                .prop("action", Ty::String),
        )
        // テキスト子（`<Label>text</Label>`）を受け付ける唯一のコンポーネント（DECISIONS R1）。
        .component(m, ComponentDef::new("Label").text_children())
        // スロット（要素を渡す props）と、要素・テキストの混在の子を持つコンポーネント（DECISIONS U2）。
        .component(
            m,
            ComponentDef::new("Group")
                .optional_prop("header", Ty::element("Label"))
                .optional_prop("footer", Ty::Array(Box::new(Ty::element("Label"))))
                .children(&["Keybind"])
                .text_children(),
        )
        .type_def(m, "WindowInfo", window_info)
        .type_def(m, "Placement", placement)
        .type_def(m, "Policy", policy)
        .build();
    // 定義は固定なので、組み立ては必ず成功する（テストで確かめる）。
    built.unwrap_or_default()
}

fn report(
    diags: &[Diagnostic],
    sources: &Sources,
    as_json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    if as_json {
        match cdl_host::diag::to_json(diags) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
            }
            Err(e) => {
                let _ = writeln!(err, "診断を JSON にできない: {e}");
            }
        }
    } else {
        for d in diags {
            let _ = writeln!(err, "{}", cdl_host::diag::render_human(d, sources));
        }
    }
    EXIT_DIAGNOSTICS
}

/// コマンドラインの引数。
struct Args<'a> {
    command: &'a str,
    file: &'a str,
    as_json: bool,
    stage: Stage,
    /// `eval` で、関数の形のルートに渡す `Env` の JSON ファイル。
    env: Option<&'a str>,
}

/// 引数を読む。形が合わなければ `None`（使い方の誤り）。
fn parse_args(args: &[String]) -> Option<Args<'_>> {
    let mut positional = Vec::new();
    let mut as_json = false;
    let mut stage = None;
    let mut preset = None;
    let mut env = None;
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "--json" => as_json = true,
            "--stage" if stage.is_none() => stage = Some(it.next()?),
            "--preset" if preset.is_none() => preset = Some(it.next()?),
            "--env" if env.is_none() => env = Some(it.next()?),
            _ if a.starts_with("--") => return None,
            _ => positional.push(a),
        }
    }
    let [command @ ("check" | "eval"), file] = positional.as_slice() else {
        return None;
    };
    if preset.is_some_and(|p| p != "contrast") {
        return None;
    }
    if env.is_some() && *command != "eval" {
        return None;
    }
    let stage = match stage {
        None | Some("eval") => Stage::Eval,
        Some("syntax") if *command == "check" => Stage::Syntax,
        Some("sema") if *command == "check" => Stage::Sema,
        Some(_) => return None,
    };
    Some(Args {
        command,
        file,
        as_json,
        stage,
        env,
    })
}

/// コマンドを実行して、終了コードを返す。
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(Args {
        command,
        file,
        as_json,
        stage,
        env,
    }) = parse_args(args)
    else {
        let _ = writeln!(err, "{USAGE}");
        return EXIT_USAGE;
    };
    let engine = Engine::new().with_schema(contrast_schema());
    let resolver = FsResolver::new();
    let result = if command == "check" {
        engine.check_until(&resolver, file, stage).map(|()| None)
    } else {
        engine.load(&resolver, file).map(Some)
    };
    let sources = Sources(resolver.into_sources());
    match result {
        Err(diags) => report(&diags, &sources, as_json, out, err),
        Ok(None) => EXIT_OK,
        Ok(Some(program)) => {
            let rendered = if program.is_function_root() {
                let Some(path) = env else {
                    let _ = writeln!(
                        err,
                        "ルートが関数の形です。`--env <Env の JSON ファイル>` を付けてください"
                    );
                    return EXIT_USAGE;
                };
                let value = std::fs::read_to_string(path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
                    .ok()
                    .and_then(|j| json::json_value(&j));
                let Some(value) = value else {
                    let _ = writeln!(err, "`--env` のファイルを JSON の値として読めない: {path}");
                    return EXIT_USAGE;
                };
                match program.render(&value) {
                    Ok(el) => el,
                    Err(d) => return report(&d, &sources, as_json, out, err),
                }
            } else {
                match program.render(&cdl_host::Value::Undefined) {
                    Ok(el) => el,
                    Err(d) => return report(&d, &sources, as_json, out, err),
                }
            };
            let tree = json::element_json(&rendered);
            match serde_json::to_string_pretty(&tree) {
                Ok(text) => {
                    let _ = writeln!(out, "{text}");
                    EXIT_OK
                }
                Err(e) => {
                    let _ = writeln!(err, "値木を JSON にできない: {e}");
                    EXIT_DIAGNOSTICS
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_schema_builds() {
        assert!(Schema::builder().build().is_ok());
        assert_ne!(contrast_schema(), Schema::default());
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(normalize(Path::new("a/./b/../c.cdl")), "a/c.cdl");
        assert_eq!(normalize(Path::new("../x/../y.cdl")), "../y.cdl");
        assert_eq!(normalize(Path::new("/abs/./z.cdl")), "/abs/z.cdl");
    }
}
