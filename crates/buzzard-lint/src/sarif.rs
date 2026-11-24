//! SARIF 2.1.0 output for code-scanning services.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Component, Path, Prefix};

use serde_json::{json, Value};

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::RULES;
use crate::workspace::Workspace;

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

/// Append one path segment, percent-encoding every byte outside the RFC 3986
/// unreserved set. That covers spaces, `#`, `?`, and `%`, and also `:`, which
/// may not appear unencoded in the first segment of a relative reference.
fn push_segment(out: &mut String, segment: &OsStr) {
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(segment);
    #[cfg(not(unix))]
    let lossy = segment.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = lossy.as_bytes();
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

/// The path as an RFC 3986 URI reference. A relative path stays a relative
/// reference (`sub/my%20file.vi`), and an absolute path becomes a `file` URI
/// (`file:///home/u/a.vi`, `file:///C:/p/a.vi`, `file://server/share/a.vi`).
fn uri(path: &Path) -> String {
    let mut authority = String::new();
    let mut drive = None;
    let mut rooted = false;
    let mut segments: Vec<String> = Vec::new();
    for component in path.components() {
        let mut segment = String::new();
        match component {
            Component::Prefix(prefix) => {
                match prefix.kind() {
                    Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                        drive = Some(char::from(letter).to_ascii_uppercase());
                    }
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                        push_segment(&mut authority, server);
                        push_segment(&mut segment, share);
                        segments.push(segment);
                    }
                    Prefix::Verbatim(name) | Prefix::DeviceNS(name) => {
                        push_segment(&mut segment, name);
                        segments.push(segment);
                    }
                }
                rooted = true;
                continue;
            }
            Component::RootDir => {
                rooted = true;
                continue;
            }
            Component::CurDir => segment.push('.'),
            Component::ParentDir => segment.push_str(".."),
            Component::Normal(name) => push_segment(&mut segment, name),
        }
        segments.push(segment);
    }
    let path = segments.join("/");
    match (rooted, drive) {
        (false, _) => path,
        (true, Some(drive)) => format!("file:///{drive}:/{path}"),
        (true, None) => format!("file://{authority}/{path}"),
    }
}

fn location(path: &Path) -> Value {
    json!({ "physicalLocation": { "artifactLocation": { "uri": uri(path) } } })
}

/// Key of the partial fingerprint that a finding about a member of an
/// archive carries: the rule code and the member's path.
const MEMBER_FINGERPRINT: &str = "buzzardMember/v1";

/// A SARIF log with one run that lists every rule and the given
/// diagnostics. `tool_version` is reported as the version of the tool.
///
/// An artifact location must name a file a consumer can open, and a
/// member of an `.llb` archive is not one. A finding about a member is
/// therefore located at its archive. The message starts with the
/// member's name (see [`Diagnostic::on_disk`]), and a logical location
/// names the member. Such a result also has a partial fingerprint made of
/// the rule code and the member's path, because nothing else tells the
/// findings of two members of one archive apart. `workspace` is the one
/// the diagnostics came from.
pub fn to_sarif(workspace: &Workspace, diagnostics: &[Diagnostic], tool_version: &str) -> Value {
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
            let virtual_path = &d.path;
            let (d, member) = d.on_disk(workspace);
            let mut place = location(&d.path);
            let mut fingerprint = None;
            if let Some(member) = member {
                let qualified = virtual_path.to_string_lossy().replace('\\', "/");
                // The findings of all members of an archive have one
                // artifact and no region. The fingerprint tells them apart.
                fingerprint = Some(format!("{}:{qualified}", d.code));
                place["logicalLocations"] = json!([{
                    "name": member,
                    "fullyQualifiedName": qualified,
                    "kind": "member",
                }]);
            }
            let mut result = json!({
                "ruleId": d.code,
                "ruleIndex": RULES.iter().position(|r| r.code == d.code),
                "level": level(d.severity),
                "message": { "text": d.message },
                "locations": [place],
            });
            if let Some(name) = fingerprint {
                result["partialFingerprints"] = json!({ MEMBER_FINGERPRINT: name });
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_encoded_relative_references() {
        assert_eq!(uri(Path::new("sub/a.vi")), "sub/a.vi");
        assert_eq!(
            uri(Path::new("my lib/Path To String.vi")),
            "my%20lib/Path%20To%20String.vi"
        );
        assert_eq!(uri(Path::new("../x.vi")), "../x.vi");
        assert_eq!(uri(Path::new("./x.vi")), "./x.vi");
        // Characters a consumer would read as query, fragment, or escape.
        assert_eq!(
            uri(Path::new("a#1/50%20done?.vi")),
            "a%231/50%2520done%3F.vi"
        );
        // A colon in the first segment would read as a URI scheme.
        #[cfg(unix)]
        assert_eq!(uri(Path::new("a:b/x.vi")), "a%3Ab/x.vi");
        assert_eq!(
            uri(Path::new("テスト.vi")),
            "%E3%83%86%E3%82%B9%E3%83%88.vi"
        );
        assert_eq!(uri(Path::new("a-b_c~d.e")), "a-b_c~d.e");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_unix_paths_are_file_uris() {
        assert_eq!(
            uri(Path::new("/home/runner/my proj/a.vi")),
            "file:///home/runner/my%20proj/a.vi"
        );
        assert_eq!(uri(Path::new("/")), "file:///");
        // A backslash is an ordinary character in a Unix file name.
        assert_eq!(uri(Path::new("a\\b.vi")), "a%5Cb.vi");
        // Bytes that are not UTF-8 are kept, not replaced.
        let raw = <OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"\x83e.vi");
        assert_eq!(uri(Path::new(raw)), "%83e.vi");
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_use_forward_slashes_and_file_uris() {
        assert_eq!(uri(Path::new(r"sub\my file.vi")), "sub/my%20file.vi");
        assert_eq!(
            uri(Path::new(r"C:\Users\John Doe\a.vi")),
            "file:///C:/Users/John%20Doe/a.vi"
        );
        assert_eq!(uri(Path::new(r"c:\p\a.vi")), "file:///C:/p/a.vi");
        assert_eq!(
            uri(Path::new(r"\\server\share\dir\a.vi")),
            "file://server/share/dir/a.vi"
        );
        assert_eq!(uri(Path::new(r"\\?\C:\p\a.vi")), "file:///C:/p/a.vi");
    }

    #[test]
    fn locations_carry_encoded_uris() {
        let diagnostic = Diagnostic {
            path: "my lib/a b.vi".into(),
            code: "BZ001",
            severity: Severity::Warning,
            message: "m".into(),
            related: vec!["other dir/c#.vi".into()],
        };
        let sarif = to_sarif(&Workspace::default(), &[diagnostic], "0");
        let result = &sarif["runs"][0]["results"][0];
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "my%20lib/a%20b.vi"
        );
        assert_eq!(
            result["relatedLocations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "other%20dir/c%23.vi"
        );
    }
}
