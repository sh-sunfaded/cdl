//! 自前の字句検査（DECISIONS C9・C10・C12・C13）。
//!
//! oxc に渡す前に、次を検査する。
//! - 字句の誤り（E0xxx）: 文字列・テンプレート・コメント・数値・使えない文字
//! - 構文の入れ子の深さ（E1008）: oxc と自前の再帰処理がスタックを使い切らないようにする
//!
//! 構文木は作らない。JSX とテンプレート文字列は、文字列やコメントと取り違えないための
//! 最小限のモードとして追う。

use cdl_diag::Code;

/// 字句検査で見つけた誤り。位置はバイトオフセット。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexError {
    pub code: Code,
    pub start: usize,
    pub end: usize,
    pub message: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKind {
    /// ファイルの最上位。取り除かない。
    Root,
    Paren,
    Bracket,
    Brace,
    /// テンプレート文字列の `${ … }`
    TemplateExpr,
    /// JSX の開始タグの中（`<Name` から `>` / `/>` まで）
    JsxTag,
    /// JSX の子（`>` から閉じタグまで）
    JsxChildren,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    kind: FrameKind,
    /// 同じ括弧の中で `,` か `;` まで続いた、演算子などの連鎖の数（DECISIONS C10 改訂）。
    chain: u32,
    /// テンプレート文字列の開始位置（閉じていないときの診断に使う）。
    start: usize,
}

/// 入れ子の深さの帳簿。深さ = 開いている括弧の数 + 各括弧の連鎖の数。
struct Nesting {
    frames: Vec<Frame>,
    depth: u32,
    max: u32,
}

/// 上限を超えた。
#[derive(Debug, PartialEq, Eq)]
struct TooDeep;

impl Nesting {
    fn new(max: u32) -> Self {
        Nesting {
            frames: vec![Frame {
                kind: FrameKind::Root,
                chain: 0,
                start: 0,
            }],
            depth: 0,
            max,
        }
    }

    fn check(&self) -> Result<(), TooDeep> {
        if self.depth > self.max {
            Err(TooDeep)
        } else {
            Ok(())
        }
    }

    fn top(&self) -> FrameKind {
        self.frames.last().map_or(FrameKind::Root, |f| f.kind)
    }

    fn push(&mut self, kind: FrameKind, start: usize) -> Result<(), TooDeep> {
        self.frames.push(Frame {
            kind,
            chain: 0,
            start,
        });
        self.depth = self.depth.saturating_add(1);
        self.check()
    }

    /// 一番上の括弧が `kind` なら取り除く。違えば何もしない（括弧の対応は oxc が報告する）。
    fn pop(&mut self, kind: FrameKind) -> Option<Frame> {
        if kind == FrameKind::Root || self.top() != kind {
            return None;
        }
        let frame = self.frames.pop()?;
        self.depth = self.depth.saturating_sub(frame.chain.saturating_add(1));
        Some(frame)
    }

    fn replace_top(&mut self, kind: FrameKind) {
        if let Some(frame) = self.frames.last_mut()
            && frame.kind != FrameKind::Root
        {
            frame.kind = kind;
        }
    }

    fn inc(&mut self) -> Result<(), TooDeep> {
        if let Some(frame) = self.frames.last_mut() {
            frame.chain = frame.chain.saturating_add(1);
            self.depth = self.depth.saturating_add(1);
        }
        self.check()
    }

    fn reset(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            self.depth = self.depth.saturating_sub(frame.chain);
            frame.chain = 0;
        }
    }
}

/// このキーワードのあとは式が始まる（`<` は JSX、`/` は正規表現になる）。
const EXPR_KEYWORDS: &[&[u8]] = &[
    b"return",
    b"default",
    b"case",
    b"typeof",
    b"void",
    b"delete",
    b"await",
    b"yield",
    b"new",
    b"else",
    b"in",
    b"of",
    b"instanceof",
];

/// oxc が前置の構文として再帰で読むキーワード。連鎖として数える。
const NESTING_KEYWORDS: &[&[u8]] = &[
    b"if",
    b"typeof",
    b"void",
    b"delete",
    b"await",
    b"new",
    b"yield",
    b"keyof",
    b"readonly",
    b"unique",
    b"infer",
    b"asserts",
];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'$' || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

/// `at` から始まる文字のバイト長（範囲外なら 0）。
fn char_len(src: &str, at: usize) -> usize {
    src.get(at..)
        .and_then(|rest| rest.chars().next())
        .map_or(0, char::len_utf8)
}

/// 文字列の中の `\` から始まるエスケープを検査する。
/// 戻り値は（読んだバイト数, 正しいか）。`template` ならテンプレート文字列の規則を使う。
pub(crate) fn scan_escape(bytes: &[u8], at: usize, template: bool) -> (usize, bool) {
    let get = |off: usize| at.checked_add(off).and_then(|i| bytes.get(i).copied());
    match get(1) {
        Some(b'n' | b't' | b'\\' | b'"' | b'\'') => (2, true),
        Some(b'`' | b'$') if template => (2, true),
        Some(b'u') if get(2) == Some(b'{') => {
            let mut value: u32 = 0;
            let mut digits: usize = 0;
            let mut off: usize = 3;
            while let Some(b) = get(off) {
                let Some(d) = char::from(b).to_digit(16) else {
                    break;
                };
                value = value.saturating_mul(16).saturating_add(d);
                digits = digits.saturating_add(1);
                off = off.saturating_add(1);
            }
            if get(off) != Some(b'}') {
                return (off, false);
            }
            let scalar = digits > 0 && value <= 0x10_FFFF && !(0xD800..=0xDFFF).contains(&value);
            (off.saturating_add(1), scalar)
        }
        // 続く文字ごと読み飛ばす。非 ASCII のバイトは、続きのバイトを次の走査で読む。
        Some(_) => (2, false),
        None => (1, false),
    }
}

/// 数字の列（`_` 区切りを含む）を読む。戻り値は（終わりの位置, 数字があり `_` の位置が正しいか）。
fn scan_digits(bytes: &[u8], mut i: usize, is_digit: fn(u8) -> bool) -> (usize, bool) {
    let mut ok = true;
    let mut any = false;
    let mut prev_sep = false;
    while let Some(&b) = bytes.get(i) {
        if b == b'_' {
            if !any || prev_sep {
                ok = false;
            }
            prev_sep = true;
        } else if is_digit(b) {
            any = true;
            prev_sep = false;
        } else {
            break;
        }
        i = i.saturating_add(1);
    }
    (i, ok && any && !prev_sep)
}

/// 数値リテラルの形を検査する（値は oxc が計算する）。`start` は数字か `.` の位置。
/// 戻り値は（終わりの位置, 誤りのメッセージ）。
pub(crate) fn scan_number(bytes: &[u8], start: usize) -> (usize, Option<&'static str>) {
    let at = |i: usize| bytes.get(i).copied();
    let next = start.saturating_add(1);
    let mut error = None;
    let mut i;
    if at(start) == Some(b'0') && matches!(at(next), Some(b'x' | b'X' | b'b' | b'B' | b'o' | b'O'))
    {
        let radix_digit: fn(u8) -> bool = match at(next) {
            Some(b'x' | b'X') => |b| b.is_ascii_hexdigit(),
            Some(b'b' | b'B') => |b| matches!(b, b'0' | b'1'),
            _ => |b| matches!(b, b'0'..=b'7'),
        };
        let (end, ok) = scan_digits(bytes, start.saturating_add(2), radix_digit);
        i = end;
        if matches!(at(next), Some(b'o' | b'O')) {
            error = Some("8 進数リテラル（`0o`）は使えない");
        } else if !ok {
            error = Some("数値リテラルの形が不正");
        }
    } else if at(start) == Some(b'0') && matches!(at(next), Some(b'0'..=b'9' | b'_')) {
        let (end, _) = scan_digits(bytes, next, |b| b.is_ascii_digit());
        i = end;
        error = Some("先頭が 0 の数値リテラルは使えない");
    } else {
        let (end, int_ok) = scan_digits(bytes, start, |b| b.is_ascii_digit());
        let has_int = end > start;
        i = end;
        let mut ok = int_ok || !has_int;
        if at(i) == Some(b'.') {
            let (end, frac_ok) = scan_digits(bytes, i.saturating_add(1), |b| b.is_ascii_digit());
            let has_frac = end > i.saturating_add(1);
            // `1.` は許すが、`.` だけ・`1._5` は許さない。
            ok = ok && (frac_ok || !has_frac) && (has_int || has_frac);
            i = end;
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            let mut j = i.saturating_add(1);
            if matches!(at(j), Some(b'+' | b'-')) {
                j = j.saturating_add(1);
            }
            let (end, exp_ok) = scan_digits(bytes, j, |b| b.is_ascii_digit());
            ok = ok && exp_ok;
            i = end;
        }
        if !ok {
            error = Some("数値リテラルの形が不正");
        }
    }
    if at(i) == Some(b'n') {
        i = i.saturating_add(1);
        error = Some("BigInt リテラルは使えない");
    }
    (i.max(next), error)
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    nesting: Nesting,
    /// 直前のトークンのあとで、式が始まりうるか。
    expr_start: bool,
    errors: Vec<LexError>,
    fatal: bool,
}

impl<'a> Lexer<'a> {
    fn peek(&self, off: usize) -> Option<u8> {
        self.pos
            .checked_add(off)
            .and_then(|i| self.bytes.get(i).copied())
    }

    fn advance(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.bytes.len());
    }

    fn error(&mut self, code: Code, start: usize, end: usize, message: &'static str) {
        self.errors.push(LexError {
            code,
            start,
            end: end.min(self.bytes.len()).max(start),
            message,
        });
    }

    fn fatal(&mut self, code: Code, start: usize, end: usize, message: &'static str) {
        self.error(code, start, end, message);
        self.fatal = true;
    }

    fn depth(&mut self, result: Result<(), TooDeep>) {
        if result.is_err() {
            let start = self.pos;
            let end = start.saturating_add(char_len(self.src, start).max(1));
            self.fatal(Code::NestingTooDeep, start, end, "構文の入れ子が深すぎる");
        }
    }

    fn run(&mut self) {
        while !self.fatal && self.pos < self.bytes.len() {
            match self.nesting.top() {
                FrameKind::JsxTag => self.jsx_tag_step(),
                FrameKind::JsxChildren => self.jsx_children_step(),
                _ => self.code_step(),
            }
        }
    }

    /// 前の位置がそのまま戻ってきたら、少なくとも 1 バイト進める（無限ループの防止）。
    fn ensure_progress(&mut self, before: usize) {
        if self.pos == before {
            self.advance(1);
        }
    }

    fn open(&mut self, kind: FrameKind) {
        // 後置の位置の `(` `[` は呼び出し・添字の連鎖になる。
        if !self.expr_start {
            let r = self.nesting.inc();
            self.depth(r);
        }
        let r = self.nesting.push(kind, self.pos);
        self.depth(r);
        self.advance(1);
        self.expr_start = true;
    }

    fn code_step(&mut self) {
        let before = self.pos;
        let Some(b) = self.peek(0) else {
            return;
        };
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => self.advance(1),
            b'/' if self.peek(1) == Some(b'/') => self.line_comment(),
            b'/' if self.peek(1) == Some(b'*') => self.block_comment(),
            b'"' | b'\'' => {
                self.string(b);
                self.expr_start = false;
            }
            b'`' => {
                let start = self.pos;
                self.advance(1);
                self.template_body(start);
                self.expr_start = false;
            }
            b'0'..=b'9' => self.number(),
            b'.' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => self.number(),
            b'(' => self.open(FrameKind::Paren),
            b'[' => self.open(FrameKind::Bracket),
            b'{' => self.open(FrameKind::Brace),
            b')' | b']' => {
                let kind = if b == b')' {
                    FrameKind::Paren
                } else {
                    FrameKind::Bracket
                };
                self.nesting.pop(kind);
                self.advance(1);
                self.expr_start = false;
            }
            b'}' => {
                self.advance(1);
                if let Some(frame) = self.nesting.pop(FrameKind::TemplateExpr) {
                    self.template_body(frame.start);
                } else if self.nesting.pop(FrameKind::Brace).is_some()
                    && self.nesting.top() == FrameKind::Root
                {
                    // トップレベルに戻る `}`（関数・interface の終わり）は文の区切り。
                    // ここで数え直さないと、宣言が並ぶだけで連鎖がたまる。
                    // 式の途中のオブジェクトリテラルの `}` で数え直されても、変換器の深さの検査が止める。
                    self.nesting.reset();
                }
                self.expr_start = false;
            }
            b',' | b';' => {
                self.nesting.reset();
                self.advance(1);
                self.expr_start = true;
            }
            b'<' if self.expr_start
                && self.peek(1).is_some_and(|c| c == b'>' || is_ident_start(c)) =>
            {
                self.jsx_open();
            }
            b'/' if self.expr_start => self.regex(),
            b'\\' => {
                let end = self.pos.saturating_add(1);
                self.error(
                    Code::ForbiddenCharacter,
                    self.pos,
                    end,
                    "文字列の外に `\\` は書けない",
                );
                self.advance(1);
            }
            b':' => {
                self.advance(1);
                self.expr_start = true;
            }
            b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'=' | b'!' | b'&' | b'|' | b'?'
            | b'~' | b'^' | b'.' => {
                let r = self.nesting.inc();
                self.depth(r);
                self.advance(1);
                self.expr_start = b != b'.';
            }
            b if is_ident_start(b) => self.word(),
            _ => self.advance(1),
        }
        self.ensure_progress(before);
    }

    fn word(&mut self) {
        let start = self.pos;
        while self.peek(0).is_some_and(is_ident_continue) {
            self.advance(1);
        }
        let word = self.bytes.get(start..self.pos).unwrap_or_default();
        if NESTING_KEYWORDS.contains(&word) {
            let r = self.nesting.inc();
            self.depth(r);
        }
        self.expr_start = EXPR_KEYWORDS.contains(&word);
    }

    fn line_comment(&mut self) {
        while self.peek(0).is_some_and(|b| b != b'\n') {
            self.advance(1);
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        self.advance(2);
        loop {
            match self.peek(0) {
                None => {
                    let end = self.bytes.len();
                    self.fatal(
                        Code::UnterminatedComment,
                        start,
                        end,
                        "ブロックコメントが閉じていない",
                    );
                    return;
                }
                Some(b'*') if self.peek(1) == Some(b'/') => {
                    self.advance(2);
                    return;
                }
                Some(_) => self.advance(1),
            }
        }
    }

    fn string(&mut self, quote: u8) {
        let start = self.pos;
        self.advance(1);
        loop {
            match self.peek(0) {
                None | Some(b'\n' | b'\r') => {
                    let end = self.pos;
                    self.fatal(Code::UnterminatedString, start, end, "文字列が閉じていない");
                    return;
                }
                Some(b'\\') => self.escape(false),
                Some(b) if b == quote => {
                    self.advance(1);
                    return;
                }
                Some(_) => self.advance(1),
            }
        }
    }

    fn escape(&mut self, template: bool) {
        let start = self.pos;
        let (len, ok) = scan_escape(self.bytes, start, template);
        // 不正なエスケープの span は、続く文字の終わりまでにする。
        let mut end = start.saturating_add(len);
        if !ok {
            if len == 2 {
                end = start
                    .saturating_add(1)
                    .saturating_add(char_len(self.src, start.saturating_add(1)).max(1));
            }
            self.error(Code::InvalidEscape, start, end, "使えないエスケープ");
        }
        self.pos = end.min(self.bytes.len());
    }

    /// テンプレート文字列の本体を、`${` か閉じる `` ` `` まで読む。`start` は開始の `` ` `` の位置。
    fn template_body(&mut self, start: usize) {
        loop {
            match self.peek(0) {
                None => {
                    let end = self.bytes.len();
                    self.fatal(
                        Code::UnterminatedTemplate,
                        start,
                        end,
                        "テンプレート文字列が閉じていない",
                    );
                    return;
                }
                Some(b'`') => {
                    self.advance(1);
                    return;
                }
                Some(b'$') if self.peek(1) == Some(b'{') => {
                    let r = self.nesting.push(FrameKind::TemplateExpr, start);
                    self.depth(r);
                    self.advance(2);
                    self.expr_start = true;
                    return;
                }
                Some(b'\\') => self.escape(true),
                Some(_) => self.advance(1),
            }
        }
    }

    fn number(&mut self) {
        let start = self.pos;
        let (end, error) = scan_number(self.bytes, start);
        if let Some(message) = error {
            self.error(Code::UnsupportedLiteral, start, end, message);
        }
        self.pos = end.min(self.bytes.len());
        self.expr_start = false;
    }

    fn regex(&mut self) {
        let start = self.pos;
        self.advance(1);
        let mut in_class = false;
        while let Some(b) = self.peek(0) {
            match b {
                b'\n' | b'\r' => break,
                b'\\' => self.advance(2),
                b'[' => {
                    in_class = true;
                    self.advance(1);
                }
                b']' => {
                    in_class = false;
                    self.advance(1);
                }
                b'/' if !in_class => {
                    self.advance(1);
                    break;
                }
                _ => self.advance(1),
            }
        }
        while self.peek(0).is_some_and(is_ident_continue) {
            self.advance(1);
        }
        let end = self.pos;
        self.error(
            Code::UnsupportedLiteral,
            start,
            end,
            "正規表現リテラルは使えない",
        );
        self.expr_start = false;
    }

    fn jsx_open(&mut self) {
        let start = self.pos;
        if self.peek(1) == Some(b'>') {
            let r = self.nesting.push(FrameKind::JsxChildren, start);
            self.depth(r);
            self.advance(2);
        } else {
            let r = self.nesting.push(FrameKind::JsxTag, start);
            self.depth(r);
            self.advance(1);
        }
    }

    /// 要素が閉じたあと。親が JSX の子でなければ、要素は式として終わる。
    fn after_element(&mut self) {
        self.expr_start = false;
    }

    fn jsx_tag_step(&mut self) {
        let before = self.pos;
        let Some(b) = self.peek(0) else {
            return;
        };
        match b {
            b'/' if self.peek(1) == Some(b'>') => {
                self.advance(2);
                self.nesting.pop(FrameKind::JsxTag);
                self.after_element();
            }
            b'/' if self.peek(1) == Some(b'/') => self.line_comment(),
            b'/' if self.peek(1) == Some(b'*') => self.block_comment(),
            b'>' => {
                self.advance(1);
                self.nesting.replace_top(FrameKind::JsxChildren);
            }
            b'{' => {
                self.expr_start = true;
                self.open(FrameKind::Brace);
            }
            b'"' | b'\'' => self.jsx_attr_string(b),
            b'<' => self.jsx_open(),
            // 正しい開始タグには現れない記号。`<T,>() => …` のように、JSX に見えて実は
            // コードだった場合でも深さを数え漏らさないよう、連鎖として数える。
            b'(' | b'[' | b'!' | b'+' | b'~' | b'?' | b'*' | b'%' | b'&' | b'|' | b'^' | b',' => {
                let r = self.nesting.inc();
                self.depth(r);
                self.advance(1);
            }
            _ => self.advance(1),
        }
        self.ensure_progress(before);
    }

    fn jsx_children_step(&mut self) {
        let before = self.pos;
        let Some(b) = self.peek(0) else {
            return;
        };
        match b {
            b'<' if self.peek(1) == Some(b'/') => {
                while self.peek(0).is_some_and(|c| c != b'>') {
                    self.advance(1);
                }
                self.advance(1);
                self.nesting.pop(FrameKind::JsxChildren);
                self.after_element();
            }
            b'<' => self.jsx_open(),
            b'{' => {
                self.expr_start = true;
                self.open(FrameKind::Brace);
            }
            // テキスト子は CDL では誤り（DECISIONS A1）。JSX に見えて実はコードだった場合に
            // 備えて、テキストの記号は連鎖として数える。
            b if b.is_ascii_punctuation() => {
                let r = self.nesting.inc();
                self.depth(r);
                self.advance(1);
            }
            _ => self.advance(1),
        }
        self.ensure_progress(before);
    }

    /// JSX の属性文字列。エスケープはない（DECISIONS C13）。
    fn jsx_attr_string(&mut self, quote: u8) {
        let start = self.pos;
        self.advance(1);
        loop {
            match self.peek(0) {
                None => {
                    let end = self.bytes.len();
                    self.fatal(Code::UnterminatedString, start, end, "文字列が閉じていない");
                    return;
                }
                Some(b'&') => {
                    let at = self.pos;
                    self.error(
                        Code::JsxAttrCharRef,
                        at,
                        at.saturating_add(1),
                        "JSX の属性文字列に `&` は書けない（`{\"…\"}` と書く）",
                    );
                    self.advance(1);
                }
                Some(b) if b == quote => {
                    self.advance(1);
                    return;
                }
                Some(_) => self.advance(1),
            }
        }
    }
}

/// 位置に関係なく使えない文字（DECISIONS C9）。
fn forbidden_chars(src: &str, errors: &mut Vec<LexError>) {
    let mut chars = src.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let message = match c {
            '\u{FEFF}' => "BOM（U+FEFF）は使えない",
            '\u{2028}' => "U+2028 は使えない",
            '\u{2029}' => "U+2029 は使えない",
            '\r' if chars.peek().map(|&(_, n)| n) != Some('\n') => {
                "単独の CR は使えない（改行は LF か CRLF）"
            }
            _ => continue,
        };
        errors.push(LexError {
            code: Code::ForbiddenCharacter,
            start: i,
            end: i.saturating_add(c.len_utf8()),
            message,
        });
    }
}

/// ソースの字句を検査する。誤りがなければ空の列を返す。
pub(crate) fn check(src: &str, max_depth: u32) -> Vec<LexError> {
    let mut errors = Vec::new();
    forbidden_chars(src, &mut errors);
    let mut lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        nesting: Nesting::new(max_depth),
        expr_start: true,
        errors: Vec::new(),
        fatal: false,
    };
    lexer.run();
    errors.extend(lexer.errors);
    errors.sort_by_key(|e| (e.start, e.end));
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str) -> Vec<(Code, usize, usize)> {
        check(src, 128)
            .into_iter()
            .map(|e| (e.code, e.start, e.end))
            .collect()
    }

    #[test]
    fn accepts_ordinary_cdl() {
        let src = r#""use cdl 1";
import { Contrast, Keybind } from "host:contrast";
const BASE = px(8);
const s = 'it\'s';
const t = `mod+${BASE}\` \${x} \u{1F600}`;
const n = [1_000, 0xff, 0b1010, 1.5, .5, 1e-3, 2.];
export default (
  <Contrast>
    {/* comment */}
    <Keybind keys="a\nb" action='x' />
    {[1].map((b) => <Keybind keys={`${b}`} action="it's" />)}
  </Contrast>
);
"#;
        assert_eq!(codes(src), []);
    }

    #[test]
    fn reports_unterminated_string_and_template_and_comment() {
        assert_eq!(
            codes("const a = \"abc\n;"),
            [(Code::UnterminatedString, 10, 14)]
        );
        assert_eq!(
            codes("const a = `abc"),
            [(Code::UnterminatedTemplate, 10, 14)]
        );
        assert_eq!(
            codes("const a = `x${`y${1}"),
            [(Code::UnterminatedTemplate, 14, 20)]
        );
        assert_eq!(codes("/* x"), [(Code::UnterminatedComment, 0, 4)]);
    }

    #[test]
    fn reports_invalid_escapes() {
        assert_eq!(
            codes(r#"const a = "\x41";"#),
            [(Code::InvalidEscape, 11, 13)]
        );
        assert_eq!(codes(r#"const a = "\0";"#), [(Code::InvalidEscape, 11, 13)]);
        assert_eq!(
            codes(r#"const a = "\u0041";"#),
            [(Code::InvalidEscape, 11, 13)]
        );
        assert_eq!(
            codes(r#"const a = "\u{D800}";"#),
            [(Code::InvalidEscape, 11, 19)]
        );
        assert_eq!(
            codes(r#"const a = "\u{110000}";"#),
            [(Code::InvalidEscape, 11, 21)]
        );
        assert_eq!(
            codes(r#"const a = "\u{}";"#),
            [(Code::InvalidEscape, 11, 15)]
        );
        assert_eq!(codes(r#"const a = "\`";"#), [(Code::InvalidEscape, 11, 13)]);
        assert_eq!(
            codes(r#"const a = "\あ";"#),
            [(Code::InvalidEscape, 11, 15)]
        );
    }

    #[test]
    fn reports_unsupported_numeric_forms() {
        for src in [
            "0o17", "017", "08", "0_1", "1n", "0xffn", "1__0", "1_", "0x", "1e", "1._5",
        ] {
            let got = codes(&format!("const a = {src};"));
            assert_eq!(
                got,
                [(Code::UnsupportedLiteral, 10, 10 + src.len())],
                "{src}"
            );
        }
    }

    #[test]
    fn reports_forbidden_characters() {
        assert_eq!(
            codes("\u{FEFF}const a = 1;"),
            [(Code::ForbiddenCharacter, 0, 3)]
        );
        assert_eq!(
            codes("const a = 1;\rconst b = 2;"),
            [(Code::ForbiddenCharacter, 12, 13)]
        );
        assert_eq!(codes("const a = 1;\r\n"), []);
        assert_eq!(
            codes("const a\u{2028} = 1;"),
            [(Code::ForbiddenCharacter, 7, 10)]
        );
        assert_eq!(
            codes("const \\u0061 = 1;"),
            [(Code::ForbiddenCharacter, 6, 7)]
        );
    }

    #[test]
    fn regex_literal_is_reported_without_confusing_strings() {
        assert_eq!(
            codes("const r = /a\"b(/g;"),
            [(Code::UnsupportedLiteral, 10, 17)]
        );
        // 割り算は正規表現ではない。
        assert_eq!(codes("const r = a / b / c;"), []);
    }

    #[test]
    fn jsx_text_and_attributes_are_not_lexed_as_code() {
        // JSX のテキストの `'` は文字列の始まりではない。
        assert_eq!(codes("const e = <A>it's {x}</A>;"), []);
        // 属性文字列にエスケープはない（`\"` で閉じる）。
        assert_eq!(codes(r#"const e = <A k="a\" />;"#), []);
        assert_eq!(
            codes("const e = <A k=\"a &amp; b\" />;"),
            [(Code::JsxAttrCharRef, 18, 19)]
        );
        // 比較の `<` は JSX ではない。
        assert_eq!(codes("const c = a < b && c > 'd';"), []);
    }

    #[test]
    fn nesting_limit_counts_brackets_jsx_and_chains() {
        let deep = |open: &str, n: usize, close: &str| {
            format!("const a = {}x{};", open.repeat(n), close.repeat(n))
        };
        let too_deep = |src: &str| {
            codes(src)
                .iter()
                .any(|(c, _, _)| *c == Code::NestingTooDeep)
        };
        // `const a =` の `=` も連鎖の 1 つに数える。
        assert!(!too_deep(&deep("(", 127, ")")));
        assert!(too_deep(&deep("(", 128, ")")));
        assert!(too_deep(&deep("[", 129, "]")));
        assert!(too_deep(&deep("!", 129, "")));
        assert!(too_deep(&format!("const a = x{};", " + x".repeat(129))));
        assert!(too_deep(&format!("const a = x{};", ".y".repeat(129))));
        assert!(too_deep(&format!("const a = f{};", "()".repeat(129))));
        assert!(too_deep(&format!("const a = {}0;", "c ? 1 : ".repeat(129))));
        assert!(too_deep(&format!(
            "const a = {}x{};",
            "`${".repeat(129),
            "}`".repeat(129)
        )));
        assert!(too_deep(&format!(
            "const a = {}{};",
            "<A>".repeat(129),
            "</A>".repeat(129)
        )));
        // JSX に見える型引数つきのアロー関数でも、深さを数え漏らさない。
        assert!(too_deep(&format!(
            "const f = <T,>(x: T) => {}x{};",
            "(".repeat(200),
            ")".repeat(200)
        )));
        assert!(too_deep(&format!(
            "const f = <T extends U>(x: T) => {}x;",
            "!".repeat(200)
        )));
        // トップレベルで閉じる `}` でも数え直す（宣言が多いだけのファイルを弾かない）。
        assert!(!too_deep(
            &"function f(n: number): number { return n; }\ninterface I { a: number }\n".repeat(500)
        ));
        // `,` と `;` で数え直す。
        assert!(!too_deep(&format!(
            "const a = [{}];",
            "x + 1, ".repeat(1000)
        )));
        assert!(!too_deep(&"const a = x + 1;\n".repeat(1000)));
    }

    #[test]
    fn huge_inputs_stop_at_the_limit() {
        for unit in ["(", "!", "[", "{", "<A>", "`${", "a.", "-", "x ? y : "] {
            let src = format!("const a = {}", unit.repeat(1 << 18));
            let errors = check(&src, 128);
            assert!(
                errors.iter().any(|e| e.code == Code::NestingTooDeep),
                "{unit}"
            );
        }
    }

    #[test]
    fn scan_number_reports_end_and_error() {
        assert_eq!(scan_number(b"1_000;", 0), (5, None));
        assert_eq!(scan_number(b"0x1F", 0), (4, None));
        assert_eq!(scan_number(b".5e+3", 0), (5, None));
        assert_eq!(scan_number(b"0", 0), (1, None));
        assert!(scan_number(b"0o7", 0).1.is_some());
    }
}

/// Kani のハーネス（DECISIONS F5: K1–K4）。
///
/// 対象は自前の字句検査（[`check`]）だけ。oxc での構文解析と、その結果の AST の変換は
/// 対象外（oxc は規模が大きくアリーナと unsafe を使うため、Kani では検証できない）。
/// その経路の非パニックは proptest（`tests/proptest_syntax.rs` の P1）で確かめる。
#[cfg(kani)]
mod verification {
    use super::*;

    /// K1: 8 バイト以下の任意の UTF-8 の入力で、字句検査はパニックせず、
    /// 誤りの span は入力の範囲内にある（oxc に渡す前の段階だけ）。
    #[kani::proof]
    #[kani::unwind(10)]
    fn lexer_never_panics() {
        let bytes: [u8; 8] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        if let Some(Ok(src)) = bytes.get(..len).map(core::str::from_utf8) {
            for e in check(src, 4) {
                assert!(e.start <= e.end && e.end <= src.len());
            }
        }
    }

    /// K2: エスケープの検査はパニックせず、入力の外を読まない。
    #[kani::proof]
    #[kani::unwind(10)]
    fn escape_scan_is_bounded() {
        let bytes: [u8; 8] = kani::any();
        let template: bool = kani::any();
        let (len, _) = scan_escape(&bytes, 0, template);
        assert!(len >= 1 && len <= bytes.len().saturating_add(1));
    }

    /// K3: 数値リテラルの検査はパニックせず、少なくとも 1 バイト進む。
    #[kani::proof]
    #[kani::unwind(10)]
    fn number_scan_is_bounded() {
        let bytes: [u8; 8] = kani::any();
        let (end, _) = scan_number(&bytes, 0);
        assert!(end >= 1 && end <= bytes.len());
    }

    /// K4: 入れ子の帳簿は上限を超えたら必ず `TooDeep` を返し、数はあふれない。
    #[kani::proof]
    #[kani::unwind(8)]
    fn nesting_reports_overflow() {
        let max: u32 = kani::any();
        kani::assume(max < 4);
        let mut n = Nesting::new(max);
        for _ in 0..6 {
            let op: u8 = kani::any();
            let r = match op % 4 {
                0 => n.push(FrameKind::Paren, 0),
                1 => n.inc(),
                2 => {
                    n.pop(FrameKind::Paren);
                    Ok(())
                }
                _ => {
                    n.reset();
                    Ok(())
                }
            };
            assert_eq!(r.is_err(), n.depth > max);
        }
    }
}
