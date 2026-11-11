//! 値木を、ホスト側の Rust の型として読み出す参照 API（SPEC 第 11 章、DECISIONS E8）。
//!
//! 型検査を通った値木は、スキーマに合っている。それでもホストの期待と食い違えば
//! （スキーマとホストのコードの不一致など）、要素の位置で E4008 を返す。

use cdl_diag::{Code, Diagnostic};
use cdl_eval::{Element, Value};

/// 長さ（px）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px(pub f64);

/// 時間（ミリ秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Millis(pub i64);

/// 色（RGBA）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

/// 値から読み出せる型。
pub trait FromValue: Sized {
    /// 診断に書く、期待する CDL の型。
    const EXPECTED: &'static str;

    fn from_value(v: &Value) -> Option<Self>;
}

impl FromValue for bool {
    const EXPECTED: &'static str = "boolean";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl FromValue for f64 {
    const EXPECTED: &'static str = "number";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl FromValue for String {
    const EXPECTED: &'static str = "string";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        }
    }
}

impl FromValue for Px {
    const EXPECTED: &'static str = "Length";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Length(n) => Some(Px(*n)),
            _ => None,
        }
    }
}

impl FromValue for Millis {
    const EXPECTED: &'static str = "Duration";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Duration(ms) => Some(Millis(*ms)),
            _ => None,
        }
    }
}

impl FromValue for Rgba {
    const EXPECTED: &'static str = "Color";

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Color(c) => Some(Rgba(*c)),
            _ => None,
        }
    }
}

/// 値の種類。メッセージに値そのものを入れない（大きな値や共有された値で、メッセージが巨大になるため）。
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Length(_) => "Length",
        Value::Duration(_) => "Duration",
        Value::Color(_) => "Color",
        Value::Array(_) => "配列",
        Value::Object(_) => "オブジェクト",
        Value::Element(_) => "要素",
        Value::Function(_) => "関数",
    }
}

/// 値木の要素を読むための参照。
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    element: &'a Element,
}

impl<'a> Node<'a> {
    pub fn new(element: &'a Element) -> Self {
        Node { element }
    }

    /// 組み込みコンポーネントの名前。
    pub fn name(&self) -> &'a str {
        &self.element.name
    }

    /// 要素そのもの（props・子・位置）。
    pub fn element(&self) -> &'a Element {
        self.element
    }

    /// 子の要素（コンポーネントは展開済み）。
    pub fn children(&self) -> impl Iterator<Item = Node<'a>> + 'a {
        self.element.children.iter().map(|c| Node::new(c))
    }

    /// この要素の位置で E4008 を作る。
    pub fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(Code::ConversionFailed, self.element.span.clone(), message)
    }

    /// 必須の props を読む。ないか型が違えば E4008。
    pub fn prop<T: FromValue>(&self, key: &str) -> Result<T, Diagnostic> {
        self.optional_prop(key)?
            .ok_or_else(|| self.error(format!("`<{}>` に `{key}` がない", self.name())))
    }

    /// 省略できる props を読む。値がなければ `None`、型が違えば E4008。
    pub fn optional_prop<T: FromValue>(&self, key: &str) -> Result<Option<T>, Diagnostic> {
        match self.element.props.get(key) {
            None | Some(Value::Undefined) => Ok(None),
            Some(v) => T::from_value(v).map(Some).ok_or_else(|| {
                self.error(format!(
                    "`<{}>` の `{key}` は `{}` が要る（{}）",
                    self.name(),
                    T::EXPECTED,
                    kind(v)
                ))
            }),
        }
    }

    /// 要素が `name` であることを確かめる。違えば E4008。
    pub fn expect(&self, name: &str) -> Result<(), Diagnostic> {
        if self.name() == name {
            Ok(())
        } else {
            Err(self.error(format!(
                "`<{name}>` が要るところに `<{}>` がある",
                self.name()
            )))
        }
    }
}
