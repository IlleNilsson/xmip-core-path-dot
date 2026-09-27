#![forbid(unsafe_code)]

//! The dot path technology — a technology of `xmip-core-path`.
//!
//! The language is the Xmip content selector of `runtime-model.md` "Content
//! Selectors": `order.customer.name`, `orders[0].id`, `orders[n].id`,
//! `headers['desiredProperty']`. Four segment kinds — a name, an ordinal `[0]`,
//! a key `['x']` and the streaming wildcard `[n]` — and the brackets are Xmip
//! notation, not `JSONPath`'s. `[n]` is the first occurrence when reading and
//! every occurrence when writing.
//!
//! [`DotLanguage`] is the [`PathLanguage`] `dot`: it compiles a selector
//! once, and the compiled [`Selector`] reads one value from a Stream's JSON
//! and writes into a rewrite of it (ADR-0013). Promote reads through it;
//! demote writes through it; route and process read. The JSON document itself
//! — parsed once per Message, bridged to a scalar, written back as a Stream —
//! is the capability's `json`, shared with the other JSON languages
//! (ADR-0044); what is dot's is the selector walk.
//!
//! A selector declares how far it reaches — [`Selector::reach`] — and that
//! declaration is the load-bearing part: it is what lets a Receive Location be
//! configured with promotions that are cheap by construction. This crate
//! reads JSON through a document parsed whole; a streaming reader over the
//! same selectors would answer from the declared reach instead.

mod selector;
mod walk;

pub use selector::{Reach, Segment, Selector};

use contract::ContractError;
use path::{CompiledExpression, Content, PathLanguage, Rewriting, json};
use serde_json::Value;
use xcore::ScalarValue;

/// The language `dot`.
pub struct DotLanguage;

impl PathLanguage for DotLanguage {
    fn language(&self) -> &'static str {
        "dot"
    }

    fn compile(&self, expression: &str) -> Result<Box<dyn CompiledExpression>, ContractError> {
        Ok(Box::new(Compiled {
            selector: Selector::parse(expression)?,
            expression: expression.to_string(),
        }))
    }
}

/// A selector compiled, with the text a refusal names.
struct Compiled {
    selector: Selector,
    expression: String,
}

impl CompiledExpression for Compiled {
    fn read(&self, content: &Content<'_>) -> Result<Option<ScalarValue>, ContractError> {
        let document = content.form::<Value>()?;
        walk::read(&document, self.selector.segments())
            .map(|found| json::scalar(found, &self.expression))
            .transpose()
    }

    /// Replace the value at every place the selector resolves to, or add a
    /// last name or key to the object it names. A selector into nothing is
    /// refused: demote names a place, it does not invent structure.
    fn write(&self, rewriting: &mut Rewriting, value: ScalarValue) -> Result<(), ContractError> {
        let replacement = json::from_scalar(value)?;
        let document = rewriting.form_mut::<Value>()?;
        if walk::write(document, self.selector.segments(), &replacement) == 0 {
            return Err(ContractError::new(format!(
                "{:?} names nothing to write into",
                self.expression
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::fixture::stream;
    use xcore::StreamId;

    const ORDER: &str = concat!(
        r#"{"id":"A1","paid":false,"headers":{"x-ref":"R"},"#,
        r#""lines":[{"sku":"X","qty":2,"price":9.5},{"sku":"Y","qty":1}]}"#
    );

    fn compiled(selector: &str) -> Result<Box<dyn CompiledExpression>, ContractError> {
        DotLanguage.compile(selector)
    }

    #[test]
    fn reads_scalars_by_selector_and_refuses_structures() {
        let order = stream(ORDER);
        let content = Content::of(&order);
        let read = |p: &str| compiled(p)?.read(&content);
        assert_eq!(
            read("id").expect("reads"),
            Some(ScalarValue::Text("A1".into()))
        );
        assert_eq!(read("paid").expect("reads"), Some(ScalarValue::Bool(false)));
        assert_eq!(
            read("headers['x-ref']").expect("reads"),
            Some(ScalarValue::Text("R".into()))
        );
        assert_eq!(
            read("lines[1].qty").expect("reads"),
            Some(ScalarValue::Integer(1))
        );
        assert_eq!(
            read("lines[n].price").expect("reads"),
            Some(ScalarValue::Decimal(9.5))
        );
        assert_eq!(read("lines[n].colour").expect("reads"), None);
        assert_eq!(read("nowhere").expect("reads"), None);
        assert!(read("lines").is_err());
        assert!(read("lines[").is_err());
        assert_eq!(
            Selector::parse("lines[n].price").expect("parses").reach(),
            Reach::StreamScan
        );
    }

    #[test]
    fn rewrites_into_a_new_stream_with_the_given_id() {
        let mut rewriting = Rewriting::of(&stream(ORDER), StreamId::new(2));
        let mut write =
            |p: &str, v: ScalarValue| compiled(p).and_then(|c| c.write(&mut rewriting, v));
        write("paid", ScalarValue::Bool(true)).expect("writes");
        write("headers['x-ref']", ScalarValue::Text("R7".into())).expect("writes");
        write("lines[n].qty", ScalarValue::Integer(0)).expect("writes every line");
        write("lines[1].note", ScalarValue::Null).expect("adds");
        assert!(write("lines[5].qty", ScalarValue::Integer(1)).is_err());
        assert!(write("nowhere.deep", ScalarValue::Null).is_err());
        assert!(write("id", ScalarValue::Binary(vec![1])).is_err());
        assert!(write("", ScalarValue::Null).is_err());
        let out = rewriting.finish().expect("finishes");
        assert_eq!(out.id(), StreamId::new(2));
        assert_eq!(out.media_type(), Some("application/json"));
        let back: Value = serde_json::from_slice(out.bytes()).expect("json");
        assert_eq!(
            back,
            serde_json::json!({
                "id": "A1", "paid": true, "headers": {"x-ref": "R7"},
                "lines": [
                    {"sku": "X", "qty": 0, "price": 9.5},
                    {"sku": "Y", "qty": 0, "note": null}
                ]
            })
        );
    }

    #[test]
    fn a_stream_that_is_not_json_is_refused() {
        let broken = stream("{nope");
        let id = compiled("id").expect("compiles");
        assert!(id.read(&Content::of(&broken)).is_err());
        assert!(
            id.write(
                &mut Rewriting::of(&broken, StreamId::new(1)),
                ScalarValue::Null
            )
            .is_err()
        );
    }
}
