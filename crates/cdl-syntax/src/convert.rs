//! oxc の AST を CDL の AST に変換し、CDL にない構文を診断する（SPEC 第 3・5 章、DECISIONS C1–C5）。
//!
//! 誤りを見つけても、その項目を捨てて次の項目へ進む（1 回の実行で複数の誤りを報告する）。

use cdl_diag::{Code, Diagnostic};
use oxc_ast::ast as o;
use oxc_span::GetSpan;

use crate::ast::*;

/// 束縛の名前に使えない語（SPEC 第 3 章の予約語と、CDL のキーワード。DECISIONS C3）。
const RESERVED: &[&str] = &[
    "let",
    "var",
    "class",
    "new",
    "this",
    "null",
    "for",
    "while",
    "do",
    "break",
    "continue",
    "async",
    "await",
    "yield",
    "try",
    "catch",
    "throw",
    "enum",
    "namespace",
    "as",
    "const",
    "function",
    "return",
    "if",
    "else",
    "switch",
    "case",
    "default",
    "type",
    "interface",
    "import",
    "export",
    "from",
    "satisfies",
    "true",
    "false",
    "undefined",
];

/// `use[A-Z0-9_]` で始まる名前（React の Hooks の命名。DECISIONS C4）。
fn is_hook_name(name: &str) -> bool {
    name.strip_prefix("use")
        .and_then(|rest| rest.bytes().next())
        .is_some_and(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

const HOOK_HELP: &str = "状態は持てない。値は `const` で宣言する";
const EDITION_HELP: &str = "ファイルの最初に `\"use cdl 1\";` と書く";

/// 予約語ごとの、代わりの書き方。
fn reserved_help(word: &str) -> Option<&'static str> {
    Some(match word {
        "let" | "var" => "`const` を使う（束縛は再代入できない）",
        "null" => "値がないことは `undefined` で表す",
        "for" | "while" | "do" => "繰り返しは配列の `map`・`filter`・`reduce` で書く",
        "break" | "continue" => "`switch` の `case` は `return` で終える",
        "class" => "関数とオブジェクトで書く",
        "this" => "引数（コンポーネントなら `props`）を使う",
        "new" => "値は組み込み関数（`px()` など）かリテラルで作る",
        "enum" => "文字列リテラルのユニオン型（`\"a\" | \"b\"`）を使う",
        "namespace" => "ファイルを分けて `import` する",
        "as" => "型注釈か `satisfies` を使う",
        "async" | "await" => "評価は同期的に終わる。`async` を外す",
        "try" | "throw" => "失敗しうる値は `T | undefined` で表す",
        _ => return None,
    })
}

/// `on[A-Z]` で始まる属性名（イベントハンドラ。DECISIONS C4）。
fn is_event_attr(name: &str) -> bool {
    name.strip_prefix("on")
        .and_then(|rest| rest.bytes().next())
        .is_some_and(|b| b.is_ascii_uppercase())
}

fn is_jsx_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r')
}

/// テキスト子の空白の規則（DECISIONS U5。JSX の標準＝Babel / TypeScript と同じ。ただし空白だけのテキストは常に捨てる）:
/// 行に分け、各行の前後の空白を除く（最初の行の先頭と最後の行の末尾の空白は残す）、空行を捨て、
/// 残りを 1 つの空白でつなぐ。単一行のテキストの前後の空白は残る。空白だけのテキストは `None`（子にしない）。
pub(crate) fn normalize_jsx_text(raw: &str) -> Option<String> {
    if raw.chars().all(|c| is_jsx_space(c) || c == '\n') {
        return None;
    }
    let all: Vec<&str> = raw.split('\n').collect();
    let last = all.len().saturating_sub(1);
    let lines: Vec<&str> = all
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line = if i == 0 {
                line
            } else {
                line.trim_start_matches(is_jsx_space)
            };
            if i == last {
                line
            } else {
                line.trim_end_matches(is_jsx_space)
            }
        })
        .filter(|l| !l.is_empty())
        .collect();
    Some(lines.join(" "))
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

fn sp(span: oxc_span::Span) -> Span {
    Span {
        start: span.start,
        end: span.end,
    }
}

pub(crate) struct Converter<'s> {
    file: &'s str,
    src: &'s str,
    diags: Vec<Diagnostic>,
    depth: u32,
    max_depth: u32,
}

impl<'s> Converter<'s> {
    pub(crate) fn new(file: &'s str, src: &'s str, max_depth: u32) -> Self {
        Converter {
            file,
            src,
            diags: Vec::new(),
            depth: 0,
            max_depth,
        }
    }

    pub(crate) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diags
    }

    fn make(&self, code: Code, span: Span, message: impl Into<String>) -> Diagnostic {
        let span = cdl_diag::Span {
            file: self.file.to_owned(),
            start: span.start,
            end: span.end,
        };
        Diagnostic::error(code, span, message)
    }

    fn diag(&mut self, code: Code, span: Span, message: impl Into<String>) {
        let d = self.make(code, span, message);
        self.diags.push(d);
    }

    fn diag_help(&mut self, code: Code, span: Span, message: impl Into<String>, help: &str) {
        let d = self.make(code, span, message).with_help(help);
        self.diags.push(d);
    }

    fn err<T>(&mut self, code: Code, span: Span, message: impl Into<String>) -> Option<T> {
        self.diag(code, span, message);
        None
    }

    fn unsupported<T>(&mut self, span: Span, what: &str) -> Option<T> {
        self.err(
            Code::UnsupportedSyntax,
            span,
            format!("{what}は CDL では書けない"),
        )
    }

    fn err_help<T>(
        &mut self,
        code: Code,
        span: Span,
        message: impl Into<String>,
        help: &str,
    ) -> Option<T> {
        self.diag_help(code, span, message, help);
        None
    }

    fn unsupported_help<T>(&mut self, span: Span, what: &str, help: &str) -> Option<T> {
        self.err_help(
            Code::UnsupportedSyntax,
            span,
            format!("{what}は CDL では書けない"),
            help,
        )
    }

    fn reserved<T>(&mut self, span: Span, word: &str) -> Option<T> {
        let message = format!("`{word}` は CDL にない（予約語）");
        match reserved_help(word) {
            Some(help) => self.err_help(Code::ReservedWord, span, message, help),
            None => self.err(Code::ReservedWord, span, message),
        }
    }

    /// 再帰の深さを数える。自前の字句検査の上限を抜けた入力に対する多重防御（DECISIONS C10）。
    fn enter(&mut self, span: Span) -> bool {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.max_depth {
            self.diag(Code::NestingTooDeep, span, "構文の入れ子が深すぎる");
            return false;
        }
        true
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// 文が `;` で終わっているか（SPEC 第 3 章）。oxc の文の span は、あれば `;` を含む。
    fn semicolon(&mut self, span: oxc_span::Span) {
        let end = usize::try_from(span.end).unwrap_or(usize::MAX);
        let last = end.checked_sub(1).and_then(|i| self.src.as_bytes().get(i));
        if last != Some(&b';') {
            let at = Span {
                start: span.end,
                end: span.end,
            };
            self.diag_help(
                Code::MissingSemicolon,
                at,
                "文末に `;` がない",
                "`;` を足す",
            );
        }
    }

    fn binding(&mut self, name: &str, span: oxc_span::Span) -> Option<Ident> {
        let span = sp(span);
        if RESERVED.contains(&name) {
            return self.err_help(
                Code::ReservedWord,
                span,
                format!("`{name}` は予約語なので名前に使えない"),
                "別の名前を使う",
            );
        }
        if is_hook_name(name) {
            return self.err_help(
                Code::ReactFeature,
                span,
                format!("`{name}`: `use` で始まる名前（React の Hooks）は使えない"),
                HOOK_HELP,
            );
        }
        Some(Ident {
            name: name.to_owned(),
            span,
        })
    }

    /// oxc の文字列の値。サロゲートを含むと `None` になる。
    fn js_str(&mut self, value: Option<&str>, span: Span) -> Option<String> {
        match value {
            Some(s) => Some(s.to_owned()),
            None => self.err(Code::InvalidEscape, span, "文字列にサロゲートは書けない"),
        }
    }

    fn number(&mut self, value: f64, span: Span) -> Option<f64> {
        if value.is_finite() {
            Some(value)
        } else {
            self.err(
                Code::NonFiniteNumber,
                span,
                "数値リテラルが有限の値にならない",
            )
        }
    }

    // ---- ファイル -------------------------------------------------------

    pub(crate) fn module(&mut self, program: &o::Program<'_>) -> Option<Module> {
        if let Some(h) = &program.hashbang {
            self.unsupported::<()>(sp(h.span), "`#!` の行");
        }
        let edition = self.edition(program);
        let mut items = Vec::new();
        for stmt in &program.body {
            if let Some(item) = self.item(stmt) {
                items.push(item);
            }
        }
        Some(Module {
            file: self.file.to_owned(),
            edition: edition?,
            items,
            span: sp(program.span),
        })
    }

    /// `"use cdl N";`（DECISIONS C8）。
    fn edition(&mut self, program: &o::Program<'_>) -> Option<u32> {
        let mut directives = program.directives.iter();
        let Some(first) = directives.next() else {
            let at = Span { start: 0, end: 0 };
            self.diag_help(
                Code::InvalidEdition,
                at,
                "ファイルの最初の文に edition 宣言がない",
                EDITION_HELP,
            );
            return None;
        };
        for extra in directives {
            self.diag(
                Code::InvalidEdition,
                sp(extra.span),
                "edition 宣言はファイルの最初に 1 回だけ置く",
            );
        }
        self.semicolon(first.span);
        let span = sp(first.span);
        let digits = first.directive.as_str().strip_prefix("use cdl ");
        let well_formed = digits.filter(|d| {
            !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && !d.starts_with('0')
        });
        let Some(digits) = well_formed else {
            self.diag_help(
                Code::InvalidEdition,
                span,
                "edition 宣言の形が違う",
                EDITION_HELP,
            );
            return None;
        };
        match digits.parse::<u32>() {
            Ok(1) => Some(1),
            _ => self.err(
                Code::UnsupportedEdition,
                span,
                format!("edition {digits} には対応していない（対応: 1）"),
            ),
        }
    }

    fn item(&mut self, stmt: &o::Statement<'_>) -> Option<Item> {
        let span = sp(stmt.span());
        let kind = match stmt {
            o::Statement::ImportDeclaration(d) => ItemKind::Import(self.import(d)?),
            o::Statement::ExportDeclaration(d) => {
                let kind = self.declaration(&d.declaration, d.span)?;
                return Some(Item {
                    exported: true,
                    kind,
                    span,
                });
            }
            o::Statement::ExportDefaultDeclaration(d) => {
                if let o::ExportDefaultDeclarationKind::FunctionDeclaration(f) = &d.declaration {
                    // `export default function App(…): R { … }` は、名前を持たないアロー関数と同じ
                    // （ルート関数。DECISIONS U1）。名前は束縛しない。
                    let (_, params, ret, body) = self.function_parts(f, false)?;
                    let expr = Expr {
                        kind: ExprKind::Arrow {
                            params,
                            ret,
                            body: ArrowBody::Block(body),
                        },
                        span: sp(f.span),
                    };
                    return Some(Item {
                        exported: true,
                        kind: ItemKind::ExportDefault(expr),
                        span,
                    });
                }
                let Some(expr) = d.declaration.as_expression() else {
                    return self.unsupported(span, "`export default` に、関数以外の宣言を置くこと");
                };
                self.semicolon(d.span);
                return Some(Item {
                    exported: true,
                    kind: ItemKind::ExportDefault(self.expr(expr)?),
                    span,
                });
            }
            o::Statement::ExportNamedDeclaration(_) | o::Statement::ExportAllDeclaration(_) => {
                return self.unsupported_help(
                    span,
                    "`export { … }` と再エクスポート",
                    "宣言に `export` を付ける（`export const a = …;`）",
                );
            }
            o::Statement::VariableDeclaration(d) => ItemKind::Const(self.const_decl(d)?),
            o::Statement::FunctionDeclaration(f) => ItemKind::Function(self.function(f)?),
            o::Statement::TSTypeAliasDeclaration(d) => {
                self.semicolon(d.span);
                ItemKind::TypeAlias(self.type_alias(d)?)
            }
            o::Statement::TSInterfaceDeclaration(d) => ItemKind::Interface(self.interface(d)?),
            other => return self.unsupported_stmt(other),
        };
        Some(Item {
            exported: false,
            kind,
            span,
        })
    }

    /// `export` のあとの宣言。`span` は `export` を含む文全体。
    fn declaration(&mut self, d: &o::Declaration<'_>, span: oxc_span::Span) -> Option<ItemKind> {
        Some(match d {
            o::Declaration::VariableDeclaration(v) => {
                // `export const` の `;` は文全体の末尾にある。
                let decl = self.const_decl_inner(v)?;
                self.semicolon(span);
                ItemKind::Const(decl)
            }
            o::Declaration::FunctionDeclaration(f) => ItemKind::Function(self.function(f)?),
            o::Declaration::TSTypeAliasDeclaration(t) => {
                self.semicolon(span);
                ItemKind::TypeAlias(self.type_alias(t)?)
            }
            o::Declaration::TSInterfaceDeclaration(i) => ItemKind::Interface(self.interface(i)?),
            o::Declaration::ClassDeclaration(c) => return self.reserved(sp(c.span), "class"),
            o::Declaration::TSEnumDeclaration(e) => return self.reserved(sp(e.span), "enum"),
            o::Declaration::TSNamespaceDeclaration(n) => {
                return self.reserved(sp(n.span), "namespace");
            }
            other => return self.unsupported(sp(other.span()), "この宣言"),
        })
    }

    /// トップレベルとブロックで共通の、CDL にない文。
    fn unsupported_stmt<T>(&mut self, stmt: &o::Statement<'_>) -> Option<T> {
        let span = sp(stmt.span());
        match stmt {
            o::Statement::ClassDeclaration(_) => self.reserved(span, "class"),
            o::Statement::TSEnumDeclaration(_) => self.reserved(span, "enum"),
            o::Statement::TSNamespaceDeclaration(_) => self.reserved(span, "namespace"),
            o::Statement::ForStatement(_)
            | o::Statement::ForInStatement(_)
            | o::Statement::ForOfStatement(_) => self.reserved(span, "for"),
            o::Statement::WhileStatement(_) => self.reserved(span, "while"),
            o::Statement::DoWhileStatement(_) => self.reserved(span, "do"),
            o::Statement::BreakStatement(_) => self.reserved(span, "break"),
            o::Statement::ContinueStatement(_) => self.reserved(span, "continue"),
            o::Statement::ThrowStatement(_) => self.reserved(span, "throw"),
            o::Statement::TryStatement(_) => self.reserved(span, "try"),
            o::Statement::ExpressionStatement(_) => self.unsupported(span, "式だけの文"),
            o::Statement::EmptyStatement(_) => self.unsupported(span, "空の文（余分な `;`）"),
            o::Statement::BlockStatement(_) => self.unsupported(span, "`{ … }` だけのブロック文"),
            o::Statement::ReturnStatement(_)
            | o::Statement::IfStatement(_)
            | o::Statement::SwitchStatement(_) => {
                self.unsupported(span, "関数の外の `return`・`if`・`switch`")
            }
            _ => self.unsupported(span, "この文"),
        }
    }

    fn import(&mut self, d: &o::ImportDeclaration<'_>) -> Option<Import> {
        self.semicolon(d.span);
        let span = sp(d.span);
        if d.phase.is_some() || d.with_clause.is_some() {
            return self.unsupported(span, "import の属性・フェーズ");
        }
        let Some(specifiers) = &d.specifiers else {
            return self.unsupported(span, "名前を取り込まない import");
        };
        if specifiers.is_empty() {
            return self.unsupported(span, "空の import");
        }
        let mut names = Vec::new();
        let mut ok = true;
        for spec in specifiers {
            let o::ImportDeclarationSpecifier::ImportSpecifier(s) = spec else {
                ok = false;
                self.unsupported_help::<()>(
                    sp(spec.span()),
                    "既定の import と名前空間の import",
                    "`import { 名前 } from \"…\";` と書く",
                );
                continue;
            };
            if s.import_kind == o::ImportOrExportKind::Type {
                ok = false;
                self.unsupported::<()>(
                    sp(s.span),
                    "名前ごとの `type`（`import type { … }` と書く）",
                );
                continue;
            }
            let o::ModuleExportName::IdentifierName(imported) = &s.imported else {
                ok = false;
                self.unsupported::<()>(sp(s.span), "文字列の名前の import");
                continue;
            };
            if imported.span != s.local.span {
                ok = false;
                self.err_help::<()>(
                    Code::ReservedWord,
                    sp(s.span),
                    "import の改名（`as`）はできない",
                    "名前を変えずに `import { 名前 }` と書く",
                );
                continue;
            }
            match self.binding(s.local.name.as_str(), s.local.span) {
                Some(name) => names.push(name),
                None => ok = false,
            }
        }
        let source = StrLit {
            value: self.js_str(d.source.value.as_str(), sp(d.source.span))?,
            span: sp(d.source.span),
        };
        ok.then_some(Import {
            type_only: d.import_kind == o::ImportOrExportKind::Type,
            names,
            source,
            span: sp(d.span),
        })
    }

    fn const_decl(&mut self, d: &o::VariableDeclaration<'_>) -> Option<ConstDecl> {
        let decl = self.const_decl_inner(d)?;
        self.semicolon(d.span);
        Some(decl)
    }

    fn const_decl_inner(&mut self, d: &o::VariableDeclaration<'_>) -> Option<ConstDecl> {
        let span = sp(d.span);
        match d.kind {
            o::VariableDeclarationKind::Const => {}
            o::VariableDeclarationKind::Let => return self.reserved(span, "let"),
            o::VariableDeclarationKind::Var => return self.reserved(span, "var"),
            _ => return self.unsupported(span, "`using`"),
        }
        if d.declare {
            return self.unsupported(span, "`declare`");
        }
        if d.declarations.len() != 1 {
            return self.unsupported_help(
                span,
                "1 つの `const` で複数の名前を宣言すること",
                "名前ごとに `const` で宣言する",
            );
        }
        let decl = d.declarations.first()?;
        let o::BindingPattern::BindingIdentifier(id) = &decl.id else {
            return self.unsupported_help(
                span,
                "分割代入",
                "`const x = o.x;` のように 1 つずつ取り出す",
            );
        };
        let name = self.binding(id.name.as_str(), id.span);
        if decl.definite {
            return self.unsupported(span, "`!` つきの宣言");
        }
        let ty = match &decl.type_annotation {
            Some(t) => Some(self.ty(&t.type_annotation)?),
            None => None,
        };
        let Some(init) = &decl.init else {
            return self.unsupported(span, "初期化式のない `const`");
        };
        let init = self.expr(init)?;
        Some(ConstDecl {
            name: name?,
            ty,
            init,
            span,
        })
    }

    fn type_alias(&mut self, d: &o::TSTypeAliasDeclaration<'_>) -> Option<TypeAlias> {
        if d.type_parameters.is_some() {
            return self.unsupported(sp(d.span), "型引数をとる型");
        }
        if d.declare {
            return self.unsupported(sp(d.span), "`declare`");
        }
        let name = self.binding(d.id.name.as_str(), d.id.span);
        let ty = self.ty(&d.type_annotation)?;
        Some(TypeAlias {
            name: name?,
            ty,
            span: sp(d.span),
        })
    }

    fn interface(&mut self, d: &o::TSInterfaceDeclaration<'_>) -> Option<Interface> {
        let span = sp(d.span);
        if d.type_parameters.is_some() {
            return self.unsupported(span, "型引数をとる interface");
        }
        if !d.extends.is_empty() {
            return self.unsupported(span, "`interface … extends`");
        }
        if d.declare {
            return self.unsupported(span, "`declare`");
        }
        let name = self.binding(d.id.name.as_str(), d.id.span);
        let members = self.members(&d.body.body)?;
        Some(Interface {
            name: name?,
            members,
            span,
        })
    }

    fn members(&mut self, sigs: &[o::TSSignature<'_>]) -> Option<Vec<PropSig>> {
        let mut members = Vec::new();
        let mut ok = true;
        for sig in sigs {
            match self.member(sig) {
                Some(m) => members.push(m),
                None => ok = false,
            }
        }
        ok.then_some(members)
    }

    fn member(&mut self, sig: &o::TSSignature<'_>) -> Option<PropSig> {
        let o::TSSignature::TSPropertySignature(p) = sig else {
            return self.unsupported(sp(sig.span()), "メソッド・呼び出し・添字のシグネチャ");
        };
        let span = sp(p.span);
        if p.computed {
            return self.unsupported(span, "計算プロパティ名");
        }
        if p.readonly {
            return self.unsupported(span, "`readonly`");
        }
        let key = self.prop_key(&p.key)?;
        let Some(t) = &p.type_annotation else {
            return self.unsupported(span, "型注釈のないプロパティ");
        };
        Some(PropSig {
            key,
            optional: p.optional,
            ty: self.ty(&t.type_annotation)?,
            span,
        })
    }

    /// プロパティ名（識別子か文字列）。キーワードも使える（DECISIONS C3）。
    fn prop_key(&mut self, key: &o::PropertyKey<'_>) -> Option<Ident> {
        match key {
            o::PropertyKey::StaticIdentifier(id) => Some(Ident {
                name: id.name.as_str().to_owned(),
                span: sp(id.span),
            }),
            o::PropertyKey::StringLiteral(s) => Some(Ident {
                name: self.js_str(s.value.as_str(), sp(s.span))?,
                span: sp(s.span),
            }),
            other => self.unsupported(sp(other.span()), "このプロパティ名"),
        }
    }

    // ---- 関数とブロック -------------------------------------------------

    fn function(&mut self, f: &o::Function<'_>) -> Option<FunctionDecl> {
        let span = sp(f.span);
        let (name, params, ret, body) = self.function_parts(f, true)?;
        Some(FunctionDecl {
            name: name?,
            params,
            ret,
            body,
            span,
        })
    }

    /// 関数の名前・引数・戻り値の型・本体。`name_required` が偽なら、名前がなくてもよい。
    #[allow(clippy::type_complexity)]
    fn function_parts(
        &mut self,
        f: &o::Function<'_>,
        name_required: bool,
    ) -> Option<(Option<Ident>, Vec<Param>, Option<Type>, Block)> {
        let span = sp(f.span);
        if f.r#type != o::FunctionType::FunctionDeclaration || f.declare {
            return self.unsupported(span, "本体のない関数宣言（`declare`）");
        }
        if f.generator {
            return self.unsupported(span, "ジェネレータ関数");
        }
        if f.r#async {
            return self.reserved(span, "async");
        }
        if f.type_parameters.is_some() {
            return self.unsupported(span, "型引数をとる関数");
        }
        if f.this_param.is_some() {
            return self.reserved(span, "this");
        }
        let name = match &f.id {
            Some(id) if name_required => self.binding(id.name.as_str(), id.span),
            Some(id) => Some(Ident {
                name: id.name.as_str().to_owned(),
                span: sp(id.span),
            }),
            None if name_required => self.unsupported(span, "名前のない関数"),
            None => None,
        };
        let params = self.params(&f.params);
        let ret = match &f.return_type {
            Some(t) => Some(self.ty(&t.type_annotation)?),
            None => None,
        };
        let Some(body) = &f.body else {
            return self.unsupported(span, "本体のない関数");
        };
        if let Some(d) = body.directives.first() {
            return self.unsupported(sp(d.span), "関数の中のディレクティブ");
        }
        let body = self.block(&body.statements, body.span)?;
        if name_required && name.is_none() {
            return None;
        }
        Some((name, params?, ret, body))
    }

    fn params(&mut self, p: &o::FormalParameters<'_>) -> Option<Vec<Param>> {
        if let Some(rest) = &p.rest {
            return self.unsupported(sp(rest.span), "残余引数（`...`）");
        }
        let mut params = Vec::new();
        let mut ok = true;
        for item in &p.items {
            match self.param(item) {
                Some(param) => params.push(param),
                None => ok = false,
            }
        }
        ok.then_some(params)
    }

    fn param(&mut self, p: &o::FormalParameter<'_>) -> Option<Param> {
        let span = sp(p.span);
        if !p.decorators.is_empty() || p.accessibility.is_some() || p.readonly || p.r#override {
            return self.unsupported(span, "引数の修飾子");
        }
        if p.optional {
            return self.unsupported(span, "省略可能な引数");
        }
        if p.initializer.is_some() {
            return self.unsupported(span, "既定引数");
        }
        let pattern = match &p.pattern {
            o::BindingPattern::BindingIdentifier(id) => {
                ParamPattern::Ident(self.binding(id.name.as_str(), id.span)?)
            }
            o::BindingPattern::ObjectPattern(obj) => self.object_pattern(obj)?,
            _ => {
                return self.unsupported_help(
                    span,
                    "この形の引数パターン",
                    "引数は名前か、1 階層のオブジェクトパターン `{ a, b = 既定値 }: P`",
                );
            }
        };
        let ty = match &p.type_annotation {
            Some(t) => Some(self.ty(&t.type_annotation)?),
            None => None,
        };
        Some(Param { pattern, ty, span })
    }

    /// 引数のオブジェクトパターン `{ a, b = expr }`。別名・入れ子・rest・計算キーは書けない（DECISIONS R4）。
    fn object_pattern(&mut self, obj: &o::ObjectPattern<'_>) -> Option<ParamPattern> {
        let span = sp(obj.span);
        if let Some(rest) = &obj.rest {
            return self.unsupported_help(
                sp(rest.span),
                "分割代入の rest（`...`）",
                "取り出す名前を 1 つずつ書く",
            );
        }
        let mut props = Vec::new();
        let mut ok = true;
        for bp in &obj.properties {
            match self.pattern_prop(bp) {
                Some(p) => props.push(p),
                None => ok = false,
            }
        }
        ok.then_some(ParamPattern::Object(props, span))
    }

    fn pattern_prop(&mut self, bp: &o::BindingProperty<'_>) -> Option<PatternProp> {
        let span = sp(bp.span);
        if bp.computed || !bp.shorthand {
            return self.unsupported_help(
                span,
                "分割代入の別名・計算キー",
                "props の名前をそのまま使う（`{ a }`）。別名が要るなら `props.a` と参照する",
            );
        }
        // 省略形 `{ a }` か `{ a = expr }` だけが残る。
        let (id, default) = match &bp.value {
            o::BindingPattern::BindingIdentifier(id) => (id, None),
            o::BindingPattern::AssignmentPattern(ap) => match &ap.left {
                o::BindingPattern::BindingIdentifier(id) => (id, Some(&ap.right)),
                _ => {
                    return self.unsupported_help(
                        span,
                        "入れ子の分割代入",
                        "1 階層だけ書ける。深い値は `a.b` と参照する",
                    );
                }
            },
            _ => {
                return self.unsupported_help(
                    span,
                    "入れ子の分割代入",
                    "1 階層だけ書ける。深い値は `a.b` と参照する",
                );
            }
        };
        let name = self.binding(id.name.as_str(), id.span);
        let default = match default {
            Some(e) => Some(self.expr(e)?),
            None => None,
        };
        Some(PatternProp {
            name: name?,
            default,
            span,
        })
    }

    /// `{ const …; stmt }`（SPEC 第 5 章の `block`）。
    fn block(&mut self, stmts: &[o::Statement<'_>], span: oxc_span::Span) -> Option<Block> {
        let span = sp(span);
        let mut consts = Vec::new();
        let mut last = None;
        let mut ok = true;
        for stmt in stmts {
            if last.is_some() {
                ok = false;
                self.unsupported::<()>(sp(stmt.span()), "ブロックの最後の文のあとの文");
                continue;
            }
            match stmt {
                o::Statement::VariableDeclaration(d) => match self.const_decl(d) {
                    Some(c) => consts.push(c),
                    None => ok = false,
                },
                o::Statement::FunctionDeclaration(f) => {
                    ok = false;
                    self.unsupported::<()>(sp(f.span), "ブロックの中の関数宣言");
                }
                _ => match self.stmt(stmt) {
                    Some(s) => last = Some(s),
                    None => {
                        ok = false;
                        // 誤りは報告済み。後続の文を「最後の文のあと」と重ねて報告しない。
                        last = Some(Stmt::Return(
                            Expr {
                                kind: ExprKind::Undefined,
                                span,
                            },
                            span,
                        ));
                    }
                },
            }
        }
        let Some(stmt) = last else {
            return self.unsupported(span, "`return`・`if`・`switch` で終わらないブロック");
        };
        ok.then(|| Block {
            consts,
            stmt: Box::new(stmt),
            span,
        })
    }

    fn stmt(&mut self, stmt: &o::Statement<'_>) -> Option<Stmt> {
        let span = sp(stmt.span());
        if !self.enter(span) {
            return None;
        }
        let result = self.stmt_inner(stmt, span);
        self.leave();
        result
    }

    fn stmt_inner(&mut self, stmt: &o::Statement<'_>, span: Span) -> Option<Stmt> {
        match stmt {
            o::Statement::ReturnStatement(r) => {
                self.semicolon(r.span);
                let Some(arg) = &r.argument else {
                    return self.unsupported(span, "値のない `return`");
                };
                Some(Stmt::Return(self.expr(arg)?, span))
            }
            o::Statement::IfStatement(i) => {
                let cond = self.expr(&i.test);
                let then = self.if_body(&i.consequent);
                let otherwise = match &i.alternate {
                    None => {
                        self.diag_help(
                            Code::MissingElse,
                            span,
                            "`if` には `else` が要る",
                            "`else { return …; }` を足す",
                        );
                        None
                    }
                    Some(o::Statement::BlockStatement(b)) => {
                        self.block(&b.body, b.span).map(Else::Block)
                    }
                    Some(alt @ o::Statement::IfStatement(_)) => {
                        self.stmt(alt).map(|s| Else::If(Box::new(s)))
                    }
                    Some(other) => {
                        self.unsupported(sp(other.span()), "`{ … }` で囲まない `else` の本体")
                    }
                };
                Some(Stmt::If {
                    cond: cond?,
                    then: then?,
                    otherwise: otherwise?,
                    span,
                })
            }
            o::Statement::SwitchStatement(s) => self.switch(s, span),
            other => self.unsupported_stmt(other),
        }
    }

    fn if_body(&mut self, stmt: &o::Statement<'_>) -> Option<Block> {
        match stmt {
            o::Statement::BlockStatement(b) => self.block(&b.body, b.span),
            other => self.unsupported_help(
                sp(other.span()),
                "`{ … }` で囲まない `if` の本体",
                "`{ … }` で囲む",
            ),
        }
    }

    fn switch(&mut self, s: &o::SwitchStatement<'_>, span: Span) -> Option<Stmt> {
        let scrutinee = self.expr(&s.discriminant);
        let mut cases = Vec::new();
        let mut default = None;
        let mut ok = true;
        for case in &s.cases {
            let case_span = sp(case.span);
            if default.is_some() {
                ok = false;
                self.unsupported::<()>(case_span, "`default` のあとの `case`");
                continue;
            }
            let body = match case.consequent.as_slice() {
                [only] => self.stmt(only),
                [] => self.unsupported_help(
                    case_span,
                    "本体のない `case`（複数のラベルの共有）",
                    "`case` ごとに `return` を書く",
                ),
                _ => self.unsupported(case_span, "文が 2 つ以上ある `case`"),
            };
            match &case.test {
                Some(test) => match (self.case_lit(test), body) {
                    (Some(test), Some(body)) => cases.push(Case {
                        test,
                        body,
                        span: case_span,
                    }),
                    _ => ok = false,
                },
                None => match body {
                    Some(body) => default = Some(Box::new(body)),
                    None => ok = false,
                },
            }
        }
        let scrutinee = scrutinee?;
        ok.then_some(Stmt::Switch {
            scrutinee,
            cases,
            default,
            span,
        })
    }

    fn case_lit(&mut self, e: &o::Expression<'_>) -> Option<Lit> {
        let span = sp(e.span());
        match e {
            o::Expression::StringLiteral(s) => {
                Some(Lit::String(self.js_str(s.value.as_str(), span)?, span))
            }
            o::Expression::NumericLiteral(n) => {
                Some(Lit::Number(self.number(n.value, span)?, span))
            }
            o::Expression::BooleanLiteral(b) => Some(Lit::Bool(b.value, span)),
            o::Expression::UnaryExpression(u) if u.operator == o::UnaryOperator::UnaryNegation => {
                match &u.argument {
                    o::Expression::NumericLiteral(n) => {
                        Some(Lit::Number(-self.number(n.value, span)?, span))
                    }
                    _ => self.unsupported(span, "リテラルでない `case`"),
                }
            }
            _ => self.unsupported(span, "リテラルでない `case`"),
        }
    }

    // ---- 型 --------------------------------------------------------------

    fn ty(&mut self, t: &o::TSType<'_>) -> Option<Type> {
        let span = sp(t.span());
        if !self.enter(span) {
            return None;
        }
        let result = self.ty_inner(t, span);
        self.leave();
        result
    }

    fn ty_inner(&mut self, t: &o::TSType<'_>, span: Span) -> Option<Type> {
        Some(match t {
            o::TSType::TSBooleanKeyword(_) => Type::Boolean(span),
            o::TSType::TSNumberKeyword(_) => Type::Number(span),
            o::TSType::TSStringKeyword(_) => Type::String(span),
            o::TSType::TSUndefinedKeyword(_) => Type::Undefined(span),
            o::TSType::TSNullKeyword(_) => return self.reserved(span, "null"),
            o::TSType::TSAnyKeyword(_) => return self.unsupported(span, "`any` 型"),
            o::TSType::TSUnknownKeyword(_) => return self.unsupported(span, "`unknown` 型"),
            o::TSType::TSArrayType(a) => Type::Array(Box::new(self.ty(&a.element_type)?), span),
            o::TSType::TSTupleType(tuple) => {
                let mut elems = Vec::new();
                let mut ok = true;
                for elem in &tuple.element_types {
                    match elem.as_ts_type() {
                        Some(t) => match self.ty(t) {
                            Some(t) => elems.push(t),
                            None => ok = false,
                        },
                        None => {
                            ok = false;
                            self.unsupported::<()>(sp(elem.span()), "省略可能・残余のタプル要素");
                        }
                    }
                }
                if !ok {
                    return None;
                }
                Type::Tuple(elems, span)
            }
            o::TSType::TSTypeLiteral(l) => Type::Object(self.members(&l.members)?, span),
            o::TSType::TSUnionType(u) => {
                let mut types = Vec::new();
                let mut ok = true;
                for t in &u.types {
                    match self.ty(t) {
                        Some(t) => types.push(t),
                        None => ok = false,
                    }
                }
                if !ok {
                    return None;
                }
                Type::Union(types, span)
            }
            o::TSType::TSLiteralType(l) => Type::Literal(match &l.literal {
                o::TSLiteral::BooleanLiteral(b) => Lit::Bool(b.value, span),
                o::TSLiteral::NumericLiteral(n) => Lit::Number(self.number(n.value, span)?, span),
                o::TSLiteral::StringLiteral(s) => {
                    Lit::String(self.js_str(s.value.as_str(), span)?, span)
                }
                o::TSLiteral::UnaryExpression(u) => match (&u.operator, &u.argument) {
                    (o::UnaryOperator::UnaryNegation, o::Expression::NumericLiteral(n)) => {
                        Lit::Number(-self.number(n.value, span)?, span)
                    }
                    _ => return self.unsupported(span, "このリテラル型"),
                },
                _ => return self.unsupported(span, "このリテラル型"),
            }),
            o::TSType::TSTypeReference(r) => {
                let o::TSTypeName::IdentifierReference(id) = &r.type_name else {
                    return self.unsupported(span, "修飾された型名");
                };
                let mut args = Vec::new();
                if let Some(a) = &r.type_arguments {
                    let mut ok = true;
                    for t in &a.params {
                        match self.ty(t) {
                            Some(t) => args.push(t),
                            None => ok = false,
                        }
                    }
                    if !ok {
                        return None;
                    }
                }
                Type::Named {
                    name: Ident {
                        name: id.name.as_str().to_owned(),
                        span: sp(id.span),
                    },
                    args,
                    span,
                }
            }
            o::TSType::TSFunctionType(f) => {
                if f.type_parameters.is_some() || f.this_param.is_some() {
                    return self.unsupported(span, "型引数・`this` をとる関数型");
                }
                let params = self.params(&f.params)?;
                let mut typed = Vec::new();
                for p in params {
                    let ParamPattern::Ident(name) = p.pattern else {
                        return self.unsupported(p.span, "関数型の分割代入の引数");
                    };
                    match p.ty {
                        Some(t) => typed.push((name, t)),
                        None => return self.unsupported(p.span, "関数型の、型注釈のない引数"),
                    }
                }
                Type::Function {
                    params: typed,
                    ret: Box::new(self.ty(&f.return_type.type_annotation)?),
                    span,
                }
            }
            o::TSType::TSParenthesizedType(p) => return self.ty(&p.type_annotation),
            o::TSType::TSIntersectionType(_) => return self.unsupported(span, "交差型"),
            o::TSType::TSTypeOperatorType(_) => {
                return self.unsupported(span, "型演算子（`keyof` など）");
            }
            o::TSType::TSTypeQuery(_) => return self.unsupported(span, "`typeof` 型"),
            _ => return self.unsupported(span, "この型"),
        })
    }

    // ---- 式 --------------------------------------------------------------

    fn expr(&mut self, e: &o::Expression<'_>) -> Option<Expr> {
        let span = sp(e.span());
        if !self.enter(span) {
            return None;
        }
        let result = self.expr_inner(e, span);
        self.leave();
        result
    }

    fn boxed(&mut self, e: &o::Expression<'_>) -> Option<Box<Expr>> {
        self.expr(e).map(Box::new)
    }

    fn exprs<'a, I>(&mut self, items: I) -> Option<Vec<Expr>>
    where
        I: IntoIterator<Item = &'a o::Expression<'a>>,
    {
        let mut out = Vec::new();
        let mut ok = true;
        for e in items {
            match self.expr(e) {
                Some(e) => out.push(e),
                None => ok = false,
            }
        }
        ok.then_some(out)
    }

    fn expr_inner(&mut self, e: &o::Expression<'_>, span: Span) -> Option<Expr> {
        let kind = match e {
            o::Expression::BooleanLiteral(b) => ExprKind::Bool(b.value),
            o::Expression::NullLiteral(_) => return self.reserved(span, "null"),
            o::Expression::NumericLiteral(n) => ExprKind::Number(self.number(n.value, span)?),
            o::Expression::BigIntLiteral(_) => {
                return self.err(Code::UnsupportedLiteral, span, "BigInt リテラルは使えない");
            }
            o::Expression::RegExpLiteral(_) => {
                return self.err(Code::UnsupportedLiteral, span, "正規表現リテラルは使えない");
            }
            o::Expression::StringLiteral(s) => {
                ExprKind::String(self.js_str(s.value.as_str(), span)?)
            }
            o::Expression::TemplateLiteral(t) => self.template(t, span)?,
            o::Expression::Identifier(id) => self.reference(id.name.as_str(), span)?,
            o::Expression::ArrayExpression(a) => self.array(a)?,
            o::Expression::ObjectExpression(obj) => self.object(obj)?,
            o::Expression::ArrowFunctionExpression(a) => self.arrow(a, span)?,
            o::Expression::UnaryExpression(u) => self.unary(u, span)?,
            o::Expression::BinaryExpression(b) => self.binary(b, span)?,
            o::Expression::LogicalExpression(l) => ExprKind::Logical {
                op: match l.operator {
                    o::LogicalOperator::And => LogicalOp::And,
                    o::LogicalOperator::Or => LogicalOp::Or,
                    o::LogicalOperator::Coalesce => LogicalOp::Coalesce,
                },
                lhs: self.boxed(&l.left)?,
                rhs: self.boxed(&l.right)?,
            },
            o::Expression::ConditionalExpression(c) => {
                let test = self.boxed(&c.test);
                let cons = self.boxed(&c.consequent);
                let alt = self.boxed(&c.alternate);
                ExprKind::Conditional {
                    test: test?,
                    cons: cons?,
                    alt: alt?,
                }
            }
            o::Expression::StaticMemberExpression(m) => self.static_member(m)?,
            o::Expression::ComputedMemberExpression(m) => self.computed_member(m, span)?,
            o::Expression::CallExpression(c) => self.call(c, span)?,
            o::Expression::ChainExpression(c) => match &c.expression {
                o::ChainElement::CallExpression(call) => self.call(call, span)?,
                o::ChainElement::StaticMemberExpression(m) => self.static_member(m)?,
                o::ChainElement::ComputedMemberExpression(m) => self.computed_member(m, span)?,
                _ => return self.unsupported(span, "この `?.` の式"),
            },
            o::Expression::ParenthesizedExpression(p) => return self.expr(&p.expression),
            o::Expression::TSSatisfiesExpression(s) => {
                let expr = self.boxed(&s.expression);
                let ty = self.ty(&s.type_annotation);
                ExprKind::Satisfies {
                    expr: expr?,
                    ty: ty?,
                }
            }
            o::Expression::JSXElement(el) => ExprKind::Element(self.element(el)?),
            o::Expression::JSXFragment(f) => ExprKind::Fragment(self.children(&f.children)?),
            o::Expression::SequenceExpression(_) => return self.unsupported(span, "カンマ演算子"),
            o::Expression::AssignmentExpression(_) => return self.unsupported(span, "代入"),
            o::Expression::UpdateExpression(_) => return self.unsupported(span, "`++` と `--`"),
            o::Expression::AwaitExpression(_) => return self.reserved(span, "await"),
            o::Expression::YieldExpression(_) => return self.reserved(span, "yield"),
            o::Expression::ThisExpression(_) => return self.reserved(span, "this"),
            o::Expression::NewExpression(_) => return self.reserved(span, "new"),
            o::Expression::ClassExpression(_) => return self.reserved(span, "class"),
            o::Expression::FunctionExpression(_) => {
                return self.unsupported_help(
                    span,
                    "`function` 式",
                    "アロー関数 `(…) => …` を使う",
                );
            }
            o::Expression::TaggedTemplateExpression(_) => {
                return self.unsupported(span, "タグ付きテンプレート");
            }
            o::Expression::TSAsExpression(_) => return self.reserved(span, "as"),
            o::Expression::TSTypeAssertion(_) => return self.unsupported(span, "型アサーション"),
            o::Expression::TSNonNullExpression(_) => {
                return self.unsupported_help(
                    span,
                    "非 null アサーション `!`",
                    "`??` か `!== undefined` で絞り込む",
                );
            }
            _ => return self.unsupported(span, "この式"),
        };
        Some(Expr { kind, span })
    }

    fn reference(&mut self, name: &str, span: Span) -> Option<ExprKind> {
        if name == "undefined" {
            return Some(ExprKind::Undefined);
        }
        if RESERVED.contains(&name) {
            return self.reserved(span, name);
        }
        if is_hook_name(name) {
            return self.err_help(
                Code::ReactFeature,
                span,
                format!("`{name}`: `use` で始まる名前（React の Hooks）は使えない"),
                HOOK_HELP,
            );
        }
        Some(ExprKind::Ident(name.to_owned()))
    }

    fn template(&mut self, t: &o::TemplateLiteral<'_>, span: Span) -> Option<ExprKind> {
        let mut quasis = Vec::new();
        for q in &t.quasis {
            let Some(cooked) = q.value.cooked else {
                return self.err(Code::InvalidEscape, sp(q.span), "使えないエスケープ");
            };
            quasis.push(self.js_str(cooked.as_str(), span)?);
        }
        Some(ExprKind::Template {
            quasis,
            exprs: self.exprs(&t.expressions)?,
        })
    }

    fn array(&mut self, a: &o::ArrayExpression<'_>) -> Option<ExprKind> {
        let mut elems = Vec::new();
        let mut ok = true;
        for elem in &a.elements {
            let converted = match elem {
                o::ArrayExpressionElement::SpreadElement(s) => self
                    .expr(&s.argument)
                    .map(|e| ArrayElem::Spread(e, sp(s.span))),
                o::ArrayExpressionElement::Elision(e) => self.unsupported(sp(e.span), "配列の穴"),
                other => match other.as_expression() {
                    Some(e) => self.expr(e).map(ArrayElem::Expr),
                    None => self.unsupported(sp(other.span()), "この配列の要素"),
                },
            };
            match converted {
                Some(e) => elems.push(e),
                None => ok = false,
            }
        }
        ok.then_some(ExprKind::Array(elems))
    }

    fn object(&mut self, obj: &o::ObjectExpression<'_>) -> Option<ExprKind> {
        let mut props = Vec::new();
        let mut ok = true;
        for prop in &obj.properties {
            let converted = match prop {
                o::ObjectPropertyKind::SpreadProperty(s) => self
                    .expr(&s.argument)
                    .map(|e| ObjProp::Spread(e, sp(s.span))),
                o::ObjectPropertyKind::ObjectProperty(p) => self.object_property(p),
            };
            match converted {
                Some(p) => props.push(p),
                None => ok = false,
            }
        }
        ok.then_some(ExprKind::Object(props))
    }

    fn object_property(&mut self, p: &o::ObjectProperty<'_>) -> Option<ObjProp> {
        let span = sp(p.span);
        if p.kind != o::PropertyKind::Init {
            return self.unsupported(span, "getter と setter");
        }
        if p.method {
            return self.unsupported_help(span, "メソッド構文", "`k: (…) => …` と書く");
        }
        if p.computed {
            return self.unsupported(span, "計算プロパティ名");
        }
        let key = self.prop_key(&p.key)?;
        if p.shorthand {
            self.reference(&key.name, key.span)?;
            return Some(ObjProp::Shorthand(key));
        }
        Some(ObjProp::KeyValue {
            key,
            value: self.expr(&p.value)?,
            span,
        })
    }

    fn arrow(&mut self, a: &o::ArrowFunctionExpression<'_>, span: Span) -> Option<ExprKind> {
        if a.r#async {
            return self.reserved(span, "async");
        }
        if a.type_parameters.is_some() {
            return self.unsupported(span, "型引数をとる関数");
        }
        let params = self.params(&a.params);
        let ret = match &a.return_type {
            Some(t) => Some(self.ty(&t.type_annotation)?),
            None => None,
        };
        let body = match &a.body {
            o::ArrowFunctionBody::FunctionBody(b) => {
                if let Some(d) = b.directives.first() {
                    return self.unsupported(sp(d.span), "関数の中のディレクティブ");
                }
                ArrowBody::Block(self.block(&b.statements, b.span)?)
            }
            other => match other.as_expression() {
                Some(e) => ArrowBody::Expr(self.boxed(e)?),
                None => return self.unsupported(span, "このアロー関数の本体"),
            },
        };
        Some(ExprKind::Arrow {
            params: params?,
            ret,
            body,
        })
    }

    fn unary(&mut self, u: &o::UnaryExpression<'_>, span: Span) -> Option<ExprKind> {
        let op = match u.operator {
            o::UnaryOperator::UnaryNegation => UnaryOp::Neg,
            o::UnaryOperator::LogicalNot => UnaryOp::Not,
            o::UnaryOperator::UnaryPlus => return self.unsupported(span, "単項の `+`"),
            o::UnaryOperator::BitwiseNot => return self.unsupported(span, "ビット演算"),
            o::UnaryOperator::Typeof => return self.unsupported(span, "`typeof`"),
            o::UnaryOperator::Void => return self.unsupported(span, "`void`"),
            o::UnaryOperator::Delete => return self.unsupported(span, "`delete`"),
        };
        Some(ExprKind::Unary {
            op,
            expr: self.boxed(&u.argument)?,
        })
    }

    fn binary(&mut self, b: &o::BinaryExpression<'_>, span: Span) -> Option<ExprKind> {
        use o::BinaryOperator as B;
        let op = match b.operator {
            B::Addition => BinaryOp::Add,
            B::Subtraction => BinaryOp::Sub,
            B::Multiplication => BinaryOp::Mul,
            B::Division => BinaryOp::Div,
            B::Remainder => BinaryOp::Rem,
            B::StrictEquality => BinaryOp::Eq,
            B::StrictInequality => BinaryOp::Ne,
            B::LessThan => BinaryOp::Lt,
            B::LessEqualThan => BinaryOp::Le,
            B::GreaterThan => BinaryOp::Gt,
            B::GreaterEqualThan => BinaryOp::Ge,
            B::Equality | B::Inequality => {
                self.diag_help(
                    Code::UnsupportedSyntax,
                    span,
                    "`==` と `!=` は CDL では書けない",
                    "`===` か `!==` を使う",
                );
                return None;
            }
            B::Exponential => return self.unsupported(span, "`**`"),
            B::In | B::Instanceof => return self.unsupported(span, "`in` と `instanceof`"),
            _ => return self.unsupported(span, "ビット演算・シフト"),
        };
        let lhs = self.boxed(&b.left);
        let rhs = self.boxed(&b.right);
        Some(ExprKind::Binary {
            op,
            lhs: lhs?,
            rhs: rhs?,
        })
    }

    fn static_member(&mut self, m: &o::StaticMemberExpression<'_>) -> Option<ExprKind> {
        Some(ExprKind::Member {
            object: self.boxed(&m.object)?,
            prop: Ident {
                name: m.property.name.as_str().to_owned(),
                span: sp(m.property.span),
            },
            optional: m.optional,
        })
    }

    fn computed_member(
        &mut self,
        m: &o::ComputedMemberExpression<'_>,
        span: Span,
    ) -> Option<ExprKind> {
        if m.optional {
            return self.unsupported(span, "`?.[ ]`");
        }
        let object = self.boxed(&m.object);
        let index = self.boxed(&m.expression);
        Some(ExprKind::Index {
            object: object?,
            index: index?,
        })
    }

    fn call(&mut self, c: &o::CallExpression<'_>, span: Span) -> Option<ExprKind> {
        if c.optional {
            return self.unsupported(span, "`?.()`");
        }
        if c.type_arguments.is_some() {
            return self.unsupported(span, "型引数つきの呼び出し");
        }
        let callee = self.boxed(&c.callee);
        let mut args = Vec::new();
        let mut ok = true;
        for arg in &c.arguments {
            let converted = match arg.as_expression() {
                Some(e) => self.expr(e),
                None => self.unsupported(sp(arg.span()), "引数のスプレッド"),
            };
            match converted {
                Some(e) => args.push(e),
                None => ok = false,
            }
        }
        let callee = callee?;
        ok.then_some(ExprKind::Call { callee, args })
    }

    // ---- JSX -------------------------------------------------------------

    fn element(&mut self, el: &o::JSXElement<'_>) -> Option<Element> {
        let span = sp(el.span);
        if !self.enter(span) {
            return None;
        }
        let result = self.element_inner(el);
        self.leave();
        result
    }

    fn element_inner(&mut self, el: &o::JSXElement<'_>) -> Option<Element> {
        let opening = &el.opening_element;
        let name = match &opening.name {
            o::JSXElementName::IdentifierReference(id) => Ident {
                name: id.name.as_str().to_owned(),
                span: sp(id.span),
            },
            o::JSXElementName::Identifier(id) => Ident {
                name: id.name.as_str().to_owned(),
                span: sp(id.span),
            },
            other => return self.unsupported(sp(other.span()), "名前空間・メンバーの JSX タグ"),
        };
        let name = if name
            .name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_uppercase())
        {
            Some(name)
        } else {
            self.err(
                Code::LowercaseJsxTag,
                name.span,
                format!(
                    "`<{}>`: JSX のタグはコンポーネント名（大文字で始まる名前）だけ",
                    name.name
                ),
            )
        };
        if let Some(t) = &opening.type_arguments {
            return self.unsupported(sp(t.span), "型引数つきの JSX タグ");
        }
        let mut attrs = Vec::new();
        let mut ok = true;
        for attr in &opening.attributes {
            match self.attr(attr) {
                Some(a) => attrs.push(a),
                None => ok = false,
            }
        }
        let children = self.children(&el.children);
        let name = name?;
        let children = children?;
        ok.then_some(Element {
            name,
            attrs,
            children,
            span: sp(el.span),
        })
    }

    fn attr(&mut self, attr: &o::JSXAttributeItem<'_>) -> Option<Attr> {
        let a = match attr {
            o::JSXAttributeItem::SpreadAttribute(s) => {
                return Some(Attr::Spread(self.expr(&s.argument)?, sp(s.span)));
            }
            o::JSXAttributeItem::Attribute(a) => a,
        };
        let o::JSXAttributeName::Identifier(id) = &a.name else {
            return self.unsupported(sp(a.span), "名前空間つきの属性");
        };
        let name = id.name.as_str();
        let span = sp(id.span);
        if name == "ref" {
            return self.err(
                Code::ReactFeature,
                span,
                format!("`{name}` 属性は CDL にない"),
            );
        }
        if is_event_attr(name) {
            return self.err_help(
                Code::ReactFeature,
                span,
                format!("`{name}`: イベントハンドラ属性は CDL にない"),
                "イベントへの応答は、ホストが呼ぶポリシーのフック（`export const policy`）に書く",
            );
        }
        let value = match &a.value {
            None => None,
            Some(o::JSXAttributeValue::StringLiteral(s)) => Some(AttrValue::String(StrLit {
                value: self.js_str(s.value.as_str(), sp(s.span))?,
                span: sp(s.span),
            })),
            Some(o::JSXAttributeValue::ExpressionContainer(c)) => {
                match c.expression.as_expression() {
                    Some(e) => Some(AttrValue::Expr(self.expr(e)?)),
                    None => return self.unsupported(sp(c.span), "空の属性値 `{}`"),
                }
            }
            Some(other) => return self.unsupported(sp(other.span()), "要素を属性値に置くこと"),
        };
        Some(Attr::Named {
            span: sp(a.span),
            name: Ident {
                name: name.to_owned(),
                span,
            },
            value,
        })
    }

    fn children(&mut self, children: &[o::JSXChild<'_>]) -> Option<Vec<Child>> {
        let mut out = Vec::new();
        let mut ok = true;
        for child in children {
            let converted = match child {
                o::JSXChild::Text(t) => {
                    let span = sp(t.span);
                    let raw = self.src.get(to_usize(span.start)..to_usize(span.end));
                    let Some(raw) = raw else {
                        ok = false;
                        continue;
                    };
                    // 文字参照（`&amp;` など）は扱わない（DECISIONS R1）。
                    if let Some(at) = raw.find('&') {
                        let start = span.start.saturating_add(u32::try_from(at).unwrap_or(0));
                        self.diag_help(
                            Code::JsxTextCharRef,
                            Span {
                                start,
                                end: start.saturating_add(1),
                            },
                            "JSX のテキスト子に `&` は書けない（文字参照は使えない）",
                            "`&` を含む文字列は `{\"a & b\"}` と書く",
                        );
                        ok = false;
                        continue;
                    }
                    match normalize_jsx_text(raw) {
                        Some(text) => Some(Child::Text(text, span)),
                        // 空白だけのテキストは捨てる。
                        None => continue,
                    }
                }
                o::JSXChild::Element(e) => self.element(e).map(|el| Child::Element(el, sp(e.span))),
                o::JSXChild::Fragment(f) => self
                    .children(&f.children)
                    .map(|c| Child::Fragment(c, sp(f.span))),
                o::JSXChild::ExpressionContainer(c) => match c.expression.as_expression() {
                    // 中身が空白とコメントだけの `{}` は無視する（DECISIONS C14）。
                    None => continue,
                    Some(e) => self.expr(e).map(|e| Child::Expr(e, sp(c.span))),
                },
                o::JSXChild::Spread(s) => self.unsupported(sp(s.span), "子のスプレッド `{...xs}`"),
            };
            match converted {
                Some(c) => out.push(c),
                None => ok = false,
            }
        }
        ok.then_some(out)
    }
}
