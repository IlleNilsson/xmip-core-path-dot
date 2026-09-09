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
//! Three things, because a path language is nothing without content to address:
//! [`DotEngine`], the [`PathEngine`] for the language `dot`; [`DotStructure`],
//! a [`StructureReader`] over a JSON Stream; and [`DotRewrite`], a
//! [`StructureWriter`] that produces a new Stream with one or more values
//! replaced, as ADR-0013 asks of anything that changes content. Promote reads
//! through the first two; demote writes through the first and third; route and
//! process read.
//!
//! A selector declares how far it reaches — [`Selector::reach`] — and that
//! declaration is the load-bearing part: it is what lets a Receive Location be
//! configured with promotions that are cheap by construction. This crate reads
//! JSON through a parsed document, so its engine reports the honest
//! [`PathCost::Materialized`]; a streaming reader over the same selectors
//! would report the declared reach instead.

mod selector;
mod walk;

pub use selector::{Reach, Segment, Selector};

use contract::{
    ContractDescriptor, ContractError, ContractId, StructureReader, StructureWriter,
    StructuredValue,
};
use path::{Path, PathCost, PathEngine};
use serde_json::Value;
use stream::Stream;
use xcore::StreamId;

/// The `dot` engine. The reader speaks selectors already, so the engine adds
/// no traversal of its own.
pub struct DotEngine;

impl PathEngine for DotEngine {
    fn language(&self) -> &'static str {
        "dot"
    }

    fn read(
        &self,
        reader: &dyn StructureReader,
        path: &Path,
    ) -> Result<Option<StructuredValue>, ContractError> {
        reader.read(&path.expression)
    }

    fn write(
        &self,
        writer: &mut dyn StructureWriter,
        path: &Path,
        value: StructuredValue,
    ) -> Result<(), ContractError> {
        writer.write(&path.expression, value)
    }

    /// The JSON reader parses the document whole, whatever the selector
    /// declares; the declaration is answered by [`Selector::reach`].
    fn cost(&self, _path: &Path) -> PathCost {
        PathCost::Materialized
    }
}

fn descriptor() -> ContractDescriptor {
    ContractDescriptor {
        id: ContractId("json-schema".to_string()),
        version: "1".to_string(),
        representation: "application/json".to_string(),
    }
}

fn parse(stream: &Stream) -> Result<Value, ContractError> {
    serde_json::from_slice(stream.bytes()).map_err(|error| ContractError {
        message: format!("not valid JSON: {error}"),
    })
}

/// A JSON Stream, read by selector.
pub struct DotStructure {
    descriptor: ContractDescriptor,
    value: Value,
}

impl DotStructure {
    /// Parse `stream` once; every read is a selector walk after that.
    ///
    /// # Errors
    /// The Stream is not JSON.
    pub fn parse(stream: &Stream) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            value: parse(stream)?,
        })
    }
}

impl StructureReader for DotStructure {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    fn read(&self, path: &str) -> Result<Option<StructuredValue>, ContractError> {
        let selector = Selector::parse(path)?;
        walk::read(&self.value, selector.segments())
            .map(|found| scalar(found, path))
            .transpose()
    }
}

/// A JSON Stream being rewritten into a new one.
pub struct DotRewrite {
    descriptor: ContractDescriptor,
    id: StreamId,
    value: Value,
}

impl DotRewrite {
    /// Start from `stream`; the Stream `finish` produces carries `id`.
    ///
    /// # Errors
    /// The Stream is not JSON.
    pub fn of(stream: &Stream, id: StreamId) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            id,
            value: parse(stream)?,
        })
    }
}

impl StructureWriter for DotRewrite {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    /// Replace the value at every place `path` resolves to, or add a last
    /// name or key to the object it names. A selector into nothing is
    /// refused: demote names a place, it does not invent structure.
    fn write(&mut self, path: &str, value: StructuredValue) -> Result<(), ContractError> {
        let selector = Selector::parse(path)?;
        let replacement = json(value)?;
        if walk::write(&mut self.value, selector.segments(), &replacement) == 0 {
            return Err(ContractError {
                message: format!("{path:?} names nothing to write into"),
            });
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Stream, ContractError> {
        let bytes = serde_json::to_vec(&self.value).map_err(|error| ContractError {
            message: format!("cannot serialise JSON: {error}"),
        })?;
        Ok(Stream::new(
            self.id,
            bytes,
            Some(self.descriptor.representation),
        ))
    }
}

fn scalar(value: &Value, path: &str) -> Result<StructuredValue, ContractError> {
    Ok(match value {
        Value::Null => StructuredValue::Null,
        Value::Bool(flag) => StructuredValue::Bool(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => StructuredValue::Integer(integer),
            None => StructuredValue::Decimal(number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(text) => StructuredValue::Text(text.clone()),
        Value::Array(_) | Value::Object(_) => {
            return Err(ContractError {
                message: format!("{path} is not a scalar"),
            });
        }
    })
}

fn json(value: StructuredValue) -> Result<Value, ContractError> {
    Ok(match value {
        StructuredValue::Null => Value::Null,
        StructuredValue::Bool(flag) => Value::Bool(flag),
        StructuredValue::Integer(integer) => Value::from(integer),
        StructuredValue::Decimal(decimal) => {
            serde_json::Number::from_f64(decimal).map_or(Value::Null, Value::Number)
        }
        StructuredValue::Text(text) => Value::String(text),
        StructuredValue::Binary(_) => {
            return Err(ContractError {
                message: "binary has no JSON form here".to_string(),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(text: &str) -> Stream {
        Stream::new(StreamId::new(1), text.as_bytes().to_vec(), None)
    }

    const ORDER: &str = concat!(
        r#"{"id":"A1","paid":false,"headers":{"x-ref":"R"},"#,
        r#""lines":[{"sku":"X","qty":2,"price":9.5},{"sku":"Y","qty":1}]}"#
    );

    #[test]
    fn reads_scalars_by_selector_and_refuses_structures() {
        let structure = DotStructure::parse(&stream(ORDER)).expect("parses");
        let engine = DotEngine;
        let read = |p: &str| engine.read(&structure, &Path::new("dot", p));
        assert_eq!(
            read("id").expect("reads"),
            Some(StructuredValue::Text("A1".into()))
        );
        assert_eq!(
            read("paid").expect("reads"),
            Some(StructuredValue::Bool(false))
        );
        assert_eq!(
            read("headers['x-ref']").expect("reads"),
            Some(StructuredValue::Text("R".into()))
        );
        assert_eq!(
            read("lines[1].qty").expect("reads"),
            Some(StructuredValue::Integer(1))
        );
        assert_eq!(
            read("lines[n].price").expect("reads"),
            Some(StructuredValue::Decimal(9.5))
        );
        assert_eq!(read("lines[n].colour").expect("reads"), None);
        assert_eq!(read("nowhere").expect("reads"), None);
        assert!(read("lines").is_err());
        assert!(read("lines[").is_err());
        assert_eq!(engine.cost(&Path::new("dot", "id")), PathCost::Materialized);
        assert_eq!(
            Selector::parse("lines[n].price").expect("parses").reach(),
            Reach::StreamScan
        );
    }

    #[test]
    fn rewrites_into_a_new_stream_with_the_given_id() {
        let mut rewrite = DotRewrite::of(&stream(ORDER), StreamId::new(2)).expect("parses");
        let engine = DotEngine;
        let mut write =
            |p: &str, v: StructuredValue| engine.write(&mut rewrite, &Path::new("dot", p), v);
        write("paid", StructuredValue::Bool(true)).expect("writes");
        write("headers['x-ref']", StructuredValue::Text("R7".into())).expect("writes");
        write("lines[n].qty", StructuredValue::Integer(0)).expect("writes every line");
        write("lines[1].note", StructuredValue::Null).expect("adds");
        assert!(write("lines[5].qty", StructuredValue::Integer(1)).is_err());
        assert!(write("nowhere.deep", StructuredValue::Null).is_err());
        assert!(write("id", StructuredValue::Binary(vec![1])).is_err());
        assert!(write("", StructuredValue::Null).is_err());
        let out = Box::new(rewrite).finish().expect("finishes");
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
    fn a_stream_that_is_not_json_is_refused_up_front() {
        assert!(DotStructure::parse(&stream("{nope")).is_err());
        assert!(DotRewrite::of(&stream("{nope"), StreamId::new(1)).is_err());
    }
}
