//! Drives a real `buzzard mcp` process over pipes, as an AI agent's client would.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use common::*;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Every line the server writes to stdout, verbatim.
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    /// Start the server with `dir` as its working directory.
    fn start(dir: &std::path::Path) -> Self {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start `buzzard mcp`");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        writeln!(stdin, "{line}").expect("write to the server");
        stdin.flush().expect("flush to the server");
    }

    /// The next stdout line, which must be exactly one JSON-RPC message.
    fn receive(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("the server answers in time");
        let message: Value = serde_json::from_str(&line)
            .unwrap_or_else(|err| panic!("stdout carried a non-protocol line ({err}): {line}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
        message
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send_line(&message.to_string());
        let reply = self.receive();
        assert_eq!(reply["id"], self.next_id, "{reply}");
        reply
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let mut params = json!({ "name": name });
        params["arguments"] = arguments;
        let reply = self.request("tools/call", params);
        reply["result"].clone()
    }

    /// Closes stdin and waits for the process. Returns `(exit ok, stderr)`.
    fn finish(mut self) -> (bool, String) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("wait for the server");
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .expect("stderr is piped")
            .read_to_string(&mut stderr)
            .expect("read the server's stderr");
        assert!(
            self.lines.recv_timeout(TIMEOUT).is_err(),
            "no unsolicited output before exit"
        );
        (status.success(), stderr)
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"]
        .as_str()
        .expect("the content is text")
}

#[test]
#[expect(
    clippy::similar_names,
    reason = "`callers` and `callees` hold the answers of `buzzard_callers` and `buzzard_callees`"
)]
fn full_session_over_real_pipes() {
    let fixtures = fixtures().canonicalize().unwrap();
    let mut session = Session::start(&fixtures);

    let init = session.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "integration-test", "version": "0" },
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["capabilities"], json!({ "tools": {} }));
    assert_eq!(init["result"]["serverInfo"]["name"], "buzzard");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    // A notification produces no output, so the next reply answers the
    // ping.
    session.send_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(session.request("ping", Value::Null)["result"], json!({}));

    let tools = session.request("tools/list", Value::Null);
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "buzzard_check",
            "buzzard_verify",
            "buzzard_deps",
            "buzzard_inspect",
            "buzzard_diagram",
            "buzzard_callers",
            "buzzard_callees",
            "buzzard_symbols",
            "buzzard_rules",
        ]
    );

    // Relative paths resolve against the working directory of the process.
    let found = session.call(
        "buzzard_symbols",
        json!({ "root": ".", "query": "rellink" }),
    );
    assert_eq!(found["isError"], false, "{found}");
    assert_eq!(
        found["structuredContent"]["symbols"],
        json!([{ "name": "relative_link.vi", "kind": "vi", "path": "relative_link.vi" }])
    );
    let inspected = session.call("buzzard_inspect", json!({ "path": "relative_link.vi" }));
    assert!(text(&inspected).starts_with("### relative_link.vi\n"));
    assert_eq!(inspected["structuredContent"]["metadata"]["kind"], "vi");
    let callees = session.call(
        "buzzard_callees",
        json!({ "path": "relative_link.vi", "root": "." }),
    );
    assert_eq!(callees["structuredContent"]["callees"], json!(["empty.vi"]));
    let callers = session.call(
        "buzzard_callers",
        json!({ "path": "empty.vi", "root": "." }),
    );
    assert_eq!(
        callers["structuredContent"]["callers"],
        json!(["relative_link.vi"])
    );
    let diagram = session.call(
        "buzzard_diagram",
        json!({ "path": "add.vi", "format": "mermaid" }),
    );
    assert!(text(&diagram).starts_with("flowchart"), "{diagram}");
    let checked = session.call(
        "buzzard_check",
        json!({ "paths": ["."], "ignore": ["BZ001"] }),
    );
    assert_eq!(checked["isError"], false);
    assert_eq!(checked["structuredContent"]["summary"]["files"], 27);
    assert!(checked["structuredContent"]["summary"]["by_code"]
        .get("BZ001")
        .is_none());
    let rules = session.call("buzzard_rules", json!({}));
    assert!(text(&rules).contains("`BZ000`"));

    // Three failures in a row: a tool error, a protocol error, and a parse
    // error.
    let missing = session.call("buzzard_inspect", json!({ "path": "missing.vi" }));
    assert_eq!(missing["isError"], true);
    assert_eq!(text(&missing), "file does not exist: missing.vi");
    let unknown = session.request("resources/list", Value::Null);
    assert_eq!(unknown["error"]["code"], -32601);
    session.send_line("{ this is not json");
    let parse = session.receive();
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(parse["id"], Value::Null);
    assert_eq!(session.request("ping", Value::Null)["result"], json!({}));

    // Closing stdin ends the server cleanly, and it logged nothing.
    let (ok, stderr) = session.finish();
    assert!(ok);
    assert_eq!(stderr, "");
}

#[test]
fn sees_files_written_during_the_session() {
    let dir = TempDir::new("mcp");
    dir.vi("Leaf.vi", &CLEAN, &[]);
    dir.vi("Main.vi", &CLEAN, &["Leaf.vi"]);
    let mut session = Session::start(&dir.0);
    let callers = |session: &mut Session| {
        let result = session.call("buzzard_callers", json!({ "path": "Leaf.vi", "root": "." }));
        result["structuredContent"]["callers"].clone()
    };
    assert_eq!(callers(&mut session), json!(["Main.vi"]));
    dir.vi("Second.vi", &CLEAN, &["Leaf.vi"]);
    assert_eq!(callers(&mut session), json!(["Main.vi", "Second.vi"]));
    let (ok, stderr) = session.finish();
    assert!(ok);
    assert_eq!(stderr, "");
}
