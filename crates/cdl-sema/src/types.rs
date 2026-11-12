//! 型（SPEC 第 4 章、DECISIONS B1–B17）。
//!
//! 型は構造的で、別名を区別しない。ユニオンはいつも正規化した形で持つ（[`Ty::union`]）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Boolean,
    Number,
    String,
    Undefined,
    Length,
    Duration,
    Color,
    Literal(Literal),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    /// `Record<string, T>`
    Record(Box<Ty>),
    /// 閉じたオブジェクト型。
    Object(BTreeMap<String, Field>),
    /// 正規化済みのユニオン（要素は 2 つ以上で、入れ子のユニオンを含まない）。
    Union(Vec<Ty>),
    /// `Element<A | B>`。組み込みコンポーネントの名前の集合。
    Element(BTreeSet<String>),
    Function(Vec<Ty>, Box<Ty>),
    /// 誤りのあとの型。どの型とも互換にして、誤りの連鎖を防ぐ。
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    String(String),
    Number(f64),
    Bool(bool),
}

/// オブジェクト型のフィールド。`optional` なら値は `ty | undefined`（DECISIONS B7）。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub ty: Ty,
    pub optional: bool,
}

impl Ty {
    /// 型の木の深さが `max_depth` を超えるか、ノード数が `max_nodes` を超えるか（AUDIT P2）。
    /// 深い木でもスタックを使い切らないよう、明示のスタックで数え、超えた時点で打ち切る。
    pub fn exceeds(&self, max_depth: u32, max_nodes: u32) -> bool {
        let mut stack: Vec<(&Ty, u32)> = vec![(self, 1)];
        let mut nodes: u32 = 0;
        while let Some((t, depth)) = stack.pop() {
            nodes = nodes.saturating_add(1);
            if depth > max_depth || nodes > max_nodes {
                return true;
            }
            let next = depth.saturating_add(1);
            match t {
                Ty::Array(x) | Ty::Record(x) => stack.push((x, next)),
                Ty::Tuple(xs) | Ty::Union(xs) => stack.extend(xs.iter().map(|x| (x, next))),
                Ty::Object(fields) => stack.extend(fields.values().map(|f| (&f.ty, next))),
                Ty::Function(params, ret) => {
                    stack.extend(params.iter().map(|x| (x, next)));
                    stack.push((ret, next));
                }
                _ => {}
            }
        }
        false
    }
}

impl Field {
    pub fn required(ty: Ty) -> Self {
        Field {
            ty,
            optional: false,
        }
    }

    pub fn optional(ty: Ty) -> Self {
        Field { ty, optional: true }
    }

    /// 読み出したときの型。
    pub fn effective(&self) -> Ty {
        if self.optional {
            Ty::union(vec![self.ty.clone(), Ty::Undefined])
        } else {
            self.ty.clone()
        }
    }
}

impl Literal {
    pub fn number(n: f64) -> Literal {
        // -0 は +0 にそろえる（DECISIONS D2）。
        Literal::Number(if n == 0.0 { 0.0 } else { n })
    }

    pub fn base(&self) -> Ty {
        match self {
            Literal::String(_) => Ty::String,
            Literal::Number(_) => Ty::Number,
            Literal::Bool(_) => Ty::Boolean,
        }
    }
}

impl Ty {
    pub fn object<I, S>(fields: I) -> Ty
    where
        I: IntoIterator<Item = (S, Field)>,
        S: Into<String>,
    {
        Ty::Object(fields.into_iter().map(|(k, f)| (k.into(), f)).collect())
    }

    pub fn element(name: &str) -> Ty {
        Ty::Element(BTreeSet::from([name.to_owned()]))
    }

    /// ユニオンの要素（ユニオンでなければ自分だけ）。
    pub fn members(&self) -> &[Ty] {
        match self {
            Ty::Union(ms) => ms,
            other => std::slice::from_ref(other),
        }
    }

    /// 正規化したユニオンを作る。入れ子を平らにし、重複を除き、基本型に含まれるリテラルを除き、
    /// `Element` をまとめ、`true | false` を `boolean` にする。
    pub fn union(members: Vec<Ty>) -> Ty {
        let mut flat = Vec::new();
        for m in members {
            match m {
                Ty::Union(inner) => flat.extend(inner),
                Ty::Error => return Ty::Error,
                other => flat.push(other),
            }
        }
        let mut elements: Option<BTreeSet<String>> = None;
        let mut out: Vec<Ty> = Vec::new();
        for m in flat {
            if let Ty::Element(names) = m {
                elements.get_or_insert_with(BTreeSet::new).extend(names);
            } else if !out.contains(&m) {
                out.push(m);
            }
        }
        let has_true = out.contains(&Ty::Literal(Literal::Bool(true)));
        let has_false = out.contains(&Ty::Literal(Literal::Bool(false)));
        if has_true && has_false {
            out.retain(|m| !matches!(m, Ty::Literal(Literal::Bool(_))));
            if !out.contains(&Ty::Boolean) {
                out.push(Ty::Boolean);
            }
        }
        let bases: Vec<Ty> = out
            .iter()
            .filter(|m| matches!(m, Ty::String | Ty::Number | Ty::Boolean))
            .cloned()
            .collect();
        out.retain(|m| match m {
            Ty::Literal(l) => !bases.contains(&l.base()),
            _ => true,
        });
        if let Some(names) = elements {
            out.push(Ty::Element(names));
        }
        match out.len() {
            0 => Ty::Union(Vec::new()),
            1 => out.pop().unwrap_or(Ty::Error),
            _ => Ty::Union(out),
        }
    }

    pub fn contains_undefined(&self) -> bool {
        self.members().contains(&Ty::Undefined)
    }

    pub fn without_undefined(&self) -> Ty {
        Ty::union(
            self.members()
                .iter()
                .filter(|m| **m != Ty::Undefined)
                .cloned()
                .collect(),
        )
    }

    /// リテラル型を基本型に広げる（オブジェクト・配列の中の値。DECISIONS B3）。
    pub fn widen(&self) -> Ty {
        match self {
            Ty::Literal(l) => l.base(),
            Ty::Union(ms) => {
                // `false | B`（`c && B` の型）の `false` は広げない。広げると `boolean | B` になり、
                // 子に置けなくなる（DECISIONS R2）。
                let false_or_b = ms.contains(&Ty::Literal(Literal::Bool(false)))
                    && ms
                        .iter()
                        .any(|m| !matches!(m, Ty::Literal(_) | Ty::Undefined));
                Ty::union(
                    ms.iter()
                        .map(|m| {
                            if false_or_b && *m == Ty::Literal(Literal::Bool(false)) {
                                m.clone()
                            } else {
                                m.widen()
                            }
                        })
                        .collect(),
                )
            }
            Ty::Array(t) => Ty::Array(Box::new(t.widen())),
            Ty::Tuple(ts) => Ty::Array(Box::new(Ty::union(ts.iter().map(Ty::widen).collect()))),
            Ty::Object(fields) => Ty::Object(
                fields
                    .iter()
                    .map(|(k, f)| {
                        (
                            k.clone(),
                            Field {
                                ty: f.ty.widen(),
                                optional: f.optional,
                            },
                        )
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// ユニオンが CDL で許される形か（DECISIONS B2）。
    pub fn is_valid_union(&self) -> bool {
        let Ty::Union(ms) = self else {
            return true;
        };
        let others: Vec<&Ty> = ms.iter().filter(|m| **m != Ty::Undefined).collect();
        if others.len() <= 1 || others.iter().all(|m| matches!(m, Ty::Literal(_))) {
            return true;
        }
        // `c && B` の型 `false | B`（B は 1 つの型。DECISIONS R2）。
        let is_false = |m: &&&Ty| ***m == Ty::Literal(Literal::Bool(false));
        if others.iter().any(|m| is_false(&m))
            && others.iter().filter(|m| !is_false(m)).count() <= 1
        {
            return true;
        }
        // 子の型 `Element<A> | string`（要素とテキストの混在。DECISIONS U2）。
        others.iter().filter(|m| !is_false(m)).all(|m| {
            matches!(
                m,
                Ty::Element(_) | Ty::String | Ty::Literal(Literal::String(_))
            )
        })
    }

    /// `===` で比べられる型か（DECISIONS B13）。
    pub fn is_comparable_kind(&self) -> bool {
        self.members().iter().all(|m| {
            matches!(
                m,
                Ty::Boolean
                    | Ty::Number
                    | Ty::String
                    | Ty::Undefined
                    | Ty::Length
                    | Ty::Duration
                    | Ty::Color
                    | Ty::Literal(_)
                    | Ty::Error
            )
        })
    }

    /// 2 つの型に共通の値がありうるか（`===` の両辺）。
    pub fn overlaps(&self, other: &Ty) -> bool {
        self.members().iter().any(|a| {
            other
                .members()
                .iter()
                .any(|b| assignable(a, b) || assignable(b, a))
        })
    }
}

/// `from` の値を `to` の場所に置けるか（DECISIONS B1）。
pub fn assignable(from: &Ty, to: &Ty) -> bool {
    match (from, to) {
        (Ty::Error, _) | (_, Ty::Error) => true,
        (a, b) if a == b => true,
        (Ty::Union(ms), to) => ms.iter().all(|m| assignable(m, to)),
        (from, Ty::Union(ms)) => ms.iter().any(|m| assignable(from, m)),
        (Ty::Literal(l), to) => l.base() == *to,
        (Ty::Array(a), Ty::Array(b)) => assignable(a, b),
        (Ty::Tuple(xs), Ty::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| assignable(x, y))
        }
        (Ty::Tuple(xs), Ty::Array(b)) => xs.iter().all(|x| assignable(x, b)),
        (Ty::Record(a), Ty::Record(b)) => assignable(a, b),
        (Ty::Object(f), Ty::Object(g)) => {
            f.keys().all(|k| g.contains_key(k))
                && g.iter().all(|(k, gf)| match f.get(k) {
                    Some(ff) => assignable(&ff.effective(), &gf.effective()),
                    None => gf.optional,
                })
        }
        (Ty::Element(a), Ty::Element(b)) => a.is_subset(b),
        // 関数型は不変（DECISIONS A6）。
        (Ty::Function(..), Ty::Function(..)) => false,
        _ => false,
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::String(s) => write!(f, "{s:?}"),
            Literal::Number(n) => write!(f, "{n}"),
            Literal::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Boolean => f.write_str("boolean"),
            Ty::Number => f.write_str("number"),
            Ty::String => f.write_str("string"),
            Ty::Undefined => f.write_str("undefined"),
            Ty::Length => f.write_str("Length"),
            Ty::Duration => f.write_str("Duration"),
            Ty::Color => f.write_str("Color"),
            Ty::Literal(l) => write!(f, "{l}"),
            Ty::Array(t) => match **t {
                Ty::Union(_) | Ty::Function(..) => write!(f, "({t})[]"),
                _ => write!(f, "{t}[]"),
            },
            Ty::Tuple(ts) => {
                f.write_str("[")?;
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{t}")?;
                }
                f.write_str("]")
            }
            Ty::Record(t) => write!(f, "Record<string, {t}>"),
            Ty::Object(fields) => {
                f.write_str("{ ")?;
                for (k, field) in fields {
                    let q = if field.optional { "?" } else { "" };
                    write!(f, "{k}{q}: {}; ", field.ty)?;
                }
                f.write_str("}")
            }
            Ty::Union(ms) if ms.is_empty() => f.write_str("never"),
            Ty::Union(ms) => {
                for (i, m) in ms.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{m}")?;
                }
                Ok(())
            }
            Ty::Element(names) => {
                f.write_str("Element<")?;
                for (i, n) in names.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    f.write_str(n)?;
                }
                f.write_str(">")
            }
            Ty::Function(ps, r) => {
                f.write_str("(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") => {r}")
            }
            Ty::Error => f.write_str("?"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Ty {
        Ty::Literal(Literal::String(s.to_owned()))
    }

    #[test]
    fn union_is_normalized() {
        assert_eq!(Ty::union(vec![Ty::String, lit("a")]), Ty::String);
        assert_eq!(
            Ty::union(vec![
                Ty::Literal(Literal::Bool(true)),
                Ty::Literal(Literal::Bool(false))
            ]),
            Ty::Boolean
        );
        assert_eq!(
            Ty::union(vec![Ty::element("A"), Ty::element("B")]),
            Ty::Element(["A".to_owned(), "B".to_owned()].into())
        );
        assert_eq!(
            Ty::union(vec![Ty::union(vec![lit("a"), lit("b")]), lit("a")]),
            Ty::Union(vec![lit("a"), lit("b")])
        );
    }

    #[test]
    fn union_forms() {
        assert!(Ty::union(vec![lit("a"), lit("b"), Ty::Undefined]).is_valid_union());
        assert!(Ty::union(vec![Ty::Color, Ty::Undefined]).is_valid_union());
        assert!(!Ty::union(vec![Ty::String, Ty::Number]).is_valid_union());
    }

    #[test]
    fn assignability() {
        assert!(assignable(&lit("a"), &Ty::String));
        assert!(assignable(&lit("a"), &Ty::union(vec![lit("a"), lit("b")])));
        assert!(!assignable(&Ty::String, &lit("a")));
        assert!(assignable(
            &Ty::Array(Box::new(Ty::element("A"))),
            &Ty::Array(Box::new(Ty::union(vec![
                Ty::element("A"),
                Ty::element("B")
            ])))
        ));
        let p = Ty::object([
            ("a", Field::required(Ty::Number)),
            ("b", Field::optional(Ty::String)),
        ]);
        assert!(assignable(
            &Ty::object([("a", Field::required(Ty::Number))]),
            &p
        ));
        assert!(!assignable(
            &Ty::object([
                ("a", Field::required(Ty::Number)),
                ("c", Field::required(Ty::Number))
            ]),
            &p
        ));
        assert!(!assignable(
            &Ty::object([("b", Field::required(Ty::String))]),
            &p
        ));
        assert!(!assignable(
            &Ty::union(vec![Ty::Number, Ty::Undefined]),
            &Ty::Number
        ));
    }
}
