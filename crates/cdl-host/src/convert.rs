//! 値木を、ホスト側の Rust の型として読み出す参照 API（SPEC 第 11 章、DECISIONS E8）。
//!
//! 型検査を通った値木は、スキーマに合っている。それでもホストの期待と食い違えば
//! （スキーマとホストのコードの不一致など）、要素の位置で E4008 を返す。

use std::collections::BTreeMap;

use cdl_diag::{Code, Diagnostic, Span};
use cdl_eval::{ChildNode, Element, Value};

/// 長さ（px）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px(pub f64);

/// 時間（ミリ秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Millis(pub i64);

/// 色（RGBA）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

/// props の値から読み出せる型（DECISIONS U4）。`string` / `number` / `boolean` / `Length` / `Duration` /
/// `Color` のほか、`Option<T>`（`T | undefined`）と `Vec<T>`（`T[]`）は、`T` が読めれば読める。
pub trait FromValue: Sized {
    /// 診断に書く、期待する CDL の型。
    fn expected() -> String;

    /// 値を読む。合わなければ `None`。
    fn from_value(v: &Value) -> Option<Self>;
}

impl FromValue for bool {
    fn expected() -> String {
        "boolean".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl FromValue for f64 {
    fn expected() -> String {
        "number".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl FromValue for String {
    fn expected() -> String {
        "string".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        }
    }
}

impl FromValue for Px {
    fn expected() -> String {
        "Length".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Length(n) => Some(Px(*n)),
            _ => None,
        }
    }
}

impl FromValue for Millis {
    fn expected() -> String {
        "Duration".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Duration(ms) => Some(Millis(*ms)),
            _ => None,
        }
    }
}

impl FromValue for Rgba {
    fn expected() -> String {
        "Color".to_owned()
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Color(c) => Some(Rgba(*c)),
            _ => None,
        }
    }
}

/// `T | undefined`。値がない（`undefined`）なら `None`。
impl<T: FromValue> FromValue for Option<T> {
    fn expected() -> String {
        format!("{} | undefined", T::expected())
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Undefined => Some(None),
            other => T::from_value(other).map(Some),
        }
    }
}

/// `T[]`。要素が 1 つでも読めなければ、全体が読めない。
impl<T: FromValue> FromValue for Vec<T> {
    fn expected() -> String {
        format!("{}[]", T::expected())
    }

    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Array(xs) => xs.iter().map(T::from_value).collect(),
            _ => None,
        }
    }
}

/// 要素ツリーの 1 ノードから読み出せる型（DECISIONS U4）。derive マクロはなく、手で実装する
/// （`examples/contrast.rs`）。
pub trait FromNode: Sized {
    /// 読み出す組み込みコンポーネントの名前。
    const COMPONENT: &'static str;

    /// ノード（コンポーネント名は `COMPONENT`）から値を作る。失敗は、そのノードの位置つきの診断
    /// （[`Node::error`]）で返す。
    fn from_node(node: Node<'_>) -> Result<Self, Diagnostic>;
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

/// 子ノード 1 つ（要素かテキスト）。評価結果の [`ChildNode`] を、[`Node`] で読む形にしたもの。
#[derive(Clone, Copy, Debug)]
pub enum Child<'a> {
    Element(Node<'a>),
    Text(&'a str),
}

/// 値木の要素を読むための参照。ホストが評価結果を読む入口（DECISIONS U4）。
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    element: &'a Element,
}

impl<'a> Node<'a> {
    pub fn new(element: &'a Element) -> Self {
        Node { element }
    }

    /// 組み込みコンポーネントの名前。
    pub fn component(&self) -> &'a str {
        &self.element.name
    }

    /// `key` 属性の値（props ではない。DECISIONS R3・U3）。
    ///
    /// ホストが旧ツリーと差分を取るときの同一性は、key → ホストが宣言した識別 prop
    /// （`Keybind` の `keys` など。props として残っている）→ 親・位置の順。
    pub fn key(&self) -> Option<&'a str> {
        self.element.key.as_deref()
    }

    /// props（値が `undefined` のものは含まない）。
    pub fn props(&self) -> &'a BTreeMap<String, Value> {
        &self.element.props
    }

    /// この要素を作った JSX の位置（ファイル名を含む）。
    pub fn span(&self) -> &'a Span {
        &self.element.span
    }

    /// 展開の途中で通ったコンポーネント呼び出しの位置（内側から順）。
    pub fn expanded_from(&self) -> &'a [Span] {
        &self.element.expanded_from
    }

    /// 子の要素（コンポーネントは展開済み）。テキスト子は含まない（読むには [`Node::child_nodes`]）。
    pub fn children(&self) -> impl Iterator<Item = Node<'a>> + 'a {
        self.element
            .children
            .iter()
            .filter_map(ChildNode::as_element)
            .map(|c| Node::new(c))
    }

    /// 子の要素とテキスト（`false` と `undefined` は取り除いてある。DECISIONS R1・R2）。
    pub fn child_nodes(&self) -> impl Iterator<Item = Child<'a>> + 'a {
        self.element.children.iter().map(|c| match c {
            ChildNode::Element(e) => Child::Element(Node::new(e)),
            ChildNode::Text(t) => Child::Text(t),
        })
    }

    /// コンポーネント名が `component` の子の要素。
    pub fn children_named(&self, component: &'a str) -> impl Iterator<Item = Node<'a>> + 'a {
        self.children().filter(move |c| c.component() == component)
    }

    /// 型 `T` の子の要素を、すべて読む（要素の順）。1 つでも読めなければ、その診断。
    pub fn children_of<T: FromNode>(&self) -> Result<Vec<T>, Diagnostic> {
        self.children_named(T::COMPONENT)
            .map(T::from_node)
            .collect()
    }

    /// 一意な子（スキーマの `unique_child`）を読む。なければ `None`、2 つ以上あれば E4008。
    pub fn unique_child<T: FromNode>(&self) -> Result<Option<T>, Diagnostic> {
        let mut it = self.children_named(T::COMPONENT);
        let Some(first) = it.next() else {
            return Ok(None);
        };
        if let Some(second) = it.next() {
            return Err(second.error(format!(
                "`<{}>` の子に `<{}>` が 2 つ以上ある",
                self.component(),
                T::COMPONENT
            )));
        }
        T::from_node(first).map(Some)
    }

    /// 一意な子で、必須のもの。なければ、この要素の位置で E4008。
    pub fn required_child<T: FromNode>(&self) -> Result<T, Diagnostic> {
        self.unique_child()?.ok_or_else(|| {
            self.error(format!(
                "`<{}>` に `<{}>` が要る",
                self.component(),
                T::COMPONENT
            ))
        })
    }

    /// このノードを `T` として読む。コンポーネント名が違えば E4008。
    pub fn read<T: FromNode>(&self) -> Result<T, Diagnostic> {
        self.expect_component(T::COMPONENT)?;
        T::from_node(*self)
    }

    /// この要素の位置で E4008 を作る。
    pub fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(Code::ConversionFailed, self.element.span.clone(), message)
    }

    /// prop `key` の位置（属性の名前から値まで。`{...x}` から来た prop は、そのスプレッド属性全体。
    /// ファイルは要素と同じ）。prop がない（`undefined` を含む）ときは `None`（DECISIONS P1）。
    pub fn prop_span(&self, key: &str) -> Option<Span> {
        let r = self.element.prop_spans.get(key)?;
        Some(Span {
            file: self.element.span.file.clone(),
            start: r.start,
            end: r.end,
        })
    }

    /// prop `key` の位置で E4008 を作る。prop の位置がなければ、この要素の位置（[`Node::error`]）。
    pub fn prop_error(&self, key: &str, message: impl Into<String>) -> Diagnostic {
        match self.prop_span(key) {
            Some(span) => Diagnostic::error(Code::ConversionFailed, span, message),
            None => self.error(message),
        }
    }

    /// props を読む。ない prop は `undefined` として扱うので、省略できる prop は `Option<T>` で読む。
    /// 型が合わない・必須なのにない場合は E4008。
    pub fn prop<T: FromValue>(&self, key: &str) -> Result<T, Diagnostic> {
        let v = self.element.props.get(key).unwrap_or(&Value::Undefined);
        T::from_value(v).ok_or_else(|| {
            self.prop_error(
                key,
                format!(
                    "`<{}>` の `{key}` は `{}` が要る（{}）",
                    self.component(),
                    T::expected(),
                    kind(v)
                ),
            )
        })
    }

    /// スロット（要素を渡す props。`<Panel header={<Title />}>`）の要素。値がなければ `None`。
    /// 要素でなければ E4008。
    pub fn slot(&self, key: &str) -> Result<Option<Node<'a>>, Diagnostic> {
        match self.element.props.get(key) {
            None | Some(Value::Undefined) => Ok(None),
            Some(Value::Element(e)) => Ok(Some(Node::new(e))),
            Some(v) => Err(self.prop_error(
                key,
                format!(
                    "`<{}>` の `{key}` は要素が要る（{}）",
                    self.component(),
                    kind(v)
                ),
            )),
        }
    }

    /// 要素の配列のスロット（`footer={[<A />, <B />]}`）。値がなければ空。
    pub fn slots(&self, key: &str) -> Result<Vec<Node<'a>>, Diagnostic> {
        match self.element.props.get(key) {
            None | Some(Value::Undefined) => Ok(Vec::new()),
            Some(Value::Element(e)) => Ok(vec![Node::new(e)]),
            Some(Value::Array(xs)) => xs
                .iter()
                .map(|x| match x {
                    Value::Element(e) => Ok(Node::new(e)),
                    v => Err(self.prop_error(
                        key,
                        format!(
                            "`<{}>` の `{key}` の要素は要素が要る（{}）",
                            self.component(),
                            kind(v)
                        ),
                    )),
                })
                .collect(),
            Some(v) => Err(self.prop_error(
                key,
                format!(
                    "`<{}>` の `{key}` は要素の配列が要る（{}）",
                    self.component(),
                    kind(v)
                ),
            )),
        }
    }

    /// スロットを `T` として読む。値がなければ `None`。
    pub fn slot_as<T: FromNode>(&self, key: &str) -> Result<Option<T>, Diagnostic> {
        self.slot(key)?.map(|n| n.read::<T>()).transpose()
    }

    /// 要素が `component` であることを確かめる。違えば E4008。
    pub fn expect_component(&self, component: &str) -> Result<(), Diagnostic> {
        if self.component() == component {
            Ok(())
        } else {
            Err(self.error(format!(
                "`<{component}>` が要るところに `<{}>` がある",
                self.component()
            )))
        }
    }
}
