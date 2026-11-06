//! 構文解析の性質（DECISIONS F5・§4 の P1 と P2）。
//!
//! - P1: 任意の入力でパニックしない（oxc の経路を含む）。
//! - P2: 生成した AST を印字して読み直すと、元の AST に戻る（span は比べない）。

// 統合テストの補助関数は clippy.toml の「テストでは許す」の対象外なので、ここで許す（DECISIONS F3）。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fmt::Write as _;

use cdl_syntax::ast::*;
use cdl_syntax::{Limits, parse};
use proptest::prelude::*;

// ---- P1 ---------------------------------------------------------------------

/// CDL らしい断片。組み合わせて、構文解析の奥まで届く入力を作る。
const TOKENS: &[&str] = &[
    "\"use cdl 1\";",
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
    "let",
    "class",
    "null",
    "undefined",
    "true",
    "x",
    "Foo",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "<",
    ">",
    "</",
    "/>",
    "<>",
    "</>",
    "=",
    "=>",
    ":",
    ";",
    ",",
    ".",
    "?.",
    "?",
    "??",
    "&&",
    "||",
    "!",
    "+",
    "-",
    "*",
    "/",
    "%",
    "===",
    "!==",
    "...",
    "\"s\"",
    "'t'",
    "`a${",
    "}`",
    "`",
    "1",
    "0x1f",
    "1.5e3",
    "0o7",
    "1n",
    "\\",
    "/*",
    "*/",
    "//",
    "\n",
    "\r\n",
    "\r",
    "&",
    "#",
    "@",
    "\u{2028}",
    "\u{FEFF}",
    "あ",
];

proptest! {
    // 回数は環境変数 PROPTEST_CASES で変える（既定 256）。
    #[test]
    fn arbitrary_bytes_do_not_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let src = String::from_utf8_lossy(&bytes);
        let _ = parse("main.cdl", &src, &Limits::default());
    }

    #[test]
    fn token_soup_does_not_panic(tokens in proptest::collection::vec(proptest::sample::select(TOKENS), 0..300)) {
        let src = tokens.join(" ");
        let _ = parse("main.cdl", &src, &Limits::default());
        let src = tokens.concat();
        let _ = parse("main.cdl", &src, &Limits::default());
    }
}

// ---- P2: 印字 ------------------------------------------------------------------

fn string_lit(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20
                || c == '\u{7f}'
                || c == '\u{2028}'
                || c == '\u{2029}'
                || c == '\u{FEFF}' =>
            {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn template_part(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20
                || c == '\u{7f}'
                || c == '\u{2028}'
                || c == '\u{2029}'
                || c == '\u{FEFF}' =>
            {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

fn number(n: f64) -> String {
    format!("{n:?}")
}

fn lit(l: &Lit) -> String {
    match l {
        Lit::Number(n, _) => number(*n),
        Lit::String(s, _) => string_lit(s),
        Lit::Bool(b, _) => b.to_string(),
    }
}

fn ty(t: &Type) -> String {
    match t {
        Type::Boolean(_) => "boolean".to_owned(),
        Type::Number(_) => "number".to_owned(),
        Type::String(_) => "string".to_owned(),
        Type::Undefined(_) => "undefined".to_owned(),
        Type::Literal(l) => lit(l),
        Type::Named { name, args, .. } if args.is_empty() => name.name.clone(),
        Type::Named { name, args, .. } => {
            format!(
                "{}<{}>",
                name.name,
                args.iter().map(ty).collect::<Vec<_>>().join(", ")
            )
        }
        Type::Array(t, _) => format!("({})[]", ty(t)),
        Type::Tuple(ts, _) => format!("[{}]", ts.iter().map(ty).collect::<Vec<_>>().join(", ")),
        Type::Object(members, _) => format!("{{ {} }}", props(members)),
        Type::Union(ts, _) => ts
            .iter()
            .map(|t| format!("({})", ty(t)))
            .collect::<Vec<_>>()
            .join(" | "),
        // 関数型は戻り値型の位置などで曖昧になるので、いつも括弧で囲む。
        Type::Function { params, ret, .. } => format!(
            "(({}) => ({}))",
            params
                .iter()
                .map(|(n, t)| format!("{}: {}", n.name, ty(t)))
                .collect::<Vec<_>>()
                .join(", "),
            ty(ret)
        ),
    }
}

fn props(members: &[PropSig]) -> String {
    members
        .iter()
        .map(|m| {
            format!(
                "{}{}: {};",
                m.key.name,
                if m.optional { "?" } else { "" },
                ty(&m.ty)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn param(p: &Param) -> String {
    match &p.ty {
        Some(t) => format!("{}: {}", p.name.name, ty(t)),
        None => p.name.name.clone(),
    }
}

fn params(ps: &[Param]) -> String {
    ps.iter().map(param).collect::<Vec<_>>().join(", ")
}

fn expr(e: &Expr) -> String {
    let inner = match &e.kind {
        ExprKind::Number(n) => number(*n),
        ExprKind::String(s) => string_lit(s),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Undefined => "undefined".to_owned(),
        ExprKind::Ident(name) => name.clone(),
        ExprKind::Template { quasis, exprs } => {
            let mut out = String::from("`");
            for (i, q) in quasis.iter().enumerate() {
                out.push_str(&template_part(q));
                if let Some(e) = exprs.get(i) {
                    let _ = write!(out, "${{{}}}", expr(e));
                }
            }
            out.push('`');
            out
        }
        ExprKind::Array(elems) => format!(
            "[{}]",
            elems
                .iter()
                .map(|e| match e {
                    ArrayElem::Expr(e) => expr(e),
                    ArrayElem::Spread(e, _) => format!("...{}", expr(e)),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ExprKind::Object(ps) => format!(
            "{{ {} }}",
            ps.iter()
                .map(|p| match p {
                    ObjProp::KeyValue { key, value, .. } =>
                        format!("{}: {}", key.name, expr(value)),
                    ObjProp::Shorthand(k) => k.name.clone(),
                    ObjProp::Spread(e, _) => format!("...{}", expr(e)),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ExprKind::Unary { op, expr: e } => {
            format!("{}{}", if *op == UnaryOp::Neg { "-" } else { "!" }, expr(e))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let op = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Rem => "%",
                BinaryOp::Eq => "===",
                BinaryOp::Ne => "!==",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
            };
            format!("{} {op} {}", expr(lhs), expr(rhs))
        }
        ExprKind::Logical { op, lhs, rhs } => {
            let op = match op {
                LogicalOp::And => "&&",
                LogicalOp::Or => "||",
                LogicalOp::Coalesce => "??",
            };
            format!("{} {op} {}", expr(lhs), expr(rhs))
        }
        ExprKind::Conditional { test, cons, alt } => {
            format!("{} ? {} : {}", expr(test), expr(cons), expr(alt))
        }
        ExprKind::Member {
            object,
            prop,
            optional,
        } => {
            format!(
                "{}{}{}",
                expr(object),
                if *optional { "?." } else { "." },
                prop.name
            )
        }
        ExprKind::Index { object, index } => format!("{}[{}]", expr(object), expr(index)),
        ExprKind::Call { callee, args } => format!(
            "{}({})",
            expr(callee),
            args.iter().map(expr).collect::<Vec<_>>().join(", ")
        ),
        ExprKind::Arrow {
            params: ps,
            ret,
            body,
        } => {
            let ret = ret
                .as_ref()
                .map(|t| format!(": {}", ty(t)))
                .unwrap_or_default();
            let body = match body {
                ArrowBody::Expr(e) => expr(e),
                ArrowBody::Block(b) => block(b),
            };
            format!("({}){ret} => {body}", params(ps))
        }
        ExprKind::Satisfies { expr: e, ty: t } => format!("{} satisfies {}", expr(e), ty(t)),
        ExprKind::Element(el) => element(el),
        ExprKind::Fragment(children) => format!("<>{}</>", children_src(children)),
    };
    // 括弧は AST に残らないので、いつも付けて優先順位の違いを避ける。
    format!("({inner})")
}

fn element(el: &Element) -> String {
    let mut out = format!("<{}", el.name.name);
    for a in &el.attrs {
        match a {
            Attr::Named {
                name, value: None, ..
            } => {
                let _ = write!(out, " {}", name.name);
            }
            Attr::Named {
                name,
                value: Some(AttrValue::String(s)),
                ..
            } => {
                let _ = write!(out, " {}=\"{}\"", name.name, s.value);
            }
            Attr::Named {
                name,
                value: Some(AttrValue::Expr(e)),
                ..
            } => {
                let _ = write!(out, " {}={{{}}}", name.name, expr(e));
            }
            Attr::Spread(e, _) => {
                let _ = write!(out, " {{...{}}}", expr(e));
            }
        }
    }
    if el.children.is_empty() {
        out.push_str(" />");
    } else {
        let _ = write!(out, ">{}</{}>", children_src(&el.children), el.name.name);
    }
    out
}

fn children_src(children: &[Child]) -> String {
    children
        .iter()
        .map(|c| match c {
            Child::Element(el, _) => element(el),
            Child::Fragment(cs, _) => format!("<>{}</>", children_src(cs)),
            Child::Expr(e, _) => format!("{{{}}}", expr(e)),
        })
        .collect()
}

fn const_decl(c: &ConstDecl) -> String {
    let t =
        c.ty.as_ref()
            .map(|t| format!(": {}", ty(t)))
            .unwrap_or_default();
    format!("const {}{t} = {};", c.name.name, expr(&c.init))
}

fn block(b: &Block) -> String {
    let consts: String = b
        .consts
        .iter()
        .map(|c| format!("{} ", const_decl(c)))
        .collect();
    format!("{{ {consts}{} }}", stmt(&b.stmt))
}

fn stmt(s: &Stmt) -> String {
    match s {
        Stmt::Return(e, _) => format!("return {};", expr(e)),
        Stmt::If {
            cond,
            then,
            otherwise,
            ..
        } => {
            let otherwise = match otherwise {
                Else::Block(b) => block(b),
                Else::If(s) => stmt(s),
            };
            format!("if ({}) {} else {otherwise}", expr(cond), block(then))
        }
        Stmt::Switch {
            scrutinee,
            cases,
            default,
            ..
        } => {
            let mut out = format!("switch ({}) {{ ", expr(scrutinee));
            for c in cases {
                let _ = write!(out, "case {}: {} ", lit(&c.test), stmt(&c.body));
            }
            if let Some(d) = default {
                let _ = write!(out, "default: {} ", stmt(d));
            }
            out.push('}');
            out
        }
    }
}

fn print(m: &Module) -> String {
    let mut out = String::from("\"use cdl 1\";\n");
    for item in &m.items {
        let export = if item.exported { "export " } else { "" };
        let line = match &item.kind {
            ItemKind::Import(i) => format!(
                "import {}{{ {} }} from {};",
                if i.type_only { "type " } else { "" },
                i.names
                    .iter()
                    .map(|n| n.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                string_lit(&i.source.value)
            ),
            ItemKind::Const(c) => format!("{export}{}", const_decl(c)),
            ItemKind::TypeAlias(t) => format!("{export}type {} = {};", t.name.name, ty(&t.ty)),
            ItemKind::Interface(i) => format!(
                "{export}interface {} {{ {} }}",
                i.name.name,
                props(&i.members)
            ),
            ItemKind::Function(f) => {
                let ret = f
                    .ret
                    .as_ref()
                    .map(|t| format!(": {}", ty(t)))
                    .unwrap_or_default();
                format!(
                    "{export}function {}({}){ret} {}",
                    f.name.name,
                    params(&f.params),
                    block(&f.body)
                )
            }
            ItemKind::ExportDefault(e) => format!("export default {};", expr(e)),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `Debug` の出力から span を消す。
fn without_spans(debug: &str) -> String {
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(i) = rest.find("Span {") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find('}').unwrap();
        out.push_str("Span");
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

// ---- P2: 生成 ------------------------------------------------------------------

const NAMES: &[&str] = &["a", "b", "x", "y", "foo", "bar"];
const KEYS: &[&str] = &["a", "k", "default", "type", "if", "length"];
const TYPE_NAMES: &[&str] = &["T", "Length", "Array", "Record", "Element"];
const TAGS: &[&str] = &["A", "B", "Keybind"];
const ATTRS: &[&str] = &["k", "v", "flag", "enabled", "type"];

fn z() -> Span {
    Span::default()
}

fn id(name: &str) -> Ident {
    Ident {
        name: name.to_owned(),
        span: z(),
    }
}

fn name() -> impl Strategy<Value = Ident> {
    proptest::sample::select(NAMES).prop_map(id)
}

fn key() -> impl Strategy<Value = Ident> {
    proptest::sample::select(KEYS).prop_map(id)
}

fn finite() -> impl Strategy<Value = f64> {
    prop_oneof![
        (0u32..1000).prop_map(f64::from),
        (0.0f64..1e6),
        Just(1e-7),
        Just(1e21),
        Just(0.1)
    ]
}

fn text() -> impl Strategy<Value = String> {
    any::<String>()
}

fn lit_s() -> impl Strategy<Value = Lit> {
    prop_oneof![
        finite().prop_map(|n| Lit::Number(n, z())),
        "[a-z ]{0,6}".prop_map(|s| Lit::String(s, z())),
        any::<bool>().prop_map(|b| Lit::Bool(b, z())),
    ]
}

fn ty_s() -> impl Strategy<Value = Type> {
    let leaf = prop_oneof![
        Just(Type::Boolean(z())),
        Just(Type::Number(z())),
        Just(Type::String(z())),
        Just(Type::Undefined(z())),
        lit_s().prop_map(Type::Literal),
        proptest::sample::select(TYPE_NAMES).prop_map(|n| Type::Named {
            name: id(n),
            args: vec![],
            span: z()
        }),
    ];
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|t| Type::Array(Box::new(t), z())),
            proptest::collection::vec(inner.clone(), 2..4).prop_map(|ts| Type::Union(ts, z())),
            proptest::collection::vec(inner.clone(), 0..3).prop_map(|ts| Type::Tuple(ts, z())),
            proptest::collection::vec((key(), any::<bool>(), inner.clone()), 0..3).prop_map(|ms| {
                Type::Object(
                    ms.into_iter()
                        .map(|(k, o, t)| PropSig {
                            key: k,
                            optional: o,
                            ty: t,
                            span: z(),
                        })
                        .collect(),
                    z(),
                )
            }),
            (
                proptest::sample::select(TYPE_NAMES),
                proptest::collection::vec(inner.clone(), 1..3)
            )
                .prop_map(|(n, args)| Type::Named {
                    name: id(n),
                    args,
                    span: z()
                }),
            (
                proptest::collection::vec((name(), inner.clone()), 0..3),
                inner
            )
                .prop_map(|(ps, r)| Type::Function {
                    params: ps,
                    ret: Box::new(r),
                    span: z()
                }),
        ]
    })
}

fn e(kind: ExprKind) -> Expr {
    Expr { kind, span: z() }
}

fn param_s() -> impl Strategy<Value = Param> {
    (name(), proptest::option::of(ty_s())).prop_map(|(n, t)| Param {
        name: n,
        ty: t,
        span: z(),
    })
}

fn expr_s() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        finite().prop_map(|n| e(ExprKind::Number(n))),
        text().prop_map(|s| e(ExprKind::String(s))),
        any::<bool>().prop_map(|b| e(ExprKind::Bool(b))),
        Just(e(ExprKind::Undefined)),
        proptest::sample::select(NAMES).prop_map(|n| e(ExprKind::Ident(n.to_owned()))),
    ];
    leaf.prop_recursive(4, 32, 4, |inner| {
        let b = |x: Expr| Box::new(x);
        prop_oneof![
            proptest::collection::vec(
                prop_oneof![
                    inner.clone().prop_map(ArrayElem::Expr),
                    inner.clone().prop_map(|x| ArrayElem::Spread(x, z()))
                ],
                0..3
            )
            .prop_map(|xs| e(ExprKind::Array(xs))),
            proptest::collection::vec(
                prop_oneof![
                    (key(), inner.clone()).prop_map(|(k, v)| ObjProp::KeyValue {
                        key: k,
                        value: v,
                        span: z()
                    }),
                    name().prop_map(ObjProp::Shorthand),
                    inner.clone().prop_map(|x| ObjProp::Spread(x, z())),
                ],
                0..3
            )
            .prop_map(|ps| e(ExprKind::Object(ps))),
            (any::<bool>(), inner.clone()).prop_map(move |(neg, x)| e(ExprKind::Unary {
                op: if neg { UnaryOp::Neg } else { UnaryOp::Not },
                expr: b(x)
            })),
            (
                proptest::sample::select(vec![
                    BinaryOp::Add,
                    BinaryOp::Sub,
                    BinaryOp::Mul,
                    BinaryOp::Div,
                    BinaryOp::Rem,
                    BinaryOp::Eq,
                    BinaryOp::Ne,
                    BinaryOp::Lt,
                    BinaryOp::Le,
                    BinaryOp::Gt,
                    BinaryOp::Ge
                ]),
                inner.clone(),
                inner.clone()
            )
                .prop_map(move |(op, l, r)| e(ExprKind::Binary {
                    op,
                    lhs: b(l),
                    rhs: b(r)
                })),
            (
                proptest::sample::select(vec![LogicalOp::And, LogicalOp::Or, LogicalOp::Coalesce]),
                inner.clone(),
                inner.clone()
            )
                .prop_map(move |(op, l, r)| e(ExprKind::Logical {
                    op,
                    lhs: b(l),
                    rhs: b(r)
                })),
            (inner.clone(), inner.clone(), inner.clone()).prop_map(move |(t, c, a)| e(
                ExprKind::Conditional {
                    test: b(t),
                    cons: b(c),
                    alt: b(a)
                }
            )),
            (inner.clone(), key(), any::<bool>()).prop_map(move |(o, p, opt)| e(
                ExprKind::Member {
                    object: b(o),
                    prop: p,
                    optional: opt
                }
            )),
            (inner.clone(), inner.clone()).prop_map(move |(o, i)| e(ExprKind::Index {
                object: b(o),
                index: b(i)
            })),
            (
                inner.clone(),
                proptest::collection::vec(inner.clone(), 0..3)
            )
                .prop_map(move |(c, args)| e(ExprKind::Call { callee: b(c), args })),
            (
                proptest::collection::vec(param_s(), 0..3),
                proptest::option::of(ty_s()),
                inner.clone()
            )
                .prop_map(move |(ps, ret, body)| e(ExprKind::Arrow {
                    params: ps,
                    ret,
                    body: ArrowBody::Expr(b(body))
                })),
            (inner.clone(), ty_s())
                .prop_map(move |(x, t)| e(ExprKind::Satisfies { expr: b(x), ty: t })),
            proptest::collection::vec((text(), inner.clone()), 0..3).prop_flat_map(|parts| {
                text().prop_map(move |last| {
                    let mut quasis: Vec<String> = parts.iter().map(|(q, _)| q.clone()).collect();
                    quasis.push(last);
                    let exprs = parts.iter().map(|(_, x)| x.clone()).collect();
                    e(ExprKind::Template { quasis, exprs })
                })
            }),
            element_s(inner.clone()).prop_map(|el| e(ExprKind::Element(el))),
            proptest::collection::vec(child_s(inner), 0..3)
                .prop_map(|cs| e(ExprKind::Fragment(cs))),
        ]
    })
}

fn element_s(inner: BoxedStrategy<Expr>) -> impl Strategy<Value = Element> {
    let attr = prop_oneof![
        proptest::sample::select(ATTRS).prop_map(|n| Attr::Named {
            span: z(),
            name: id(n),
            value: None
        }),
        (proptest::sample::select(ATTRS), "[a-z ]{0,6}").prop_map(|(n, s)| Attr::Named {
            span: z(),
            name: id(n),
            value: Some(AttrValue::String(StrLit {
                value: s,
                span: z()
            }))
        }),
        (proptest::sample::select(ATTRS), inner.clone()).prop_map(|(n, x)| Attr::Named {
            span: z(),
            name: id(n),
            value: Some(AttrValue::Expr(x))
        }),
        inner.clone().prop_map(|x| Attr::Spread(x, z())),
    ];
    (
        proptest::sample::select(TAGS),
        proptest::collection::vec(attr, 0..3),
        proptest::collection::vec(inner.prop_map(|x| Child::Expr(x, z())), 0..2),
    )
        .prop_map(|(tag, attrs, children)| Element {
            name: id(tag),
            attrs,
            children,
            span: z(),
        })
}

fn child_s(inner: BoxedStrategy<Expr>) -> impl Strategy<Value = Child> {
    prop_oneof![
        inner.clone().prop_map(|x| Child::Expr(x, z())),
        element_s(inner).prop_map(|el| Child::Element(el, z())),
        Just(Child::Fragment(vec![], z())),
    ]
}

fn stmt_s() -> impl Strategy<Value = Stmt> {
    let leaf = expr_s().prop_map(|x| Stmt::Return(x, z()));
    leaf.prop_recursive(2, 8, 3, |inner| {
        let block = (proptest::collection::vec(const_s(), 0..2), inner.clone())
            .prop_map(|(consts, s)| Block {
                consts,
                stmt: Box::new(s),
                span: z(),
            })
            .boxed();
        prop_oneof![
            (expr_s(), block.clone(), block.clone()).prop_map(|(c, t, o)| Stmt::If {
                cond: c,
                then: t,
                otherwise: Else::Block(o),
                span: z()
            }),
            (expr_s(), block.clone(), expr_s(), block.clone(), block).prop_map(
                |(c, t, c2, t2, o)| Stmt::If {
                    cond: c,
                    then: t,
                    otherwise: Else::If(Box::new(Stmt::If {
                        cond: c2,
                        then: t2,
                        otherwise: Else::Block(o),
                        span: z()
                    })),
                    span: z()
                }
            ),
            (
                expr_s(),
                proptest::collection::vec((lit_s(), inner.clone()), 0..3),
                proptest::option::of(inner)
            )
                .prop_map(|(x, cases, d)| Stmt::Switch {
                    scrutinee: x,
                    cases: cases
                        .into_iter()
                        .map(|(t, s)| Case {
                            test: t,
                            body: s,
                            span: z()
                        })
                        .collect(),
                    default: d.map(Box::new),
                    span: z()
                }),
        ]
    })
}

fn const_s() -> impl Strategy<Value = ConstDecl> {
    (name(), proptest::option::of(ty_s()), expr_s()).prop_map(|(n, t, x)| ConstDecl {
        name: n,
        ty: t,
        init: x,
        span: z(),
    })
}

fn item_s() -> impl Strategy<Value = Item> {
    let item = |exported: bool, kind: ItemKind| Item {
        exported,
        kind,
        span: z(),
    };
    prop_oneof![
        (
            any::<bool>(),
            proptest::collection::vec(name(), 1..3),
            "[a-z:./]{1,8}"
        )
            .prop_map(move |(t, names, s)| item(
                false,
                ItemKind::Import(Import {
                    type_only: t,
                    names,
                    source: StrLit {
                        value: s,
                        span: z()
                    },
                    span: z()
                })
            )),
        (any::<bool>(), const_s()).prop_map(move |(x, c)| item(x, ItemKind::Const(c))),
        (any::<bool>(), name(), ty_s()).prop_map(move |(x, n, t)| item(
            x,
            ItemKind::TypeAlias(TypeAlias {
                name: n,
                ty: t,
                span: z()
            })
        )),
        (
            any::<bool>(),
            name(),
            proptest::collection::vec((key(), any::<bool>(), ty_s()), 0..3)
        )
            .prop_map(move |(x, n, ms)| item(
                x,
                ItemKind::Interface(Interface {
                    name: n,
                    members: ms
                        .into_iter()
                        .map(|(k, o, t)| PropSig {
                            key: k,
                            optional: o,
                            ty: t,
                            span: z()
                        })
                        .collect(),
                    span: z()
                })
            )),
        (
            any::<bool>(),
            name(),
            proptest::collection::vec(param_s(), 0..3),
            proptest::option::of(ty_s()),
            proptest::collection::vec(const_s(), 0..2),
            stmt_s()
        )
            .prop_map(move |(x, n, ps, ret, consts, s)| item(
                x,
                ItemKind::Function(FunctionDecl {
                    name: n,
                    params: ps,
                    ret,
                    body: Block {
                        consts,
                        stmt: Box::new(s),
                        span: z()
                    },
                    span: z()
                })
            )),
        expr_s().prop_map(move |x| item(true, ItemKind::ExportDefault(x))),
    ]
}

fn module_s() -> impl Strategy<Value = Module> {
    proptest::collection::vec(item_s(), 0..4).prop_map(|items| Module {
        file: "main.cdl".to_owned(),
        edition: 1,
        items,
        span: z(),
    })
}

proptest! {
    // 回数は環境変数 PROPTEST_CASES で変える（既定 256）。
    #[test]
    fn printed_ast_parses_back_to_itself(m in module_s()) {
        let src = print(&m);
        let parsed = match parse("main.cdl", &src, &Limits::default()) {
            Ok(parsed) => parsed,
            Err(d) => {
                // 入れ子が深すぎる入力は、生成の大きさによっては作られうる。それ以外は誤り。
                prop_assume!(!d.iter().any(|d| d.code == cdl_diag::Code::NestingTooDeep));
                return Err(TestCaseError::fail(format!("{src}\n{d:#?}")));
            }
        };
        prop_assert_eq!(without_spans(&format!("{parsed:?}")), without_spans(&format!("{m:?}")), "{}", src);
        // 読み直した AST のすべてのノードは、元のソースの範囲内の空でない span を持つ。
        let debug = format!("{parsed:?}");
        let mut rest = debug.as_str();
        while let Some(i) = rest.find("Span { start: ") {
            let tail = &rest[i + "Span { start: ".len()..];
            let (start, tail) = tail.split_once(", end: ").unwrap();
            let (end, tail) = tail.split_once(" }").unwrap();
            let (start, end): (usize, usize) = (start.parse().unwrap(), end.parse().unwrap());
            prop_assert!(start < end && src.get(start..end).is_some(), "{start}..{end}\n{src}");
            rest = tail;
        }
    }
}
