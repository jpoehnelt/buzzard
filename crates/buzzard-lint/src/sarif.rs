//! SARIF 2.1.0 output for code-scanning services.

use std::path::Path;

use serde_json::{json, Value};

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::RULES;

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn uri(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn location(path: &Path) -> Value {
    json!({ "physicalLocation": { "artifactLocation": { "uri": uri(path) } } })
}

/// A SARIF log with one run that lists every rule and the given
/// diagnostics. `tool_version` is reported as the version of the tool.
pub fn to_sarif(diagnostics: &[Diagnostic], tool_version: &str) -> Value {
    let rules: Vec<Value> = RULES
        .iter()
        .map(|r| {
            json!({
                "id": r.code,
                "name": r.name,
                "shortDescription": { "text": r.summary },
                "defaultConfiguration": { "level": level(r.severity) },
            })
        })
        .collect();
    let results: Vec<Value> = diagnostics
        .iter()
        .map(|d| {
            let mut result = json!({
                "ruleId": d.code,
                "ruleIndex": RULES.iter().position(|r| r.code == d.code),
                "level": level(d.severity),
                "message": { "text": d.message },
                "locations": [location(&d.path)],
            });
            if !d.related.is_empty() {
                result["relatedLocations"] = d.related.iter().map(|p| location(p)).collect();
            }
            result
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "buzzard",
                "version": tool_version,
                "informationUri": "https://github.com/jpoehnelt/buzzard",
                "rules": rules,
            }},
            "results": results,
        }],
    })
}
