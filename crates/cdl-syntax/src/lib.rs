//! ソースを CDL の AST に変換する。
//!
//! プロトタイプでは構文解析に `oxc_parser` を使う。`oxc_*` の型はこのクレートの外へ出さない。
#![forbid(unsafe_code)]

pub mod ast;
mod convert;
mod lex;

use cdl_diag::{Code, Diagnostic};
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

/// 構文解析の上限（SPEC 第 7 章、DECISIONS C10）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// 1 ファイルのソースサイズ（バイト）。
    pub max_source_bytes: u32,
    /// 構文の入れ子の深さ。
    pub max_nesting: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_source_bytes: 1 << 20,
            max_nesting: 64,
        }
    }
}

fn span_u32(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

fn diagnostic(
    file: &str,
    code: Code,
    start: u32,
    end: u32,
    message: impl Into<String>,
) -> Diagnostic {
    let span = cdl_diag::Span {
        file: file.to_owned(),
        start,
        end,
    };
    Diagnostic::error(code, span, message)
}

/// 1 つのファイルを構文解析する。誤りがあれば、位置順に並べた診断を返す。
///
/// 字句検査で誤りが見つかったら、oxc での構文解析は行わない。
pub fn parse(file: &str, source: &str, limits: &Limits) -> Result<ast::Module, Vec<Diagnostic>> {
    let size = source.len();
    if size > usize::try_from(limits.max_source_bytes).unwrap_or(usize::MAX) {
        return Err(vec![diagnostic(
            file,
            Code::SourceTooLarge,
            0,
            0,
            format!(
                "ソースが {size} バイトあり、上限の {} バイトを超えている",
                limits.max_source_bytes
            ),
        )]);
    }

    let lex_errors = lex::check(source, limits.max_nesting);
    if !lex_errors.is_empty() {
        return Err(lex_errors
            .into_iter()
            .map(|e| diagnostic(file, e.code, span_u32(e.start), span_u32(e.end), e.message))
            .collect());
    }

    let allocator = Allocator::default();
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, source, SourceType::tsx())
        .with_options(options)
        .parse();
    let syntax_errors: Vec<Diagnostic> = parsed
        .diagnostics
        .errors()
        .map(|e| {
            let (start, end) = e
                .labels
                .first()
                .map_or((0, 0), |l| (l.offset(), l.offset().saturating_add(l.len())));
            diagnostic(
                file,
                Code::SyntaxError,
                start,
                end,
                format!("構文エラー: {}", e.message),
            )
        })
        .collect();
    if !syntax_errors.is_empty() || parsed.fatal_error {
        let mut errors = syntax_errors;
        if errors.is_empty() {
            errors.push(diagnostic(file, Code::SyntaxError, 0, 0, "構文エラー"));
        }
        cdl_diag::sort(&mut errors);
        return Err(errors);
    }

    // 再帰の多重防御の上限は、字句検査の上限より緩くする（同じ入れ子を数え方の違いで重ねて数えるため）。
    let max_depth = limits.max_nesting.saturating_mul(4).saturating_add(16);
    let mut converter = convert::Converter::new(file, source, max_depth);
    let module = converter.module(&parsed.program);
    let mut diags = converter.into_diagnostics();
    match module {
        Some(module) if diags.is_empty() => Ok(module),
        _ => {
            if diags.is_empty() {
                diags.push(diagnostic(file, Code::SyntaxError, 0, 0, "構文エラー"));
            }
            cdl_diag::sort(&mut diags);
            Err(diags)
        }
    }
}
