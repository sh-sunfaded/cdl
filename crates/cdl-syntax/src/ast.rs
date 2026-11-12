//! CDL の AST。`oxc_*` の型をここから外へ漏らさない。
//!
//! 文法は SPEC 第 5 章に従う。括弧は AST に残さない。
//! すべてのノードは、元のソース上の位置（span）を持つ。

/// ソース上の位置（バイトオフセット）。ファイルは [`Module`] が持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// 1 つの `.cdl` ファイル。
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub file: String,
    pub edition: u32,
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub exported: bool,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Import(Import),
    Const(ConstDecl),
    TypeAlias(TypeAlias),
    Interface(Interface),
    Function(FunctionDecl),
    /// `export default expr;`（`exported` は常に真）
    ExportDefault(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub type_only: bool,
    pub names: Vec<Ident>,
    pub source: StrLit,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrLit {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Option<Type>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeAlias {
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub name: Ident,
    pub members: Vec<PropSig>,
    pub span: Span,
}

/// `interface` と型リテラルのメンバー `k?: T`。
#[derive(Clone, Debug, PartialEq)]
pub struct PropSig {
    pub key: Ident,
    pub optional: bool,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Block,
    pub span: Span,
}

/// 引数。型注釈の有無は意味解析で検査する（DECISIONS A3）。
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub pattern: ParamPattern,
    pub ty: Option<Type>,
    pub span: Span,
}

/// 引数の形。1 階層のオブジェクトパターン `{ a, b = 既定値 }` を許す（DECISIONS R4）。
#[derive(Clone, Debug, PartialEq)]
pub enum ParamPattern {
    Ident(Ident),
    /// `{ a, b = expr }`。別名・入れ子・rest はない。
    Object(Vec<PatternProp>, Span),
}

/// オブジェクトパターンの 1 項目 `name` / `name = default`。
#[derive(Clone, Debug, PartialEq)]
pub struct PatternProp {
    pub name: Ident,
    pub default: Option<Expr>,
    pub span: Span,
}

impl Param {
    /// この引数が束縛する名前（オブジェクトパターンなら各項目の名前）。
    pub fn bound_names(&self) -> Vec<&Ident> {
        match &self.pattern {
            ParamPattern::Ident(id) => vec![id],
            ParamPattern::Object(props, _) => props.iter().map(|p| &p.name).collect(),
        }
    }
}

/// `{ const …; stmt }`
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub consts: Vec<ConstDecl>,
    pub stmt: Box<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Return(Expr, Span),
    If {
        cond: Expr,
        then: Block,
        otherwise: Else,
        span: Span,
    },
    Switch {
        scrutinee: Expr,
        cases: Vec<Case>,
        default: Option<Box<Stmt>>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Else {
    Block(Block),
    If(Box<Stmt>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub test: Lit,
    pub body: Stmt,
    pub span: Span,
}

/// リテラル（`switch` の `case` とリテラル型で使う）。
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Number(f64, Span),
    String(String, Span),
    Bool(bool, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Boolean(Span),
    Number(Span),
    String(Span),
    Undefined(Span),
    Literal(Lit),
    /// `Name` / `Name<A, B>`
    Named {
        name: Ident,
        args: Vec<Type>,
        span: Span,
    },
    Array(Box<Type>, Span),
    Tuple(Vec<Type>, Span),
    Object(Vec<PropSig>, Span),
    Union(Vec<Type>, Span),
    Function {
        params: Vec<(Ident, Type)>,
        ret: Box<Type>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Number(f64),
    String(String),
    Bool(bool),
    Undefined,
    /// テンプレート文字列。`quasis.len() == exprs.len() + 1`。
    Template {
        quasis: Vec<String>,
        exprs: Vec<Expr>,
    },
    Ident(String),
    Array(Vec<ArrayElem>),
    Object(Vec<ObjProp>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        cons: Box<Expr>,
        alt: Box<Expr>,
    },
    /// `a.b` / `a?.b`
    Member {
        object: Box<Expr>,
        prop: Ident,
        optional: bool,
    },
    /// `a[i]`
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Arrow {
        params: Vec<Param>,
        ret: Option<Type>,
        body: ArrowBody,
    },
    Satisfies {
        expr: Box<Expr>,
        ty: Type,
    },
    Element(Element),
    /// `<>…</>`
    Fragment(Vec<Child>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Coalesce,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArrayElem {
    Expr(Expr),
    /// `...expr`（span は `...` を含む）
    Spread(Expr, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjProp {
    KeyValue {
        key: Ident,
        value: Expr,
        span: Span,
    },
    /// `{ keys }`
    Shorthand(Ident),
    /// `...expr`（span は `...` を含む）
    Spread(Expr, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Block),
}

/// `<Name …>…</Name>`
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: Ident,
    pub attrs: Vec<Attr>,
    pub children: Vec<Child>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Attr {
    /// `name`（値なし） / `name="…"` / `name={expr}`
    Named {
        name: Ident,
        value: Option<AttrValue>,
        span: Span,
    },
    /// `{...expr}`（span は `{` から `}` まで）
    Spread(Expr, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    String(StrLit),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Child {
    Element(Element, Span),
    Fragment(Vec<Child>, Span),
    /// `{expr}`（span は `{` から `}` まで）
    Expr(Expr, Span),
    /// テキスト子。値は JSX の空白規則で正規化済みで、空でない（DECISIONS R1）。
    Text(String, Span),
}
