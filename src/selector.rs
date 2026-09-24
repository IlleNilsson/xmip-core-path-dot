//! The Xmip content selector, parsed: `order.customer.name`, `orders[0].id`,
//! `orders[n].id`, `headers['desiredProperty']`. The expression is what an
//! operator writes; the [`Selector`] is what a Content Module walks with, and
//! its [`Reach`] is the declaration `runtime-model.md` calls load-bearing.

use contract::ContractError;
use std::fmt;

/// One step of a selector. Four kinds, and the brackets are Xmip notation,
/// not `JSONPath`'s.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Segment {
    /// A named step: `order`, `customer`.
    Name(String),
    /// An ordinal occurrence: `[0]`, `[3]`.
    Number(usize),
    /// A named key, quoted so it may hold what a name may not: `['x-id']`.
    Key(String),
    /// The streaming wildcard `[n]`: the first occurrence when reading, every
    /// occurrence when writing.
    Any,
}

/// How far into the Stream a selector must reach before it can be answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reach {
    /// Names and keys only: resolvable near the beginning of the Stream.
    StreamPrefix,
    /// A wildcard is present: resolvable by scanning forward, still without
    /// materializing.
    StreamScan,
    /// An ordinal is present and no wildcard: an ordinal needs the shape of a
    /// section to count within.
    MaterializedSection,
}

impl fmt::Display for Reach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StreamPrefix => "stream-prefix",
            Self::StreamScan => "stream-scan",
            Self::MaterializedSection => "materialized-section",
        })
    }
}

/// A parsed selector: at least one segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    segments: Vec<Segment>,
}

impl Selector {
    /// Parse `expression`. Names are joined by `.`; brackets follow a name or
    /// another bracket directly; a name after a bracket needs its `.`.
    ///
    /// # Errors
    /// The expression is empty, has an empty name, an unclosed or unknown
    /// bracket, or a stray dot.
    pub fn parse(expression: &str) -> Result<Self, ContractError> {
        let mut segments = Vec::new();
        let mut rest = expression;
        while !rest.is_empty() {
            let (segment, after) = if let Some(inner) = rest.strip_prefix('[') {
                let (body, after) = inner
                    .split_once(']')
                    .ok_or_else(|| refuse(expression, "a bracket is never closed"))?;
                (
                    bracket(body).ok_or_else(|| {
                        refuse(expression, &format!("[{body}] is not a bracket kind"))
                    })?,
                    after,
                )
            } else {
                let name = match rest.strip_prefix('.') {
                    Some(after_dot) if !segments.is_empty() => after_dot,
                    Some(_) => return Err(refuse(expression, "starts with a dot")),
                    None if segments.is_empty() => rest,
                    None => return Err(refuse(expression, "a name follows a bracket")),
                };
                let end = name.find(['.', '[']).unwrap_or(name.len());
                if end == 0 {
                    return Err(refuse(expression, "a name is empty"));
                }
                let (name, after) = name.split_at(end);
                if name.contains([']', '\'']) {
                    return Err(refuse(expression, &format!("{name} is not a name")));
                }
                (Segment::Name(name.to_string()), after)
            };
            segments.push(segment);
            rest = after;
        }
        if segments.is_empty() {
            return Err(refuse(expression, "is empty"));
        }
        Ok(Self { segments })
    }

    /// The steps, in order.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The declared evaluation: what a Receive Location can know about the
    /// cost of a promotion before performing it.
    #[must_use]
    pub fn reach(&self) -> Reach {
        if self.segments.contains(&Segment::Any) {
            Reach::StreamScan
        } else if self
            .segments
            .iter()
            .any(|segment| matches!(segment, Segment::Number(_)))
        {
            Reach::MaterializedSection
        } else {
            Reach::StreamPrefix
        }
    }
}

/// The segment a bracket body denotes: `n`, `'key'` or digits.
fn bracket(body: &str) -> Option<Segment> {
    if body == "n" {
        return Some(Segment::Any);
    }
    if let Some(key) = body
        .strip_prefix('\'')
        .and_then(|quoted| quoted.strip_suffix('\''))
    {
        return (!key.contains('\'')).then(|| Segment::Key(key.to_string()));
    }
    body.parse().ok().map(Segment::Number)
}

fn refuse(expression: &str, why: &str) -> ContractError {
    ContractError {
        message: format!("selector {expression:?} {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Segment::{Any, Key, Name, Number};

    fn segments(expression: &str) -> Vec<Segment> {
        Selector::parse(expression)
            .expect("parses")
            .segments()
            .to_vec()
    }

    #[test]
    fn parses_the_four_segment_kinds() {
        assert_eq!(
            segments("order.customer.name"),
            vec![
                Name("order".into()),
                Name("customer".into()),
                Name("name".into())
            ]
        );
        assert_eq!(
            segments("orders[0].id"),
            vec![Name("orders".into()), Number(0), Name("id".into())]
        );
        assert_eq!(
            segments("orders[n].id"),
            vec![Name("orders".into()), Any, Name("id".into())]
        );
        assert_eq!(
            segments("headers['desired.Property']"),
            vec![Name("headers".into()), Key("desired.Property".into())]
        );
        assert_eq!(
            segments("envelope.body.items[3]['sku']"),
            vec![
                Name("envelope".into()),
                Name("body".into()),
                Name("items".into()),
                Number(3),
                Key("sku".into())
            ]
        );
        assert_eq!(segments("[0][n]"), vec![Number(0), Any]);
    }

    #[test]
    fn declares_how_far_it_reaches() {
        let reach = |e: &str| Selector::parse(e).expect("parses").reach();
        assert_eq!(reach("order.customer['name']"), Reach::StreamPrefix);
        assert_eq!(reach("orders[n].id"), Reach::StreamScan);
        assert_eq!(reach("orders[0][n].id"), Reach::StreamScan);
        assert_eq!(reach("orders[0].id"), Reach::MaterializedSection);
        assert_eq!(Reach::StreamPrefix.to_string(), "stream-prefix");
        assert_eq!(Reach::StreamScan.to_string(), "stream-scan");
        assert_eq!(
            Reach::MaterializedSection.to_string(),
            "materialized-section"
        );
    }

    #[test]
    fn refuses_what_is_not_a_selector() {
        for bad in [
            "",
            ".order",
            "order.",
            "order..id",
            "orders[0]id",
            "orders[",
            "orders[x]",
            "orders[-1]",
            "orders['a'b']",
            "orders[']",
            "or]der",
        ] {
            assert!(Selector::parse(bad).is_err(), "{bad:?} should be refused");
        }
    }
}
