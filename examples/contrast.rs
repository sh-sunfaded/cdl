//! Contrast（Wayland コンポジタ）が CDL を組み込む例。
//!
//! 1. 組み込みコンポーネント（Contrast / Appearance / Animation / Keybind）とポリシーの型を Rust で登録する。
//! 2. ファイルシステムのリゾルバを用意する（ソースを読むのはホストの仕事。SPEC 第 9 章）。
//! 3. `Program::render` で要素ツリーを作り、`FromNode` / `FromValue`（手書き）で Rust の型に変換する。
//!    変換の失敗は、そのノードの位置つきの診断として返る。ルートが関数の形なら、出力ごとの `Env` で render し直す。
//! 4. ポリシーのフックを呼ぶ。失敗したらホストの既定の動作に戻す。
//!
//! 実行: `cargo run -p cdl-host --example contrast`（または、引数に .cdl のパス）

use std::path::Path;
use std::process::ExitCode;

use cdl_host::{
    ComponentDef, Diagnostic, Engine, Field, FromNode, Millis, ModuleResolver, Node, Program, Px,
    Rgba, Schema, Source, Ty, Value,
};

/// Contrast のスキーマ。
fn schema() -> Result<Schema, Vec<String>> {
    let m = "host:contrast";
    let window_info = Ty::object([
        ("appId", Field::required(Ty::String)),
        ("workspace", Field::required(Ty::Number)),
    ]);
    let placement = Ty::object([
        ("workspace", Field::required(Ty::Number)),
        ("floating", Field::required(Ty::Boolean)),
    ]);
    Schema::builder()
        .root(m, "Contrast")
        .env(
            m,
            Ty::object([(
                "output",
                Field::required(Ty::object([
                    ("name", Field::required(Ty::String)),
                    ("width", Field::required(Ty::Number)),
                    ("height", Field::required(Ty::Number)),
                    ("scale", Field::required(Ty::Number)),
                ])),
            )]),
        )
        .component(
            m,
            ComponentDef::new("Contrast")
                .children(&["Appearance", "Animation", "Keybind"])
                .unique_child("Appearance")
                .unique_child("Animation"),
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
        .type_def(m, "WindowInfo", window_info.clone())
        .type_def(m, "Placement", placement.clone())
        .type_def(
            m,
            "Policy",
            Ty::object([(
                "windowOpen",
                Field::optional(Ty::Function(vec![window_info], Box::new(placement))),
            )]),
        )
        .build()
}

/// ファイルシステムのリゾルバ。`id` は正規化した絶対パス。
struct Files;

impl ModuleResolver for Files {
    fn resolve(&self, importer: Option<&str>, specifier: &str) -> Result<Source, String> {
        let path = match importer {
            None => Path::new(specifier).to_path_buf(),
            Some(from) => Path::new(from)
                .parent()
                .unwrap_or(Path::new(""))
                .join(specifier),
        };
        // 同じファイルが同じ id になるよう、正規化した絶対パスを id にする。
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(Source {
            id: path.to_string_lossy().into_owned(),
            text,
        })
    }
}

/// Contrast 側の設定の型。`FromNode` を手で実装して、値木の 1 ノードから読む（derive マクロはない）。
/// 失敗は、そのノードの位置つきの診断（E4008）になる。
#[derive(Debug)]
struct Appearance {
    gap: Px,
    radius: Px,
    focus_color: Option<Rgba>,
}

impl FromNode for Appearance {
    const COMPONENT: &'static str = "Appearance";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Appearance {
            gap: node.prop("gap")?,
            radius: node.prop("radius")?,
            // 省略できる prop は `Option<T>` で読む。
            focus_color: node.prop("focusColor")?,
        })
    }
}

#[derive(Debug)]
struct Animation {
    duration: Millis,
    enabled: bool,
}

impl FromNode for Animation {
    const COMPONENT: &'static str = "Animation";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Animation {
            duration: node.prop("duration")?,
            enabled: node.prop::<Option<bool>>("enabled")?.unwrap_or(true),
        })
    }
}

#[derive(Debug)]
struct Keybind {
    keys: String,
    action: String,
}

impl FromNode for Keybind {
    const COMPONENT: &'static str = "Keybind";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Keybind {
            keys: node.prop("keys")?,
            action: node.prop("action")?,
        })
    }
}

#[derive(Debug)]
struct Config {
    appearance: Appearance,
    animation: Animation,
    keybinds: Vec<Keybind>,
}

impl FromNode for Config {
    const COMPONENT: &'static str = "Contrast";

    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic> {
        Ok(Config {
            // 一意な子（スキーマの `unique_child`）。必須なので、なければ診断。
            appearance: node.required_child()?,
            animation: node.required_child()?,
            keybinds: node.children_of()?,
        })
    }
}

/// ポリシーで新しいウィンドウの置き場所を決める。失敗したら既定の動作（いまのワークスペース）。
fn place(program: &Program, app_id: &str, workspace: f64) -> (f64, bool) {
    let arg = Value::object([
        ("appId", Value::string(app_id)),
        ("workspace", Value::Number(workspace)),
    ]);
    match program.call_hook("windowOpen", arg) {
        Ok(Some(Value::Object(p))) => match (p.get("workspace"), p.get("floating")) {
            (Some(Value::Number(w)), Some(Value::Bool(f))) => (*w, *f),
            _ => (workspace, false),
        },
        Ok(_) => (workspace, false),
        Err(d) => {
            eprintln!("ポリシーの誤り（既定の動作に戻す）: {}", d.message);
            (workspace, false)
        }
    }
}

/// `per-output.cdl`（ルートが `Env` を受け取る関数）を、出力ごとの `Env` で render する。
fn per_output() -> Result<(), ExitCode> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/contrast/per-output.cdl"
    );
    let schema = schema().map_err(|_| ExitCode::FAILURE)?;
    let program = Engine::new()
        .with_schema(schema)
        .load(&Files, path)
        .map_err(|d| report(&d))?;
    for (name, scale) in [("DP-1", 2.0), ("HDMI-A-1", 1.0)] {
        let output = Value::object([
            ("name", Value::string(name)),
            ("width", Value::Number(1920.0)),
            ("height", Value::Number(1080.0)),
            ("scale", Value::Number(scale)),
        ]);
        let tree = program
            .render(&Value::object([("output", output)]))
            .map_err(|d| report(&d))?;
        let config = Node::new(&tree)
            .read::<Config>()
            .map_err(|d| report(&[d]))?;
        println!(
            "{name}（scale {scale}）: gap = {}px",
            config.appearance.gap.0
        );
    }
    Ok(())
}

fn report(diags: &[Diagnostic]) -> ExitCode {
    for d in diags {
        eprintln!(
            "{}[{}] {}:{}..{}: {}",
            d.severity.as_str(),
            d.code.as_str(),
            d.span.file,
            d.span.start,
            d.span.end,
            d.message
        );
    }
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let entry = std::env::args().nth(1).unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/contrast/main.cdl"
        )
        .to_owned()
    });
    let schema = match schema() {
        Ok(s) => s,
        Err(errors) => {
            for e in errors {
                eprintln!("スキーマの誤り: {e}");
            }
            return ExitCode::FAILURE;
        }
    };
    let program = match Engine::new().with_schema(schema).load(&Files, &entry) {
        Ok(p) => p,
        Err(diags) => return report(&diags),
    };
    let tree = match program.render(&Value::Undefined) {
        Ok(t) => t,
        Err(d) => return report(&d),
    };
    let config = match Node::new(&tree).read::<Config>() {
        Ok(c) => c,
        Err(d) => return report(&[d]),
    };
    println!(
        "読み込んだファイル: {}",
        program.files().collect::<Vec<_>>().join(", ")
    );
    let Appearance {
        gap,
        radius,
        focus_color,
    } = &config.appearance;
    println!("gap = {}px, radius = {}px", gap.0, radius.0);
    if let Some(Rgba([r, g, b, a])) = focus_color {
        println!("focusColor = rgba({r}, {g}, {b}, {a})");
    }
    println!(
        "animation = {}ms（{}）",
        config.animation.duration.0,
        if config.animation.enabled {
            "有効"
        } else {
            "無効"
        }
    );
    for Keybind { keys, action } in &config.keybinds {
        println!("keybind: {keys} → {action}");
    }
    // ルートが関数の形の設定は、出力（モニタ）ごとに render し直す（UI = f(env)）。
    if let Err(code) = per_output() {
        return code;
    }
    for app in ["firefox", "pavucontrol", "kitty"] {
        let (workspace, floating) = place(&program, app, 3.0);
        println!(
            "windowOpen({app}) → workspace {workspace}{}",
            if floating {
                "（フローティング）"
            } else {
                ""
            }
        );
    }
    ExitCode::SUCCESS
}
