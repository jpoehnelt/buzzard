//! The `outputSchema` of each tool: the shape of its `structuredContent`.
//!
//! The schemas are JSON Schema, kept as JSON in `output_schemas.json`,
//! with one entry per tool. [`output`] adds the two entries that every
//! tool shares: `budget`, which a reduced answer carries, and `error`,
//! which a failed call carries alone.
//!
//! The schemas were written from real results and from the types that
//! serialize them. They name every key that a result holds, to a depth of
//! about three levels, and leave deeper values open. No schema lists a
//! `required` key, because a reduced answer leaves keys out. A test checks
//! real results of every tool against its schema.

use std::sync::OnceLock;

use buzzard_lint::ErrorKind;
use serde_json::{json, Value};

const SCHEMAS: &str = include_str!("output_schemas.json");

fn schemas() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| {
        // A unit test parses the file, so this default is never served.
        serde_json::from_str(SCHEMAS).unwrap_or_default()
    })
}

/// The schema of the `structuredContent` of the tool `name`.
pub(crate) fn output(name: &str) -> Value {
    let mut schema = schemas()
        .get(name)
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let kinds: Vec<&str> = ErrorKind::ALL.iter().map(|kind| kind.as_str()).collect();
    schema["properties"]["budget"] = json!({
        "type": "object",
        "description": "Present only when the answer was reduced to fit `max_bytes`. `full_bytes` is the size of the whole answer, and `more` says how to get the rest.",
        "properties": {
            "max_bytes": { "type": "integer" },
            "full_bytes": { "type": "integer" },
            "more": { "type": "string" },
        },
    });
    schema["properties"]["error"] = json!({
        "type": "object",
        "description": "Present only when the call failed (`isError` is true), and then alone. Decide by `kind`, not by the words of `message`.",
        "properties": {
            "kind": { "type": "string", "enum": kinds },
            "message": { "type": "string" },
            "path": { "type": "string" },
        },
    });
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::NAMES;

    #[test]
    fn the_file_holds_one_schema_for_each_tool() {
        let parsed: Value = serde_json::from_str(SCHEMAS).expect("the file is JSON");
        let names: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected = NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
        for name in NAMES {
            let schema = output(name);
            assert_eq!(schema["type"], "object", "{name}");
            assert!(schema.get("required").is_none(), "{name}");
            for shared in ["budget", "error"] {
                assert!(schema["properties"][shared].is_object(), "{name}");
            }
        }
    }

    /// Every `$ref` of a schema names an entry of its own `$defs`.
    #[test]
    fn every_reference_resolves() {
        fn references<'a>(schema: &'a Value, found: &mut Vec<&'a str>) {
            match schema {
                Value::Object(map) => {
                    if let Some(Value::String(target)) = map.get("$ref") {
                        found.push(target);
                    }
                    map.values().for_each(|value| references(value, found));
                }
                Value::Array(items) => items.iter().for_each(|item| references(item, found)),
                _ => {}
            }
        }
        for name in NAMES {
            let schema = output(name);
            let mut found = Vec::new();
            references(&schema, &mut found);
            for target in found {
                let key = target.strip_prefix("#/$defs/").expect(target);
                assert!(schema["$defs"][key].is_object(), "{name}: {target}");
            }
        }
    }
}
