//! Drives a real `buzzard lsp` process over stdio, as an editor would.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);

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
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
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
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
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

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message);
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
        self.send(message);
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
                && wanted(m["params"]["diagnostics"].as_array().unwrap())
        });
        message["params"]["diagnostics"].as_array().unwrap().clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn full_editor_session() {
    let dir = TempDir::new("lsp");
    dir.vi("main.vi", &CLEAN, &["util.vi"]);
    dir.vi("util.vi", &INLINE_CODE, &[]);
    std::fs::write(dir.0.join("App.lvproj"), "<Project/>").unwrap();
    let uri = |name: &str| format!("file://{}/{name}", dir.0.display());

    let mut session = Session::start();
    let init = session.request(
        "initialize",
        json!({ "processId": null, "rootUri": format!("file://{}", dir.0.display()), "capabilities": {} }),
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
    names.sort();
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

    // Saving a fixed file from the IDE is picked up by the file watcher.
    dir.vi("util.vi", &CLEAN, &[]);
    session.diagnostics_for("/util.vi", |d| d.is_empty());

    // A newly saved file that closes a cycle is reported on both members.
    dir.vi("util.vi", &CLEAN, &["main.vi"]);
    for name in ["/main.vi", "/util.vi"] {
        let found = session.diagnostics_for(name, |d| !d.is_empty());
        assert_eq!(found[0]["code"], "BZ005");
        assert_eq!(found[0]["severity"], 1);
        assert_eq!(found[0]["relatedInformation"].as_array().unwrap().len(), 1);
    }

    // Deleting a file clears diagnostics and updates the graph.
    std::fs::remove_file(dir.0.join("util.vi")).unwrap();
    session.diagnostics_for("/main.vi", |d| d.is_empty());
    let symbols = session.request("workspace/symbol", json!({ "query": "util" }));
    assert_eq!(symbols, json!([]));

    assert_eq!(session.request("shutdown", Value::Null), Value::Null);
    session.notify("exit", Value::Null);
    let deadline = Instant::now() + TIMEOUT;
    while session.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "server did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}
