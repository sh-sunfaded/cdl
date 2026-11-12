//! 値木の JSON（DECISIONS D7）。適合テストの期待結果と同じ形。

use cdl_host::{ChildNode, Element, Value};
use serde_json::{Map, Value as Json, json};

fn number(n: f64) -> Json {
    serde_json::Number::from_f64(n).map_or(Json::Null, Json::Number)
}

/// 要素: `{"element": 名前, "key": …, "props": {…}, "children": […]}`。`key` は付いているときだけ。
/// 子は、要素ならこの形のオブジェクト、テキストなら文字列。
pub fn element_json(e: &Element) -> Json {
    let props: Map<String, Json> = e
        .props
        .iter()
        .map(|(k, v)| (k.clone(), value_json(v)))
        .collect();
    let children: Vec<Json> = e
        .children
        .iter()
        .map(|c| match c {
            ChildNode::Element(c) => element_json(c),
            ChildNode::Text(t) => Json::String(t.to_string()),
        })
        .collect();
    let mut out = Map::new();
    out.insert("element".to_owned(), Json::String(e.name.clone()));
    if let Some(key) = &e.key {
        out.insert("key".to_owned(), Json::String(key.clone()));
    }
    out.insert("props".to_owned(), Json::Object(props));
    out.insert("children".to_owned(), Json::Array(children));
    Json::Object(out)
}

/// 値。単位つきの値は `{"length": …}` `{"duration": …}` `{"color": […]}`。
pub fn value_json(v: &Value) -> Json {
    match v {
        Value::Undefined => json!({ "undefined": true }),
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => number(*n),
        Value::String(s) => Json::String(s.to_string()),
        Value::Length(n) => json!({ "length": number(*n) }),
        Value::Duration(ms) => json!({ "duration": ms }),
        Value::Color(c) => json!({ "color": c }),
        Value::Array(xs) => Json::Array(xs.iter().map(value_json).collect()),
        Value::Object(m) => {
            Json::Object(m.iter().map(|(k, v)| (k.clone(), value_json(v))).collect())
        }
        Value::Element(e) => element_json(e),
        Value::Function(_) => json!({ "function": true }),
    }
}

/// JSON の値を CDL の値にする（フックの引数）。`null` は CDL にないので `None`。
pub fn json_value(v: &Json) -> Option<Value> {
    Some(match v {
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(n.as_f64()?),
        Json::String(s) => Value::string(s),
        Json::Array(xs) => Value::array(xs.iter().map(json_value).collect::<Option<_>>()?),
        Json::Object(m) => Value::object(
            m.iter()
                .map(|(k, v)| json_value(v).map(|v| (k.clone(), v)))
                .collect::<Option<Vec<_>>>()?,
        ),
        Json::Null => return None,
    })
}
