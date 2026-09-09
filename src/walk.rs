//! Walking a parsed JSON document by selector. A name or key steps into an
//! object member, an ordinal into an array element, and `[n]` into the
//! children of either — array elements or object member values, in document
//! order.

use crate::selector::Segment;
use serde_json::Value;

/// The first value the selector resolves to, or `None`. At `[n]` the first
/// child under which the rest of the selector resolves wins.
pub fn read<'a>(value: &'a Value, segments: &[Segment]) -> Option<&'a Value> {
    let Some((segment, rest)) = segments.split_first() else {
        return Some(value);
    };
    match segment {
        Segment::Name(member) | Segment::Key(member) => read(value.as_object()?.get(member)?, rest),
        Segment::Number(index) => read(value.as_array()?.get(*index)?, rest),
        Segment::Any => children(value).find_map(|child| read(child, rest)),
    }
}

/// Write `replacement` at every place the selector resolves to and count them.
/// A last name or key is added to an object that lacks it; an ordinal past the
/// end, a member of something that is not an object and a child under which
/// the rest does not resolve all write nothing. Zero is the caller's refusal.
pub fn write(value: &mut Value, segments: &[Segment], replacement: &Value) -> usize {
    let Some((segment, rest)) = segments.split_first() else {
        *value = replacement.clone();
        return 1;
    };
    match segment {
        Segment::Name(member) | Segment::Key(member) => match value.as_object_mut() {
            Some(members) if rest.is_empty() => {
                members.insert(member.clone(), replacement.clone());
                1
            }
            Some(members) => members
                .get_mut(member)
                .map_or(0, |child| write(child, rest, replacement)),
            None => 0,
        },
        Segment::Number(index) => value
            .as_array_mut()
            .and_then(|items| items.get_mut(*index))
            .map_or(0, |child| write(child, rest, replacement)),
        Segment::Any => children_mut(value)
            .map(|child| write(child, rest, replacement))
            .sum(),
    }
}

fn children(value: &Value) -> Box<dyn Iterator<Item = &Value> + '_> {
    match value {
        Value::Array(items) => Box::new(items.iter()),
        Value::Object(members) => Box::new(members.values()),
        _ => Box::new(std::iter::empty()),
    }
}

fn children_mut(value: &mut Value) -> Box<dyn Iterator<Item = &mut Value> + '_> {
    match value {
        Value::Array(items) => Box::new(items.iter_mut()),
        Value::Object(members) => Box::new(members.values_mut()),
        _ => Box::new(std::iter::empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::Selector;
    use serde_json::json;

    fn at<'a>(value: &'a Value, expression: &str) -> Option<&'a Value> {
        let selector = Selector::parse(expression).expect("parses");
        read(value, selector.segments())
    }

    fn put(value: &mut Value, expression: &str, replacement: &Value) -> usize {
        let selector = Selector::parse(expression).expect("parses");
        write(value, selector.segments(), replacement)
    }

    #[test]
    fn reads_by_name_ordinal_key_and_first_occurrence() {
        let doc = json!({
            "order": {"customer": {"name": "Ada"}, "x-id": 7},
            "orders": [{"id": "A"}, {"id": "B", "ref": "R"}],
            "lines": [1, {"sku": "S1"}]
        });
        assert_eq!(at(&doc, "order.customer.name"), Some(&json!("Ada")));
        assert_eq!(at(&doc, "order['x-id']"), Some(&json!(7)));
        assert_eq!(at(&doc, "orders[1].id"), Some(&json!("B")));
        assert_eq!(at(&doc, "orders[n].id"), Some(&json!("A")));
        assert_eq!(at(&doc, "orders[n].ref"), Some(&json!("R")));
        assert_eq!(at(&doc, "lines[n].sku"), Some(&json!("S1")));
        assert_eq!(at(&doc, "order[n]"), Some(&json!({"name": "Ada"})));
        assert_eq!(at(&doc, "orders[2].id"), None);
        assert_eq!(at(&doc, "order.customer.age"), None);
        assert_eq!(at(&doc, "order.customer.name.first"), None);
        assert_eq!(at(&doc, "order[0]"), None);
    }

    #[test]
    fn writes_at_a_place_adds_a_member_and_fills_every_occurrence() {
        let mut doc = json!({"order": {"paid": false}, "orders": [{"id": "A"}, {"id": "B"}, 3]});
        assert_eq!(put(&mut doc, "order.paid", &json!(true)), 1);
        assert_eq!(put(&mut doc, "order.ref", &json!("R7")), 1);
        assert_eq!(put(&mut doc, "orders[n].id", &json!("Z")), 2);
        assert_eq!(put(&mut doc, "orders[2]", &json!(4)), 1);
        assert_eq!(
            doc,
            json!({"order": {"paid": true, "ref": "R7"}, "orders": [{"id": "Z"}, {"id": "Z"}, 4]})
        );
        assert_eq!(put(&mut doc, "orders[3]", &json!(0)), 0);
        assert_eq!(put(&mut doc, "order.paid.deep", &json!(0)), 0);
        assert_eq!(put(&mut doc, "nowhere.ref", &json!(0)), 0);
        assert_eq!(put(&mut doc, "orders[n].sku.code", &json!(0)), 0);
    }
}
