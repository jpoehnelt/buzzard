//! Drives the server through in-memory readers and writers, on real files.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use buzzard_mcp::{Server, MAX_LINE_BYTES};
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;
use serde_json::{json, Value};

/// `crates/buzzard-rsrc/tests/fixtures`: real files saved by the IDE.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-rsrc/tests/fixtures")
}

/// Real `.lvproj`, `.lvlib` and `.lvclass` files.
fn projects() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-lint/tests/fixtures/project")
}

struct Session {
    server: Server,
    next_id: u64,
}

impl Session {
    fn new(base: PathBuf) -> Self {
        Self {
            server: Server::new(base),
            next_id: 0,
        }
    }

    fn fixtures() -> Self {
        Self::new(fixtures())
    }

    /// Send one raw line and parse the reply, if any.
    fn raw(&mut self, line: &str) -> Option<Value> {
        let reply = self.server.handle_line(line)?;
        assert!(!reply.contains('\n'), "a reply must be a single line");
        Some(serde_json::from_str(&reply).expect("replies are JSON"))
    }

    /// Send a request and return the whole response after checking its envelope.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        let reply = self
            .raw(&message.to_string())
            .expect("a request is answered");
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], self.next_id);
        assert!(
            reply.get("result").is_some() != reply.get("error").is_some(),
            "exactly one of result and error: {reply}"
        );
        reply
    }

    /// Call a tool and return its result object.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let mut params = json!({ "name": name });
        params["arguments"] = arguments;
        let reply = self.request("tools/call", params);
        let result = reply
            .get("result")
            .unwrap_or_else(|| panic!("protocol error from {name}: {reply}"))
            .clone();
        let content = result["content"].as_array().expect("content is a list");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].is_string());
        assert!(result["isError"].is_boolean());
        if let Some(structured) = result.get("structuredContent") {
            assert!(
                structured.is_object(),
                "structuredContent must be an object"
            );
        }
        result
    }

    /// Call a tool that must succeed; returns `(text, structuredContent)`.
    fn ok(&mut self, name: &str, arguments: Value) -> (String, Value) {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], false, "{result}");
        (text(&result), result["structuredContent"].clone())
    }

    /// Call a tool that must fail as a tool error; returns the message.
    fn fails(&mut self, name: &str, arguments: Value) -> String {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], true, "{result}");
        assert!(result.get("structuredContent").is_none());
        text(&result)
    }

    fn error_code(&mut self, method: &str, params: Value) -> i64 {
        let reply = self.request(method, params);
        reply["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected an error: {reply}"))
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .expect("the content is text")
        .to_owned()
}

/// Every file under `dir` with its bytes, to prove nothing was modified.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("list the directory") {
            let path = entry.expect("read a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                out.insert(path.clone(), fs::read(&path).expect("read the file"));
            }
        }
    }
    out
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-mcp-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the temp dir");
        Self(dir.canonicalize().expect("canonicalize the temp dir"))
    }

    /// Write a synthetic VI that links to `calls` as siblings.
    fn vi(&self, name: &str, calls: &[&str]) {
        let links: Vec<(&str, LvPath)> = calls.iter().map(|n| (*n, LvPath::sibling(n))).collect();
        let bytes = RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, 0x400, 0, 0, 0, 0])
            .links(&links)
            .build();
        fs::write(self.0.join(name), bytes).expect("write the VI");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------- lifecycle

#[test]
fn initialize_reports_tools_capability_and_server_info() {
    let mut session = Session::fixtures();
    let reply = session.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" },
        }),
    );
    let result = &reply["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["capabilities"], json!({ "tools": {} }));
    assert_eq!(result["serverInfo"]["name"], "buzzard");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(result["instructions"]
        .as_str()
        .unwrap()
        .contains("read-only"));

    // An older supported revision is echoed; an unknown one gets the newest.
    let older = session.request("initialize", json!({ "protocolVersion": "2024-11-05" }));
    assert_eq!(older["result"]["protocolVersion"], "2024-11-05");
    let unknown = session.request("initialize", json!({ "protocolVersion": "2099-01-01" }));
    assert_eq!(unknown["result"]["protocolVersion"], "2025-06-18");
    let missing = session.request("initialize", Value::Null);
    assert_eq!(missing["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn ping_answers_with_an_empty_result() {
    let mut session = Session::fixtures();
    assert_eq!(session.request("ping", Value::Null)["result"], json!({}));
    // String ids are echoed unchanged.
    let reply = session
        .raw(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#)
        .unwrap();
    assert_eq!(reply["id"], "abc");
    assert_eq!(reply["result"], json!({}));
}

#[test]
fn notifications_and_client_responses_are_never_answered() {
    let mut session = Session::fixtures();
    for line in [
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
        r#"{"jsonrpc":"2.0","method":"no/such/notification"}"#,
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"buzzard_rules"}}"#,
        r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":7,"error":{"code":1,"message":"x"}}"#,
        "",
        "   \t ",
    ] {
        assert_eq!(session.raw(line), None, "{line}");
    }
}

#[test]
fn tools_list_describes_every_tool_with_a_schema() {
    let mut session = Session::fixtures();
    let reply = session.request("tools/list", Value::Null);
    let tools = reply["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "buzzard_check",
            "buzzard_verify",
            "buzzard_inspect",
            "buzzard_diagram",
            "buzzard_callers",
            "buzzard_callees",
            "buzzard_symbols",
            "buzzard_rules",
        ]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["inputSchema"]["properties"].is_object(), "{tool}");
        assert!(tool["inputSchema"]["required"].is_array(), "{tool}");
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("ead-only") || description.contains("reads no files"));
    }
    // A pagination cursor is accepted and ignored: the list is one page.
    let again = session.request("tools/list", json!({ "cursor": "x" }));
    assert_eq!(again["result"], reply["result"]);
    assert!(reply["result"].get("nextCursor").is_none());
}

// -------------------------------------------------------------------- tools

#[test]
fn check_reports_sorted_diagnostics_and_a_summary() {
    let mut session = Session::fixtures();
    let (text, structured) = session.ok("buzzard_check", json!({ "paths": ["pylavi"] }));
    let diagnostics = structured["diagnostics"].as_array().unwrap();
    let summary = &structured["summary"];
    assert_eq!(summary["files"], 27);
    assert_eq!(summary["problems"], diagnostics.len());
    assert_eq!(summary["truncated"], false);
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["warnings"], diagnostics.len());
    let by_code: u64 = summary["by_code"]
        .as_object()
        .unwrap()
        .values()
        .map(|n| n.as_u64().unwrap())
        .sum();
    assert_eq!(by_code, diagnostics.len() as u64);

    let keys: Vec<(&str, &str)> = diagnostics
        .iter()
        .map(|d| (d["path"].as_str().unwrap(), d["code"].as_str().unwrap()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
    for d in diagnostics {
        assert!(d["path"].as_str().unwrap().starts_with("pylavi/"), "{d}");
        assert_eq!(d["severity"], "warning");
        assert!(
            !d["message"].as_str().unwrap().is_empty(),
            "a diagnostic says what is wrong: {d}"
        );
    }
    let has = |path: &str, code: &str| keys.contains(&(path, code));
    assert!(has("pylavi/absolute_link_PTH.vi", "BZ003"));
    assert!(has("pylavi/add_breakpoints.vi", "BZ004"));
    assert!(has("pylavi/empty_password.vi", "BZ004"));
    assert!(!has("pylavi/empty_separate_code.vi", "BZ001"));
    assert!(text.contains("pylavi/absolute_link_PTH.vi: warning[BZ003]: "));
    assert!(text.ends_with(&format!(
        "Found {n} problem(s) (0 error(s), {n} warning(s), 0 info) in 27 file(s).\n",
        n = diagnostics.len()
    )));
}

#[test]
fn check_honours_ignore_and_limit() {
    let mut session = Session::fixtures();
    let (_, all) = session.ok("buzzard_check", json!({ "paths": ["pylavi"] }));
    let total = all["summary"]["problems"].as_u64().unwrap();
    let bz001 = all["summary"]["by_code"]["BZ001"].as_u64().unwrap();
    assert!(bz001 > 0 && bz001 < total);

    let (_, ignored) = session.ok(
        "buzzard_check",
        json!({ "paths": ["pylavi"], "ignore": ["BZ001"] }),
    );
    assert_eq!(ignored["summary"]["problems"], total - bz001);
    assert!(ignored["summary"]["by_code"].get("BZ001").is_none());

    let (text, limited) = session.ok("buzzard_check", json!({ "paths": ["pylavi"], "limit": 3 }));
    assert_eq!(limited["diagnostics"].as_array().unwrap().len(), 3);
    assert_eq!(
        limited["diagnostics"],
        json!(all["diagnostics"].as_array().unwrap()[..3])
    );
    assert_eq!(limited["summary"]["problems"], total);
    assert_eq!(limited["summary"]["shown"], 3);
    assert_eq!(limited["summary"]["truncated"], true);
    assert!(text.contains(&format!("... {} more not shown", total - 3)));

    // A single clean file, and several paths analyzed as one workspace.
    let (text, clean) = session.ok(
        "buzzard_check",
        json!({ "paths": ["pylavi/empty_separate_code.vi"] }),
    );
    assert_eq!(clean["diagnostics"], json!([]));
    assert_eq!(text, "No problems found in 1 file(s).\n");
    let (_, pair) = session.ok(
        "buzzard_check",
        json!({ "paths": ["pylavi/empty.vi", "pylavi/add.vi", "pylavi/empty.vi"] }),
    );
    assert_eq!(pair["summary"]["files"], 2);
}

#[test]
fn check_reports_unreadable_files_as_diagnostics_not_failures() {
    let mut session = Session::fixtures();
    // A root that is a file is analyzed whatever it is; this one is Markdown.
    let (_, structured) = session.ok("buzzard_check", json!({ "paths": ["README.md"] }));
    assert_eq!(structured["summary"]["errors"], 1);
    assert_eq!(structured["diagnostics"][0]["code"], "BZ000");
    assert_eq!(structured["diagnostics"][0]["severity"], "error");
    assert_eq!(structured["diagnostics"][0]["path"], "README.md");
}

#[test]
fn inspect_describes_a_vi_with_its_dependencies() {
    let mut session = Session::fixtures();
    let (text, structured) = session.ok(
        "buzzard_inspect",
        json!({ "path": "pylavi/relative_link.vi" }),
    );
    assert!(text.starts_with("### relative_link.vi\n"));
    assert!(text.contains("| Saved version | 21.0.0 |"));
    assert!(text.contains("**Dependencies (1)**\n\n- `../empty.vi`\n"));
    assert_eq!(structured["path"], "pylavi/relative_link.vi");
    assert_eq!(structured["has_block_diagram"], true);
    assert_eq!(structured["metadata"]["kind"], "vi");
    assert_eq!(structured["metadata"]["saved_name"], "relative_link.vi");
    let links = structured["metadata"]["links"].as_array().unwrap();
    assert!(links
        .iter()
        .all(|link| link["qualified_name"] == json!(["empty.vi"])));

    let (text, structured) = session.ok(
        "buzzard_inspect",
        json!({ "path": "pylavi/empty_password.vi" }),
    );
    assert!(text.contains("| Password-protected | yes |"));
    assert_eq!(structured["metadata"]["password_protected"], true);
}

#[test]
fn inspect_describes_project_files() {
    let mut session = Session::new(projects());
    let (text, structured) = session.ok(
        "buzzard_inspect",
        json!({ "path": "grpc-labview/helloworld.lvproj" }),
    );
    assert!(text.starts_with("### helloworld.lvproj\n"), "{text}");
    assert_eq!(structured["path"], "grpc-labview/helloworld.lvproj");
    assert!(structured["project"]["members"].is_array());
    let (_, class) = session.ok(
        "buzzard_inspect",
        json!({ "path": "jki-smo/SMO.SelfTerminating.lvclass" }),
    );
    assert!(class["project"].is_object());
}

#[test]
fn diagram_exports_every_format() {
    let mut session = Session::fixtures();
    let mut export = |format: Option<&str>| {
        let mut arguments = json!({ "path": "pylavi/add.vi" });
        if let Some(format) = format {
            arguments["format"] = json!(format);
        }
        session.ok("buzzard_diagram", arguments)
    };
    let (markdown, structured) = export(Some("markdown"));
    assert!(markdown.starts_with("## Block diagram\n"), "{markdown}");
    assert!(structured.is_null());
    assert_eq!(export(None).0, markdown, "markdown is the default");

    let (json_text, structured) = export(Some("json"));
    let parsed: Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(parsed, structured);
    assert_eq!(parsed["root"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["root"]["wires"].as_array().unwrap().len(), 3);

    let (mermaid, _) = export(Some("mermaid"));
    assert!(mermaid.starts_with("flowchart LR\n"), "{mermaid}");
    assert!(mermaid.contains("-->"), "{mermaid}");
    let (dot, _) = export(Some("dot"));
    assert!(dot.starts_with("digraph"), "{dot}");
    assert!(dot.trim_end().ends_with('}'));
    let (svg, _) = export(Some("svg"));
    assert!(svg.starts_with("<svg"), "{}", &svg[..svg.len().min(80)]);
    assert!(svg.trim_end().ends_with("</svg>"));
}

#[test]
fn callers_and_callees_follow_the_dependency_graph() {
    let mut session = Session::fixtures();
    let (text, structured) = session.ok(
        "buzzard_callers",
        json!({ "path": "pylavi/empty.vi", "root": "pylavi" }),
    );
    assert_eq!(
        text,
        "1 file(s) under pylavi use pylavi/empty.vi:\n- pylavi/relative_link.vi\n"
    );
    assert_eq!(
        structured,
        json!({
            "path": "pylavi/empty.vi",
            "root": "pylavi",
            "scanned_files": 27,
            "callers": ["pylavi/relative_link.vi"],
        })
    );
    let (text, structured) = session.ok(
        "buzzard_callees",
        json!({ "path": "pylavi/relative_link.vi", "root": "pylavi" }),
    );
    assert_eq!(
        text,
        "1 file(s) under pylavi are used by pylavi/relative_link.vi:\n- pylavi/empty.vi\n"
    );
    assert_eq!(structured["callees"], json!(["pylavi/empty.vi"]));

    let (text, structured) = session.ok(
        "buzzard_callers",
        json!({ "path": "pylavi/add.vi", "root": "pylavi" }),
    );
    assert_eq!(text, "0 file(s) under pylavi use pylavi/add.vi.\n");
    assert_eq!(structured["callers"], json!([]));
}

#[test]
fn symbols_ranks_fuzzy_matches_and_reports_kinds() {
    let mut session = Session::fixtures();
    let (text, structured) = session.ok(
        "buzzard_symbols",
        json!({ "root": "pylavi", "query": "reent", "limit": 4 }),
    );
    assert_eq!(structured["matches"], 6);
    assert_eq!(
        structured["symbols"],
        json!([
            { "name": "Non-Reentrant.vi", "kind": "vi", "path": "pylavi/reentrancy/23.0/Non-Reentrant.vi" },
            { "name": "Non-Reentrant.vi", "kind": "vi", "path": "pylavi/reentrancy/26.0/Non-Reentrant.vi" },
            { "name": "Shared-Reentrant.vi", "kind": "vi", "path": "pylavi/reentrancy/23.0/Shared-Reentrant.vi" },
            { "name": "Shared-Reentrant.vi", "kind": "vi", "path": "pylavi/reentrancy/26.0/Shared-Reentrant.vi" },
        ])
    );
    assert!(text.starts_with("6 of 27 file(s) under pylavi match `reent`; showing the best 4:\n"));

    let (text, none) = session.ok(
        "buzzard_symbols",
        json!({ "root": "pylavi", "query": "zzzz" }),
    );
    assert_eq!(none["symbols"], json!([]));
    assert_eq!(text, "0 of 27 file(s) under pylavi match `zzzz`.\n");
    let (_, all) = session.ok("buzzard_symbols", json!({ "root": "pylavi", "query": "" }));
    assert_eq!(all["symbols"].as_array().unwrap().len(), 27);

    let mut projects = Session::new(projects());
    let (_, found) = projects.ok("buzzard_symbols", json!({ "root": ".", "query": "" }));
    let kinds: Vec<(&str, &str)> = found["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["kind"].as_str().unwrap()))
        .collect();
    assert!(
        kinds.contains(&("helloworld.lvproj", "project")),
        "{kinds:?}"
    );
    assert!(kinds.contains(&("URI.lvlib", "library")));
    assert!(kinds.contains(&("SMO.SelfTerminating.lvclass", "class")));
}

#[test]
fn rules_lists_the_rule_table() {
    let mut session = Session::fixtures();
    let (text, structured) = session.ok("buzzard_rules", json!({}));
    let rules = structured["rules"].as_array().unwrap();
    assert_eq!(rules.len(), buzzard_lint::RULES.len());
    for (rule, expected) in rules.iter().zip(buzzard_lint::RULES) {
        assert_eq!(rule["code"], expected.code);
        assert_eq!(rule["name"], expected.name);
        assert_eq!(rule["severity"], expected.severity.as_str());
        assert_eq!(rule["summary"], expected.summary);
        assert!(text.contains(&format!("| `{}` | {} |", expected.code, expected.name)));
    }
    // `arguments` may be left out altogether.
    let reply = session.request("tools/call", json!({ "name": "buzzard_rules" }));
    assert_eq!(reply["result"]["structuredContent"], structured);
}

// -------------------------------------------------------------- error paths

#[test]
fn tool_failures_are_results_with_is_error_not_protocol_errors() {
    let mut session = Session::fixtures();
    let missing = session.fails("buzzard_inspect", json!({ "path": "pylavi/nope.vi" }));
    assert_eq!(missing, "file does not exist: pylavi/nope.vi");
    let directory = session.fails("buzzard_inspect", json!({ "path": "pylavi" }));
    assert!(directory.contains("is a directory"), "{directory}");
    // Not an RSRC container at all.
    let unreadable = session.fails("buzzard_inspect", json!({ "path": "README.md" }));
    assert!(unreadable.starts_with("README.md: "), "{unreadable}");
    let unreadable = session.fails("buzzard_diagram", json!({ "path": "README.md" }));
    assert!(unreadable.starts_with("README.md: "), "{unreadable}");
    // A file the IDE itself rejects.
    session.call(
        "buzzard_inspect",
        json!({ "path": "pylavi/empty_invalid.vi" }),
    );

    assert_eq!(
        session.fails("buzzard_check", json!({ "paths": ["pylavi", "gone"] })),
        "path does not exist: gone"
    );
    let unknown = session.fails(
        "buzzard_check",
        json!({ "paths": ["pylavi"], "ignore": ["BZ999"] }),
    );
    assert!(unknown.contains("unknown rule code `BZ999`"), "{unknown}");
    assert!(session
        .fails("buzzard_check", json!({ "paths": [] }))
        .contains("at least one"));

    let bad_format = session.fails(
        "buzzard_diagram",
        json!({ "path": "pylavi/add.vi", "format": "png" }),
    );
    assert!(
        bad_format.contains("markdown, json, mermaid, dot, svg"),
        "{bad_format}"
    );

    let outside = session.fails(
        "buzzard_callers",
        json!({ "path": "pylabview/empty_vifile.vi", "root": "pylavi" }),
    );
    assert_eq!(
        outside,
        "pylabview/empty_vifile.vi is not one of the 27 analyzable file(s) under root pylavi"
    );
    let no_root = session.fails(
        "buzzard_callees",
        json!({ "path": "pylavi/add.vi", "root": "nowhere" }),
    );
    assert_eq!(no_root, "root is not an existing directory: nowhere");
    let file_root = session.fails(
        "buzzard_symbols",
        json!({ "root": "pylavi/add.vi", "query": "" }),
    );
    assert!(file_root.starts_with("root is not an existing directory"));

    // The session is intact afterwards.
    session.ok("buzzard_inspect", json!({ "path": "pylavi/add.vi" }));
}

#[test]
fn a_project_file_has_no_diagram() {
    let mut session = Session::new(projects());
    let message = session.fails(
        "buzzard_diagram",
        json!({ "path": "jki-smo/URI.lvlib", "format": "svg" }),
    );
    assert_eq!(
        message,
        "jki-smo/URI.lvlib: a project file has no block diagram"
    );
}

#[test]
fn bad_arguments_are_tool_errors_that_name_the_argument() {
    let mut session = Session::fixtures();
    let cases = [
        (
            "buzzard_inspect",
            json!({}),
            "missing required argument `path`",
        ),
        (
            "buzzard_inspect",
            json!({ "path": 7 }),
            "argument `path` must be a string",
        ),
        (
            "buzzard_inspect",
            json!({ "path": "" }),
            "argument `path` must not be empty",
        ),
        (
            "buzzard_inspect",
            json!({ "path": ["a"] }),
            "argument `path` must be a string",
        ),
        (
            "buzzard_inspect",
            json!({ "path": "pylavi/add.vi", "file": "x" }),
            "unknown argument `file`; accepted arguments: path",
        ),
        (
            "buzzard_rules",
            json!({ "x": 1 }),
            "unknown argument `x`; accepted arguments: none",
        ),
        (
            "buzzard_check",
            json!({}),
            "missing required argument `paths`",
        ),
        (
            "buzzard_check",
            json!({ "paths": "pylavi" }),
            "argument `paths` must be an array of strings",
        ),
        (
            "buzzard_check",
            json!({ "paths": ["pylavi", 3] }),
            "argument `paths` must be an array of strings",
        ),
        (
            "buzzard_check",
            json!({ "paths": [""] }),
            "argument `paths` contains an empty path",
        ),
        (
            "buzzard_check",
            json!({ "paths": ["pylavi"], "refresh": "yes" }),
            "argument `refresh` must be true or false",
        ),
        (
            "buzzard_check",
            json!({ "paths": ["pylavi"], "limit": 0 }),
            "argument `limit` must be a positive integer",
        ),
        (
            "buzzard_symbols",
            json!({ "root": "pylavi" }),
            "missing required argument `query`",
        ),
        (
            "buzzard_symbols",
            json!({ "root": "pylavi", "query": "a", "limit": -5 }),
            "argument `limit` must be a positive integer",
        ),
        (
            "buzzard_diagram",
            json!({ "path": "pylavi/add.vi", "format": null }),
            "argument `format` must be a string",
        ),
        (
            "buzzard_callers",
            json!({ "path": "pylavi/add.vi" }),
            "missing required argument `root`",
        ),
    ];
    for (tool, arguments, expected) in cases {
        assert_eq!(
            session.fails(tool, arguments.clone()),
            expected,
            "{tool} {arguments}"
        );
    }
}

#[test]
fn protocol_errors_use_the_json_rpc_codes() {
    let mut session = Session::fixtures();
    for method in [
        "resources/list",
        "prompts/list",
        "tools/unknown",
        "",
        "shutdown",
    ] {
        assert_eq!(session.error_code(method, Value::Null), -32601, "{method}");
    }
    let reply = session.request("no/such/method", Value::Null);
    assert_eq!(
        reply["error"]["message"],
        "Method not found: no/such/method"
    );

    // tools/call with a malformed envelope or an unknown tool.
    for params in [
        Value::Null,
        json!([]),
        json!({}),
        json!({ "name": 3 }),
        json!({ "name": "buzzard_rules", "arguments": [] }),
        json!({ "name": "buzzard_rules", "arguments": "x" }),
        json!({ "name": "no_such_tool", "arguments": {} }),
        json!({ "name": "BUZZARD_RULES" }),
    ] {
        assert_eq!(
            session.error_code("tools/call", params.clone()),
            -32602,
            "{params}"
        );
    }
    let reply = session.request("tools/call", json!({ "name": "no_such_tool" }));
    assert_eq!(reply["error"]["message"], "Unknown tool: no_such_tool");
}

#[test]
fn malformed_messages_are_answered_and_the_session_continues() {
    let mut session = Session::fixtures();
    let code = |reply: Option<Value>| {
        let reply = reply.expect("an error reply");
        assert_eq!(reply["jsonrpc"], "2.0");
        (
            reply["error"]["code"].as_i64().unwrap(),
            reply["id"].clone(),
        )
    };
    for line in [
        "garbage",
        "{",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping""#,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"} trailing",
        "\u{0}",
        "Content-Length: 52",
    ] {
        assert_eq!(code(session.raw(line)), (-32700, Value::Null), "{line}");
    }
    for line in [
        "1",
        "\"ping\"",
        "null",
        "true",
        "[]",
        r#"{"jsonrpc":"2.0"}"#,
        "{}",
    ] {
        assert_eq!(code(session.raw(line)), (-32600, Value::Null), "{line}");
    }
    // The id is echoed when it could be read.
    assert_eq!(
        code(session.raw(r#"{"jsonrpc":"2.0","id":5}"#)),
        (-32600, json!(5))
    );
    assert_eq!(
        code(session.raw(r#"{"jsonrpc":"2.0","id":5,"method":17}"#)),
        (-32600, json!(5))
    );
    for id in ["{}", "[1]", "true"] {
        let line = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#);
        assert_eq!(code(session.raw(&line)), (-32600, Value::Null), "{line}");
    }
    assert_eq!(session.request("ping", Value::Null)["result"], json!({}));
}

#[test]
fn batches_are_answered_in_order() {
    let mut session = Session::fixtures();
    let reply = session
        .raw(
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},7,{"jsonrpc":"2.0","id":2,"method":"nope"}]"#,
        )
        .unwrap();
    let replies = reply.as_array().unwrap();
    assert_eq!(replies.len(), 3);
    assert_eq!(
        replies[0],
        json!({ "jsonrpc": "2.0", "id": 1, "result": {} })
    );
    assert_eq!(replies[1]["error"]["code"], -32600);
    assert_eq!(replies[2]["error"]["code"], -32601);
    assert_eq!(replies[2]["id"], 2);
    // A batch of notifications owes nothing.
    assert_eq!(
        session.raw(r#"[{"jsonrpc":"2.0","method":"notifications/initialized"}]"#),
        None
    );
}

// ------------------------------------------------------------ hostile input

#[test]
fn huge_and_hostile_arguments_are_rejected_cleanly() {
    let mut session = Session::fixtures();
    let long = "a/".repeat(100_000);
    let message = session.fails("buzzard_inspect", json!({ "path": long }));
    assert_eq!(
        message,
        "argument `path` is 200000 bytes long; at most 4096 are accepted"
    );
    assert!(session
        .fails(
            "buzzard_inspect",
            json!({ "path": "pylavi/add.vi\u{0}.txt" })
        )
        .contains("NUL"));
    assert!(session
        .fails(
            "buzzard_symbols",
            json!({ "root": "pylavi", "query": "x".repeat(5000) })
        )
        .contains("at most 4096"));

    let many: Vec<&str> = vec!["pylavi"; 10_000];
    let message = session.fails("buzzard_check", json!({ "paths": many }));
    assert_eq!(
        message,
        "argument `paths` has 10000 entries; at most 256 are accepted"
    );
    let codes: Vec<&str> = vec!["BZ001"; 257];
    assert!(session
        .fails(
            "buzzard_check",
            json!({ "paths": ["pylavi"], "ignore": codes })
        )
        .contains("at most 256"));

    // An absurd limit is clamped rather than trusted.
    let (_, structured) = session.ok(
        "buzzard_symbols",
        json!({ "root": "pylavi", "query": "", "limit": u64::MAX }),
    );
    assert_eq!(structured["symbols"].as_array().unwrap().len(), 27);
    session.ok(
        "buzzard_check",
        json!({ "paths": ["pylavi"], "limit": 1_000_000_000_000_u64 }),
    );

    // Long, attacker-chosen names are not echoed back in full.
    let name = "x".repeat(100_000);
    let reply = session.request("tools/call", json!({ "name": name }));
    assert!(reply["error"]["message"].as_str().unwrap().len() < 200);
    let reply = session.request(&name, Value::Null);
    assert_eq!(reply["error"]["code"], -32601);
    assert!(reply["error"]["message"].as_str().unwrap().len() < 200);
    let mut arguments = serde_json::Map::new();
    arguments.insert(name, json!(1));
    let message = session.fails("buzzard_rules", Value::Object(arguments));
    assert!(message.len() < 200, "{}", message.len());

    // Query characters that mean something in Markdown or regexes are literal.
    session.ok(
        "buzzard_symbols",
        json!({ "root": "pylavi", "query": "`*[](.*)\\ \u{202e}\u{1F980}" }),
    );
}

#[test]
fn deeply_nested_json_is_a_parse_error_not_a_stack_overflow() {
    let mut session = Session::fixtures();
    let line = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
    let reply = session.raw(&line).unwrap();
    assert_eq!(reply["error"]["code"], -32700);
    let nested = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"buzzard_rules","arguments":{{"x":{}1{}}}}}}}"#,
        "[".repeat(100),
        "]".repeat(100)
    );
    let reply = session.raw(&nested).unwrap();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
}

#[test]
fn paths_may_leave_the_base_directory_but_nothing_is_written() {
    let before = snapshot(&fixtures());
    let mut session = Session::new(fixtures().join("pylavi"));
    // `..` is resolved lexically; a file outside the base is shown as given.
    let (_, structured) = session.ok(
        "buzzard_inspect",
        json!({ "path": "../pylabview/empty_vifile.vi" }),
    );
    assert!(structured["path"]
        .as_str()
        .unwrap()
        .ends_with("/pylabview/empty_vifile.vi"));
    let absolute = fixtures().join("pylavi/add.vi");
    let (_, structured) = session.ok("buzzard_inspect", json!({ "path": absolute }));
    assert_eq!(structured["path"], "add.vi");

    // Exercise every tool, then prove the fixture tree is byte-identical.
    session.ok("buzzard_check", json!({ "paths": ["."], "refresh": true }));
    session.ok("buzzard_check", json!({ "paths": [".."] }));
    for format in ["markdown", "json", "mermaid", "dot", "svg"] {
        session.ok(
            "buzzard_diagram",
            json!({ "path": "add.vi", "format": format }),
        );
    }
    session.ok(
        "buzzard_callers",
        json!({ "path": "empty.vi", "root": "." }),
    );
    session.ok(
        "buzzard_callees",
        json!({ "path": "relative_link.vi", "root": "." }),
    );
    session.ok("buzzard_symbols", json!({ "root": "..", "query": "e" }));
    session.ok("buzzard_rules", json!({}));
    assert_eq!(snapshot(&fixtures()), before);
}

#[test]
fn every_fixture_can_be_inspected_and_exported_without_a_protocol_error() {
    let mut session = Session::fixtures();
    let (_, all) = session.ok(
        "buzzard_symbols",
        json!({ "root": ".", "query": "", "limit": 1000 }),
    );
    let symbols = all["symbols"].as_array().unwrap();
    assert!(symbols.len() >= 28);
    for symbol in symbols {
        let path = symbol["path"].as_str().unwrap();
        session.call("buzzard_inspect", json!({ "path": path }));
        for format in ["markdown", "json", "mermaid", "dot", "svg"] {
            session.call("buzzard_diagram", json!({ "path": path, "format": format }));
        }
        session.call("buzzard_callers", json!({ "path": path, "root": "." }));
        session.call("buzzard_callees", json!({ "path": path, "root": "." }));
    }
}

#[test]
fn corrupted_files_give_tool_errors_or_results_never_a_dead_session() {
    let original = fs::read(fixtures().join("pylavi/add.vi")).unwrap();
    let dir = TempDir::new("corrupt");
    let mut session = Session::new(dir.0.clone());
    let mut variants: Vec<Vec<u8>> = vec![Vec::new(), b"RSRC".to_vec(), vec![0xFF; 4096]];
    for cut in [1, 16, 31, 32, 64, original.len() / 2, original.len() - 1] {
        variants.push(original[..cut].to_vec());
    }
    for step in 0..64 {
        let mut bytes = original.clone();
        let at = (step * 7919) % bytes.len();
        bytes[at] ^= 0xFF;
        variants.push(bytes);
    }
    for (index, bytes) in variants.iter().enumerate() {
        let name = format!("case{index}.vi");
        fs::write(dir.0.join(&name), bytes).unwrap();
        session.call("buzzard_inspect", json!({ "path": name }));
        for format in ["markdown", "json", "mermaid", "dot", "svg"] {
            session.call("buzzard_diagram", json!({ "path": name, "format": format }));
        }
    }
    let (_, structured) = session.ok("buzzard_check", json!({ "paths": ["."] }));
    assert_eq!(structured["summary"]["files"], variants.len());
    session.ok(
        "buzzard_callers",
        json!({ "path": "case0.vi", "root": "." }),
    );
}

// -------------------------------------------------------------------- cache

#[test]
fn the_cache_follows_changes_on_disk() {
    let dir = TempDir::new("cache");
    dir.vi("Leaf.vi", &[]);
    dir.vi("Main.vi", &["Leaf.vi"]);
    let mut session = Session::new(dir.0.clone());
    let callers = |session: &mut Session, refresh: Option<bool>| {
        let mut arguments = json!({ "path": "Leaf.vi", "root": "." });
        if let Some(refresh) = refresh {
            arguments["refresh"] = json!(refresh);
        }
        session.ok("buzzard_callers", arguments).1["callers"].clone()
    };
    assert_eq!(callers(&mut session, None), json!(["Main.vi"]));

    // A new file appears without any refresh request.
    dir.vi("Other.vi", &["Leaf.vi"]);
    assert_eq!(callers(&mut session, None), json!(["Main.vi", "Other.vi"]));

    // A rewritten file with a different size is picked up as well.
    dir.vi("Main.vi", &[]);
    assert_eq!(callers(&mut session, Some(false)), json!(["Other.vi"]));

    // So is a deleted one, and an explicit refresh always works.
    fs::remove_file(dir.0.join("Other.vi")).unwrap();
    assert_eq!(callers(&mut session, None), json!([]));
    dir.vi("Main.vi", &["Leaf.vi"]);
    assert_eq!(callers(&mut session, Some(true)), json!(["Main.vi"]));

    // The other workspace tools see the same state.
    let (_, found) = session.ok("buzzard_symbols", json!({ "root": ".", "query": "" }));
    assert_eq!(found["symbols"].as_array().unwrap().len(), 2);
    let (_, checked) = session.ok("buzzard_check", json!({ "paths": ["."] }));
    assert_eq!(checked["summary"]["files"], 2);
    fs::remove_file(dir.0.join("Leaf.vi")).unwrap();
    assert_eq!(
        session.fails("buzzard_callers", json!({ "path": "Leaf.vi", "root": "." })),
        "Leaf.vi is not one of the 1 analyzable file(s) under root ."
    );
}

#[test]
fn a_cycle_is_reported_with_its_related_files() {
    let dir = TempDir::new("cycle");
    dir.vi("A.vi", &["B.vi"]);
    dir.vi("B.vi", &["A.vi"]);
    let mut session = Session::new(dir.0.clone());
    let (_, structured) = session.ok("buzzard_check", json!({ "paths": ["."] }));
    assert_eq!(structured["summary"]["errors"], 2);
    assert_eq!(structured["summary"]["by_code"], json!({ "BZ005": 2 }));
    let first = &structured["diagnostics"][0];
    assert_eq!(first["path"], "A.vi");
    assert_eq!(first["severity"], "error");
    let related = first["related"].as_array().unwrap();
    assert!(related.contains(&json!("B.vi")), "{first}");
}

// ---------------------------------------------------------------- transport

/// A whole session through `serve`, as bytes in and bytes out.
fn serve(input: &[u8]) -> Vec<Value> {
    let mut output = Vec::new();
    Server::new(fixtures())
        .serve(Cursor::new(input), &mut output)
        .expect("in-memory I/O cannot fail");
    let text = String::from_utf8(output).expect("the output is UTF-8");
    assert!(text.is_empty() || text.ends_with('\n'));
    text.lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn script() -> Vec<u8> {
    let lines = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }).to_string(),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }).to_string(),
        "this is not json".to_owned(),
        String::new(),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "buzzard_check", "arguments": { "paths": ["pylavi"] } } }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "buzzard_inspect", "arguments": { "path": "pylavi/missing.vi" } } }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/list" }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "buzzard_diagram", "arguments": { "path": "pylavi/add.vi", "format": "svg" } } }).to_string(),
        json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": { "name": "buzzard_symbols", "arguments": { "root": "pylavi", "query": "empty" } } }).to_string(),
    ];
    // CRLF line endings are tolerated, and the last line needs no newline.
    lines.join("\r\n").into_bytes()
}

#[test]
fn serve_runs_a_whole_session_over_a_reader_and_writer() {
    let replies = serve(&script());
    let ids: Vec<&Value> = replies.iter().map(|reply| &reply["id"]).collect();
    assert_eq!(
        ids,
        [
            &json!(1),
            &json!(2),
            &json!(3),
            &Value::Null,
            &json!(4),
            &json!(5),
            &json!(6),
            &json!(7),
            &json!(8)
        ]
    );
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "buzzard");
    assert_eq!(replies[2]["result"]["tools"].as_array().unwrap().len(), 8);
    assert_eq!(replies[3]["error"]["code"], -32700);
    assert_eq!(replies[4]["result"]["isError"], false);
    assert_eq!(replies[5]["result"]["isError"], true);
    assert_eq!(replies[6]["error"]["code"], -32601);
    assert!(text(&replies[7]["result"]).starts_with("<svg"));
    assert_eq!(replies[8]["result"]["isError"], false);
}

#[test]
fn output_is_byte_identical_across_runs() {
    let run = || {
        let mut output = Vec::new();
        Server::new(fixtures())
            .serve(Cursor::new(script()), &mut output)
            .unwrap();
        output
    };
    let first = run();
    assert!(!first.is_empty(), "the script asks for replies");
    for _ in 0..3 {
        assert_eq!(run(), first);
    }
}

#[test]
fn serve_survives_binary_garbage_and_oversized_lines() {
    let mut input = Vec::new();
    input.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x80, b'\n']);
    input.extend(std::iter::repeat_n(b'x', MAX_LINE_BYTES + 10));
    input.push(b'\n');
    // A large but acceptable message still works.
    let padded = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping", "params": { "pad": "y".repeat(1 << 20) } });
    input.extend_from_slice(padded.to_string().as_bytes());
    input.push(b'\n');
    input.extend_from_slice(b"\n\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
    let replies = serve(&input);
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert!(replies[0]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("UTF-8"));
    assert_eq!(replies[1]["error"]["code"], -32700);
    assert!(replies[1]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("longer than"));
    assert_eq!(
        replies[2],
        json!({ "jsonrpc": "2.0", "id": 1, "result": {} })
    );
    assert_eq!(
        replies[3],
        json!({ "jsonrpc": "2.0", "id": 2, "result": {} })
    );
    let silent = serve(b"");
    assert!(silent.is_empty(), "expected nothing, got {silent:?}");
}

// ------------------------------------------------------------ verification

/// Real state machines; `crates/buzzard-lint/tests/fixtures/verify/README.md`
/// records their sources and licences.
fn machines() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-lint/tests/fixtures/verify")
}

#[test]
fn verify_describes_machines_and_labels_findings() {
    let mut session = Session::new(machines());
    let (text, structured) = session.ok("buzzard_verify", json!({ "paths": ["."] }));
    assert!(text.starts_with(
        "jki-smo/process.vi\n  state machine at while loop #3379, case structure #3023\n    \
         states (2): Init, Idle\n    initial state: Init\n    default state: Idle\n"
    ));
    assert!(text.contains("      Idle -> Idle, Init; may stop the loop\n"));
    assert!(text.contains(
        "jki-state-machine/new-support-vis-examples.vi\n  string/queue state machine at while \
         loop #70, case structure #177 (recognised, not modelled)\n"
    ));
    assert!(text.contains(
        "    HEURISTIC [BZ203] state `Action: Step 1` is enqueued but no case handles it; the \
         default case `Default` runs instead\n      why: the name appears in a string constant"
    ));
    assert!(text.ends_with(
        "Checked 5 files: 3 with a state machine (1 state machine, 2 string/queue machines), 2 \
         without.\n3 findings: 0 proven, 0 structural, 3 heuristic.\n"
    ));

    assert_eq!(structured["summary"]["files"], 5);
    assert_eq!(structured["summary"]["heuristic"], 3);
    let files = structured["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        [
            "jki-smo/process.vi",
            "jki-state-machine/new-support-vis-examples.vi",
            "jki-state-machine/queued-state-machine-template.vi",
        ]
    );
    let machine = &files[0]["machines"][0];
    assert_eq!(machine["machine"]["default"], "Idle");
    assert_eq!(machine["exact"], false);
    let finding = &files[1]["findings"][0];
    assert_eq!(finding["code"], "BZ203");
    assert_eq!(finding["confidence"], "heuristic");
    assert!(finding["justification"].as_str().unwrap().len() > 20);

    // The JSON format is the same data as text; a second call is identical.
    let (json_text, json_structured) = session.ok(
        "buzzard_verify",
        json!({ "paths": ["."], "format": "json" }),
    );
    assert_eq!(json_structured, structured);
    assert_eq!(
        serde_json::from_str::<Value>(&json_text).unwrap(),
        structured
    );
    let again = session.ok("buzzard_verify", json!({ "paths": ["."] }));
    assert_eq!(again, (text, structured));
}

#[test]
fn verify_draws_the_transition_graph_and_checks_its_arguments() {
    let mut session = Session::new(machines());
    let result = session.call(
        "buzzard_verify",
        json!({ "paths": ["jki-smo/process.vi"], "format": "dot" }),
    );
    assert_eq!(result["isError"], false);
    assert!(result.get("structuredContent").is_none());
    let dot = text(&result);
    assert!(dot.starts_with("digraph state_machines {\n"));
    assert!(dot.contains("label=\"jki-smo/process.vi: while loop #3379, case structure #3023\";"));
    assert!(dot.contains("m0_s0 -> m0_s1;"));

    // A file without a machine is counted, not listed.
    let (text, structured) = session.ok(
        "buzzard_verify",
        json!({ "paths": ["lv-mqtt-broker", "lv-mqtt-broker/test-simple-connection.vi"] }),
    );
    assert_eq!(
        text,
        "Checked 1 file: 0 with a state machine (0 state machines, 0 string/queue machines), 1 \
         without.\nNo findings.\n"
    );
    assert_eq!(structured["files"], json!([]));

    for (arguments, expected) in [
        (json!({}), "missing required argument `paths`"),
        (json!({ "paths": [] }), "must name at least one file"),
        (json!({ "paths": [""] }), "contains an empty path"),
        (json!({ "paths": "x" }), "must be an array of strings"),
        (
            json!({ "paths": ["nope.vi"] }),
            "path does not exist: nope.vi",
        ),
        (
            json!({ "paths": ["."], "format": "svg" }),
            "argument `format` must be one of: text, json, dot",
        ),
        (
            json!({ "paths": ["."], "refresh": true }),
            "unknown argument `refresh`",
        ),
    ] {
        let message = session.fails("buzzard_verify", arguments);
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn check_reports_state_machine_findings() {
    let mut session = Session::new(machines());
    let (text, structured) = session.ok("buzzard_check", json!({ "paths": ["jki-state-machine"] }));
    assert_eq!(structured["summary"]["by_code"]["BZ203"], 3);
    assert!(text.contains(
        "jki-state-machine/new-support-vis-examples.vi: warning[BZ203]: string/queue state \
         machine at while loop #70, case structure #177: state `Action: Step 2` is enqueued"
    ));
    let (_, rules) = session.ok("buzzard_rules", json!({}));
    let codes: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["code"].as_str().unwrap())
        .collect();
    assert!(codes.ends_with(&["BZ201", "BZ202", "BZ203"]));
}
