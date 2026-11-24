//! The guide to buzzard for an agent, served as an MCP prompt.
//!
//! The text is the skill file of the repository,
//! `.claude/skills/buzzard/SKILL.md`. The copy in this crate is what the
//! binary ships, because a package holds files of its own directory only.
//! A test keeps the two the same, byte for byte.

use serde_json::{json, Value};

/// The guide, with the YAML front matter of a skill file.
pub const GUIDE: &str = include_str!("guide.md");

/// The name of the one prompt.
const NAME: &str = "buzzard";

/// The front matter of the guide and the text after it. A guide without
/// front matter is all text. Line breaks of either kind are accepted,
/// because a checkout on Windows can convert them.
fn parts() -> (&'static str, &'static str) {
    let Some(rest) = GUIDE.strip_prefix("---") else {
        return ("", GUIDE);
    };
    match rest.find("\n---") {
        Some(end) => (&rest[..end], rest[end + 4..].trim_start()),
        None => ("", GUIDE),
    }
}

/// The guide as a reader gets it: the text after the front matter. The
/// prompt `buzzard` and `buzzard guide` both give this text.
#[must_use]
pub fn guide() -> &'static str {
    parts().1
}

/// The `description` of the front matter.
fn description() -> &'static str {
    parts()
        .0
        .lines()
        .find_map(|line| line.strip_prefix("description:"))
        .map_or("How to use buzzard.", str::trim)
}

/// The `prompts` of a `prompts/list` result.
pub(crate) fn list() -> Value {
    json!([{
        "name": NAME,
        "title": "How to use buzzard",
        "description": description(),
    }])
}

/// The result of `prompts/get`, or why the request is not valid.
pub(crate) fn get(params: &Value) -> Result<Value, String> {
    match params.get("name").and_then(Value::as_str) {
        Some(NAME) => Ok(json!({
            "description": description(),
            "messages": [{
                "role": "user",
                "content": { "type": "text", "text": parts().1 },
            }],
        })),
        Some(other) => {
            let shown: String = other.chars().take(64).collect();
            Err(format!(
                "unknown prompt: {}",
                buzzard_lint::printable::text(&shown)
            ))
        }
        None => Err("name must be a string".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_has_front_matter_and_a_body() {
        let (front, body) = parts();
        assert!(front.lines().any(|line| line == "name: buzzard"), "{front}");
        assert!(description().len() > 40, "{}", description());
        assert!(body.starts_with("# "), "{body}");
        assert!(!body.contains("name: buzzard"));
    }

    #[test]
    fn an_unknown_prompt_is_refused() {
        assert!(get(&json!({ "name": "buzzard" })).is_ok());
        assert!(get(&json!({ "name": "other" })).is_err());
        assert!(get(&json!({})).is_err());
        assert!(get(&Value::Null).is_err());
    }
}
