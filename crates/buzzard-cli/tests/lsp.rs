//! Drives a real `buzzard lsp` process over stdio, as an editor would.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);

/// `file://` URI for a path, in the form the server reports: percent-encoded
/// where the path has a space or a character outside ASCII.
fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .unwrap_or_else(|()| panic!("not an absolute path: {}", path.display()))
        .to_string()
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    backlog: Vec<Value>,
    next_id: u64,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(BIN)
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start `buzzard lsp`");
        let stdin = child.stdin.take().expect("stdin is piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut length = None;
            loop {
                let mut line = String::new();
                if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    length = value.trim().parse::<usize>().ok();
                }
            }
            let mut body = vec![0; length.expect("message without Content-Length")];
            if stdout.read_exact(&mut body).is_err() {
                return;
            }
            let message = serde_json::from_slice(&body).expect("the server sends JSON");
            if tx.send(message).is_err() {
                return;
            }
        });
        Self {
            child,
            stdin,
            incoming,
            backlog: Vec::new(),
            next_id: 0,
        }
    }

    fn frame(message: &Value) -> String {
        let body = message.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    fn send(&mut self, message: &Value) {
        self.stdin
            .write_all(Self::frame(message).as_bytes())
            .expect("write to the server");
        self.stdin.flush().expect("flush to the server");
    }

    /// Start a server and complete the handshake for one workspace folder.
    fn open(root: &Path) -> Self {
        let mut session = Self::start();
        let init = session.request(
            "initialize",
            json!({ "processId": null, "rootUri": file_uri(root), "capabilities": {} }),
        );
        assert_eq!(init["serverInfo"]["name"], "buzzard");
        session.notify("initialized", json!({}));
        session
    }

    /// Tell the server that these files changed, as an editor may.
    fn changed(&mut self, paths: &[&Path]) {
        let changes: Vec<Value> = paths
            .iter()
            .map(|path| json!({ "uri": file_uri(path), "type": 2 }))
            .collect();
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": changes }),
        );
    }

    /// Every diagnostics notification received so far that no test step
    /// has claimed, as (URI, codes).
    fn unclaimed_diagnostics(&self) -> Vec<(String, Vec<String>)> {
        self.backlog
            .iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .map(|m| {
                let codes = m["params"]["diagnostics"]
                    .as_array()
                    .expect("published diagnostics are an array");
                (
                    m["params"]["uri"]
                        .as_str()
                        .expect("published diagnostics name a URI")
                        .to_owned(),
                    codes
                        .iter()
                        .map(|d| {
                            d["code"]
                                .as_str()
                                .expect("a diagnostic has a code")
                                .to_owned()
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Wait for the next diagnostics notification, whatever file it is for.
    fn next_diagnostics(&mut self) -> (String, Vec<Value>) {
        let message = self.wait_for("diagnostics", |m| {
            m["method"] == "textDocument/publishDiagnostics"
        });
        (
            message["params"]["uri"]
                .as_str()
                .expect("published diagnostics name a URI")
                .to_owned(),
            message["params"]["diagnostics"]
                .as_array()
                .expect("published diagnostics are an array")
                .clone(),
        )
    }

    /// URIs of every symbol matching `query`, sorted.
    fn symbol_uris(&mut self, query: &str) -> Vec<String> {
        let symbols = self.request("workspace/symbol", json!({ "query": query }));
        let mut uris: Vec<String> = symbols
            .as_array()
            .expect("workspace/symbol answers with an array")
            .iter()
            .map(|s| {
                s["location"]["uri"]
                    .as_str()
                    .expect("a symbol has a URI")
                    .to_owned()
            })
            .collect();
        uris.sort();
        uris
    }

    fn hover(&mut self, path: &Path) -> Value {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": file_uri(path) },
                "position": { "line": 0, "character": 0 },
            }),
        )
    }

    /// Wait for the process to end and return its exit status.
    fn exit_status(&mut self) -> i32 {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the server process") {
                return status.code().expect("server was killed by a signal");
            }
            assert!(Instant::now() < deadline, "server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(&message);
    }

    /// Wait for the first message matching `wanted`, keeping the others.
    fn wait_for(&mut self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        if let Some(at) = self.backlog.iter().position(&wanted) {
            return self.backlog.remove(at);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self.incoming.recv_timeout(left).unwrap_or_else(|_| {
                panic!("timed out waiting for {what}; saw {:#?}", self.backlog)
            });
            if wanted(&message) {
                return message;
            }
            self.backlog.push(message);
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(&message);
        let response = self.wait_for(method, |m| m["id"] == id && m.get("method").is_none());
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    /// Wait for diagnostics for the file whose URI ends with `suffix`.
    fn diagnostics_for(&mut self, suffix: &str, wanted: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let message = self.wait_for(&format!("diagnostics for {suffix}"), |m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"]
                    .as_str()
                    .is_some_and(|uri| uri.ends_with(suffix))
                && wanted(
                    m["params"]["diagnostics"]
                        .as_array()
                        .expect("published diagnostics are an array"),
                )
        });
        message["params"]["diagnostics"]
            .as_array()
            .expect("the matched message has diagnostics")
            .clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Fail with the unexpected items, which `assert!(v.is_empty())` would hide.
#[track_caller]
fn assert_none<T: std::fmt::Debug>(items: &[T]) {
    assert!(items.is_empty(), "expected nothing, got {items:?}");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one editor session from `initialize` to `exit`; each step depends on the state the previous ones left"
)]
fn full_editor_session() {
    let dir = TempDir::new("lsp");
    dir.vi("main.vi", &CLEAN, &["util.vi"]);
    dir.vi("util.vi", &INLINE_CODE, &[]);
    std::fs::write(dir.0.join("App.lvproj"), "<Project/>").unwrap();
    let uri = |name: &str| file_uri(&dir.0.join(name));

    let mut session = Session::start();
    let init = session.request(
        "initialize",
        json!({ "processId": null, "rootUri": file_uri(&dir.0), "capabilities": {} }),
    );
    assert_eq!(init["serverInfo"]["name"], "buzzard");
    assert_eq!(init["capabilities"]["hoverProvider"], true);
    assert_eq!(init["capabilities"]["referencesProvider"], true);
    assert_eq!(init["capabilities"]["workspaceSymbolProvider"], true);
    session.notify("initialized", json!({}));

    // Initial scan publishes the one problem in the workspace.
    let found = session.diagnostics_for("/util.vi", |d| !d.is_empty());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["code"], "BZ001");
    assert_eq!(found[0]["source"], "buzzard");
    assert_eq!(found[0]["severity"], 2);

    let symbols = session.request("workspace/symbol", json!({ "query": "" }));
    let mut names: Vec<&str> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["App.lvproj", "main.vi", "util.vi"]);
    let symbols = session.request("workspace/symbol", json!({ "query": "utl" }));
    assert_eq!(symbols[0]["name"], "util.vi");
    assert_eq!(symbols[0]["location"]["uri"], uri("util.vi"));

    let position = json!({ "line": 0, "character": 0 });
    let references = session.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri("util.vi") },
            "position": position,
            "context": { "includeDeclaration": false },
        }),
    );
    assert_eq!(references.as_array().unwrap().len(), 1);
    assert_eq!(references[0]["uri"], uri("main.vi"));

    let hover = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri("main.vi") }, "position": position }),
    );
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("| Saved version | 23.0.0 |"), "{text}");
    assert!(text.contains("| Callees in workspace | 1 |"), "{text}");

    let inspected = session.request("buzzard/inspect", json!({ "uri": uri("main.vi") }));
    assert_eq!(inspected["metadata"]["kind"], "vi");
    assert_eq!(inspected["callees"], json!([uri("util.vi")]));
    assert_eq!(inspected["callers"], json!([]));

    // Project files are parsed too: hover, inspect, and re-parse on save.
    let hover = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri("App.lvproj") }, "position": position }),
    );
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("| Kind | Project |"), "{text}");
    assert!(text.contains("| Members | 0 |"), "{text}");
    std::fs::write(
        dir.0.join("App.lvproj"),
        "<Project Type=\"Project\" LVVersion=\"23008000\">\
         <Item Name=\"main.vi\" Type=\"VI\" URL=\"../main.vi\"/>\
         <Item Name=\"lost.vi\" Type=\"VI\" URL=\"../lost.vi\"/></Project>",
    )
    .unwrap();
    let found = session.diagnostics_for("/App.lvproj", |d| !d.is_empty());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["code"], "BZ006");
    assert_eq!(found[0]["severity"], 2);
    let inspected = session.request("buzzard/inspect", json!({ "uri": uri("App.lvproj") }));
    assert_eq!(inspected["project"]["kind"], "project");
    assert_eq!(inspected["project"]["lv_version"], "23.0.0");
    assert_eq!(inspected["members"], json!([uri("main.vi")]));
    let inspected = session.request("buzzard/inspect", json!({ "uri": uri("main.vi") }));
    assert_eq!(inspected["owners"], json!([uri("App.lvproj")]));
    std::fs::write(dir.0.join("App.lvproj"), "<Project/>").unwrap();
    session.diagnostics_for("/App.lvproj", <[Value]>::is_empty);

    // The block diagram of a file on request, as a graph or as text.
    let real = fixtures().join("add.vi");
    let ast = session.request("buzzard/exportAst", json!({ "uri": file_uri(&real) }));
    assert_eq!(ast["diagram"]["root"]["nodes"][1]["class"], "prim");
    let ast = session.request(
        "buzzard/exportAst",
        json!({ "uri": file_uri(&real), "format": "markdown" }),
    );
    assert!(ast["markdown"]
        .as_str()
        .unwrap()
        .starts_with("## Block diagram"));

    // Saving a fixed file from the IDE is picked up by the file watcher.
    dir.vi("util.vi", &CLEAN, &[]);
    session.diagnostics_for("/util.vi", <[Value]>::is_empty);

    // A newly saved file that closes a cycle is reported on both members.
    dir.vi("util.vi", &CLEAN, &["main.vi"]);
    for name in ["/main.vi", "/util.vi"] {
        let found = session.diagnostics_for(name, |d| d.iter().any(|x| x["code"] == "BZ005"));
        assert_eq!(found[0]["code"], "BZ005");
        assert_eq!(found[0]["severity"], 1);
        assert_eq!(found[0]["relatedInformation"].as_array().unwrap().len(), 1);
    }

    // Deleting a file clears diagnostics and updates the graph.
    std::fs::remove_file(dir.0.join("util.vi")).unwrap();
    session.diagnostics_for("/main.vi", <[Value]>::is_empty);
    let symbols = session.request("workspace/symbol", json!({ "query": "util" }));
    assert_eq!(symbols, json!([]));

    assert_eq!(session.request("shutdown", Value::Null), Value::Null);
    session.notify("exit", Value::Null);
    // `exit` after `shutdown` is a clean end.
    assert_eq!(session.exit_status(), 0);
}

fn no_diagnostics() -> Vec<(String, Vec<String>)> {
    Vec::new()
}

/// The protocol asks for status 1 when `exit` arrives without `shutdown`,
/// so that a supervisor can tell an abrupt end from a clean one.
#[test]
fn exit_without_shutdown_is_status_one() {
    let dir = TempDir::new("lsp-exit");
    dir.vi("a.vi", &INLINE_CODE, &[]);
    let mut session = Session::open(&dir.0);
    session.diagnostics_for("/a.vi", |d| !d.is_empty());
    session.notify("exit", Value::Null);
    assert_eq!(session.exit_status(), 1);
}

/// Paths with a space or a non-ASCII character are percent-encoded in
/// URIs; the server and this test must agree on the exact form.
#[test]
fn workspace_path_with_space_and_non_ascii_characters() {
    let dir = TempDir::new("lsp with space \u{e9}");
    std::fs::create_dir_all(dir.0.join("sub dir")).unwrap();
    dir.vi("main.vi", &CLEAN, &["util.vi"]);
    dir.vi("util.vi", &INLINE_CODE, &[]);
    dir.vi("sub dir/my file.vi", &INLINE_CODE, &[]);
    let root_uri = file_uri(&dir.0);
    assert!(
        root_uri.ends_with("lsp%20with%20space%20%C3%A9"),
        "{root_uri}"
    );
    let uri = |name: &str| file_uri(&dir.0.join(name));

    let mut session = Session::open(&dir.0);
    let mut published = [session.next_diagnostics(), session.next_diagnostics()];
    published.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(published[0].0, uri("sub dir/my file.vi"));
    assert_eq!(published[1].0, uri("util.vi"));
    assert_eq!(published[1].1[0]["code"], "BZ001");
    assert_eq!(
        session.symbol_uris(""),
        [uri("main.vi"), uri("sub dir/my file.vi"), uri("util.vi")]
    );
    let references = session.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri("util.vi") },
            "position": { "line": 0, "character": 0 },
            "context": { "includeDeclaration": false },
        }),
    );
    assert_eq!(references.as_array().unwrap().len(), 1);
    assert_eq!(references[0]["uri"], uri("main.vi"));
    let hover = session.hover(&dir.0.join("sub dir/my file.vi"));
    assert!(hover["contents"]["value"].is_string(), "{hover}");

    // The watcher reports the same spelling.
    dir.vi("sub dir/my file.vi", &CLEAN, &[]);
    let (cleared, diagnostics) = session.next_diagnostics();
    assert_eq!(cleared, uri("sub dir/my file.vi"));
    assert_none(&diagnostics);
    assert_eq!(session.symbol_uris("my file"), [uri("sub dir/my file.vi")]);
}

/// A directory rename is reported for the directory alone. The files below
/// it must move with it, and a deleted directory must take them along.
#[test]
fn directory_rename_and_delete_are_followed() {
    let dir = TempDir::new("lsp-dirs");
    std::fs::create_dir_all(dir.0.join("sub/deep")).unwrap();
    dir.vi("sub/a.vi", &INLINE_CODE, &[]);
    dir.vi("sub/deep/b.vi", &INLINE_CODE, &[]);
    let uri = |name: &str| file_uri(&dir.0.join(name));

    let mut session = Session::open(&dir.0);
    session.diagnostics_for("/sub/a.vi", |d| !d.is_empty());
    session.diagnostics_for("/sub/deep/b.vi", |d| !d.is_empty());

    std::fs::rename(dir.0.join("sub"), dir.0.join("sub2")).unwrap();
    for name in ["/sub2/a.vi", "/sub2/deep/b.vi"] {
        let found = session.diagnostics_for(name, |d| !d.is_empty());
        assert_eq!(found[0]["code"], "BZ001");
    }
    session.diagnostics_for("/sub/a.vi", <[Value]>::is_empty);
    session.diagnostics_for("/sub/deep/b.vi", <[Value]>::is_empty);
    assert_eq!(
        session.symbol_uris(""),
        [uri("sub2/a.vi"), uri("sub2/deep/b.vi")]
    );
    let hover = session.hover(&dir.0.join("sub2/a.vi"));
    assert!(hover["contents"]["value"].is_string(), "{hover}");
    assert_eq!(session.hover(&dir.0.join("sub/a.vi")), Value::Null);

    // A directory prepared elsewhere and moved in as a whole.
    let staging = TempDir::new("lsp-dirs-staging");
    staging.vi("c.vi", &INLINE_CODE, &[]);
    std::fs::rename(&staging.0, dir.0.join("moved")).unwrap();
    session.diagnostics_for("/moved/c.vi", |d| !d.is_empty());

    std::fs::remove_dir_all(dir.0.join("sub2")).unwrap();
    session.diagnostics_for("/sub2/a.vi", <[Value]>::is_empty);
    session.diagnostics_for("/sub2/deep/b.vi", <[Value]>::is_empty);
    assert_eq!(session.symbol_uris(""), [uri("moved/c.vi")]);
    assert_eq!(session.unclaimed_diagnostics(), no_diagnostics());
}

/// The initial scan skips hidden directories and everything outside the
/// roots. Later events must not bring such files in, or the results would
/// depend on what happened while the server ran.
#[test]
fn files_that_the_scan_skips_stay_out_after_events() {
    let outer = TempDir::new("lsp-skipped");
    let root = outer.0.join("ws");
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::create_dir_all(outer.0.join("outside")).unwrap();
    outer.vi("ws/a.vi", &CLEAN, &[]);
    outer.vi("ws/first.vi", &INLINE_CODE, &[]);
    outer.vi("ws/.hidden/pre.vi", &INLINE_CODE, &[]);
    std::fs::write(root.join(".hidden/p.lvproj"), "<Project/>").unwrap();

    let mut session = Session::open(&root);
    session.diagnostics_for("/ws/first.vi", |d| !d.is_empty());
    let expected = [
        file_uri(&root.join("a.vi")),
        file_uri(&root.join("first.vi")),
    ];
    assert_eq!(session.symbol_uris(""), expected);

    outer.vi("ws/.hidden/h.vi", &INLINE_CODE, &[]);
    outer.vi("outside/o.vi", &INLINE_CODE, &[]);
    session.changed(&[
        &root.join(".hidden/h.vi"),
        &root.join(".hidden/pre.vi"),
        &root.join(".hidden/p.lvproj"),
        &outer.0.join("outside/o.vi"),
    ]);
    // Changes are handled in order, so once this later one is published
    // the ones above have been dealt with.
    std::thread::sleep(Duration::from_millis(400));
    outer.vi("ws/first.vi", &CLEAN, &[]);
    session.diagnostics_for("/ws/first.vi", <[Value]>::is_empty);

    assert_eq!(session.symbol_uris(""), expected);
    assert_eq!(session.unclaimed_diagnostics(), no_diagnostics());
    assert_eq!(session.hover(&root.join(".hidden/h.vi")), Value::Null);
    assert_eq!(session.hover(&outer.0.join("outside/o.vi")), Value::Null);
}

/// A workspace opened through a symbolic link: every file has one name,
/// the one under the root as the client spelled it, whatever spelling a
/// change is reported in. (`FSEvents` on macOS reports resolved paths.)
#[cfg(unix)]
#[test]
fn root_reached_through_a_symlink_keeps_one_name_per_file() {
    let outer = TempDir::new("lsp-link");
    std::fs::create_dir_all(outer.0.join("actual/ws")).unwrap();
    std::os::unix::fs::symlink(outer.0.join("actual"), outer.0.join("link")).unwrap();
    let given = outer.0.join("link/ws");
    let canonical = outer.0.join("actual/ws");
    assert_eq!(given.canonicalize().unwrap(), canonical);
    outer.vi("actual/ws/util.vi", &INLINE_CODE, &[]);
    let util = file_uri(&given.join("util.vi"));
    let extra = file_uri(&given.join("extra.vi"));

    let mut session = Session::open(&given);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, util);
    assert_eq!(diagnostics[0]["code"], "BZ001");

    // Fixed on disk, and reported by the client under the resolved path.
    outer.vi("actual/ws/util.vi", &CLEAN, &[]);
    session.changed(&[&canonical.join("util.vi")]);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, util);
    assert_none(&diagnostics);

    // A new file, reported under the resolved path as well.
    outer.vi("actual/ws/extra.vi", &INLINE_CODE, &[]);
    session.changed(&[&canonical.join("extra.vi")]);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, extra);
    assert_eq!(diagnostics.len(), 1);
    // Time for a second, wrongly named entry to show up if there is one.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(session.symbol_uris(""), [extra.clone(), util.clone()]);
    assert_eq!(session.unclaimed_diagnostics(), no_diagnostics());

    // Deleting it removes the one entry there is.
    std::fs::remove_file(canonical.join("extra.vi")).unwrap();
    session.changed(&[&canonical.join("extra.vi")]);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, extra);
    assert_none(&diagnostics);
    assert_eq!(session.symbol_uris(""), [util]);
}

/// The temporary directory as the system names it, which on macOS is a
/// path through a symbolic link (`/var` -> `/private/var`). The watcher
/// there reports resolved paths; diagnostics must still be published under
/// the root the client gave.
#[test]
fn root_given_as_the_uncanonical_temp_dir() {
    let name = format!("buzzard-cli-{}-lsp-raw-temp", std::process::id());
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dir = TempDir(root.clone());
    dir.vi("util.vi", &INLINE_CODE, &[]);
    let util = file_uri(&root.join("util.vi"));

    let mut session = Session::open(&root);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, util);
    assert_eq!(diagnostics.len(), 1);

    dir.vi("util.vi", &CLEAN, &[]);
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, util);
    assert_none(&diagnostics);
    assert_eq!(session.symbol_uris(""), [util]);
}

/// The IDE truncates a file and then writes it. A file read in between
/// does not parse; that is not a finding, and must not flash up as one.
#[test]
fn file_caught_in_the_middle_of_a_save_is_not_reported_as_broken() {
    let dir = TempDir::new("lsp-midsave");
    dir.vi("main.vi", &INLINE_CODE, &[]);
    std::fs::write(dir.0.join("App.lvproj"), "<Project/>").unwrap();
    let mut session = Session::open(&dir.0);
    session.diagnostics_for("/main.vi", |d| !d.is_empty());

    let full = "<Project Type=\"Project\" LVVersion=\"23008000\">\
                <Item Name=\"lost.vi\" Type=\"VI\" URL=\"../lost.vi\"/></Project>";
    let half = &full[..40];
    // Long enough for the watcher to look at the half-written file, short
    // enough for the save to complete while the server is still waiting.
    std::fs::write(dir.0.join("App.lvproj"), half).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(dir.0.join("App.lvproj"), full).unwrap();

    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, file_uri(&dir.0.join("App.lvproj")));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "BZ006");

    // A file that stays broken is still reported, a little later.
    std::fs::write(dir.0.join("App.lvproj"), half).unwrap();
    let (uri, diagnostics) = session.next_diagnostics();
    assert_eq!(uri, file_uri(&dir.0.join("App.lvproj")));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "BZ000");
}

/// A client that sends `initialize` and `initialized` in one write must
/// still get its workspace scanned.
#[test]
fn pipelined_handshake_still_scans() {
    let dir = TempDir::new("lsp-pipelined");
    dir.vi("util.vi", &INLINE_CODE, &[]);
    let mut session = Session::start();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "processId": null, "rootUri": file_uri(&dir.0), "capabilities": {} },
    });
    let initialized = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} });
    let both = Session::frame(&initialize) + &Session::frame(&initialized);
    session.stdin.write_all(both.as_bytes()).unwrap();
    session.stdin.flush().unwrap();
    session.next_id = 1;
    let found = session.diagnostics_for("/util.vi", |d| !d.is_empty());
    assert_eq!(found[0]["code"], "BZ001");
    assert_eq!(session.symbol_uris(""), [file_uri(&dir.0.join("util.vi"))]);
}
