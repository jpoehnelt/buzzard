//! JSON-RPC 2.0 framing and the MCP lifecycle methods.

use std::io::{self, BufRead, ErrorKind, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::tools::{self, State};

/// Protocol revisions this server speaks, newest first. The tool methods it
/// implements are the same in all of them.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Longest accepted message. A longer line is discarded and answered with a
/// parse error, so a runaway client cannot exhaust memory.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// One MCP session. Holds the workspace cache shared by the tools.
#[derive(Debug)]
pub struct Server {
    state: State,
}

impl Server {
    /// `base` is the directory that relative paths in tool arguments are
    /// resolved against, and that reported paths are made relative to.
    pub fn new(base: PathBuf) -> Self {
        Self {
            state: State::new(base),
        }
    }

    /// Read messages from `reader` until end of input, writing one line to
    /// `writer` per response. Only an I/O error on either side ends the
    /// loop early; malformed messages are answered and skipped.
    pub fn serve<R: BufRead, W: Write>(&mut self, mut reader: R, mut writer: W) -> io::Result<()> {
        let mut line = Vec::new();
        loop {
            let reply = match read_line(&mut reader, &mut line)? {
                Line::Eof => return Ok(()),
                Line::TooLong => Some(parse_error(&format!(
                    "message is longer than {MAX_LINE_BYTES} bytes"
                ))),
                Line::Complete => match std::str::from_utf8(&line) {
                    Ok(text) => self.handle_line(text),
                    Err(_) => Some(parse_error("message is not UTF-8")),
                },
            };
            if let Some(reply) = reply {
                writer.write_all(reply.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }

    /// Handle one line of input. Returns the line to send back, without a
    /// trailing newline, or `None` when nothing is owed (a notification, a
    /// client response or a blank line).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => return Some(parse_error(&err.to_string())),
        };
        let reply = match message {
            // Batches were part of the 2025-03-26 revision only; answering
            // them costs nothing and keeps such clients working.
            Value::Array(batch) if batch.is_empty() => Some(error(
                Value::Null,
                INVALID_REQUEST,
                "Invalid Request: empty batch",
            )),
            Value::Array(batch) => {
                let replies: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|message| self.handle_message(message))
                    .collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            message => self.handle_message(message),
        };
        reply.map(|reply| reply.to_string())
    }

    fn handle_message(&mut self, message: Value) -> Option<Value> {
        let Value::Object(mut message) = message else {
            return Some(error(
                Value::Null,
                INVALID_REQUEST,
                "Invalid Request: message is not a JSON object",
            ));
        };
        let id = match message.remove("id") {
            None => None,
            Some(id @ (Value::String(_) | Value::Number(_) | Value::Null)) => Some(id),
            Some(_) => {
                return Some(error(
                    Value::Null,
                    INVALID_REQUEST,
                    "Invalid Request: id must be a string or a number",
                ))
            }
        };
        let method = match message.remove("method") {
            Some(Value::String(method)) => method,
            // A response to a request; this server never sends any.
            None if message.contains_key("result") || message.contains_key("error") => return None,
            _ => {
                return Some(error(
                    id.unwrap_or(Value::Null),
                    INVALID_REQUEST,
                    "Invalid Request: method must be a string",
                ))
            }
        };
        // A notification is never answered, whatever it asks for.
        let id = id?;
        let params = message.remove("params").unwrap_or(Value::Null);
        Some(match self.dispatch(&method, params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => self.call_tool(params),
            other => {
                // Method names are client-chosen and can be arbitrarily long.
                let shown: String = other.chars().take(64).collect();
                Err((METHOD_NOT_FOUND, format!("Method not found: {shown}")))
            }
        }
    }

    fn call_tool(&mut self, params: Value) -> Result<Value, (i64, String)> {
        let invalid = |reason: &str| (INVALID_PARAMS, format!("Invalid params: {reason}"));
        let Value::Object(mut params) = params else {
            return Err(invalid("params must be an object"));
        };
        let Some(Value::String(name)) = params.remove("name") else {
            return Err(invalid("name must be a string"));
        };
        if !tools::NAMES.contains(&name.as_str()) {
            let shown: String = name.chars().take(64).collect();
            return Err((INVALID_PARAMS, format!("Unknown tool: {shown}")));
        }
        let arguments = match params.remove("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err(invalid("arguments must be an object")),
        };
        // No input the test suite has found makes the analysis panic, but a
        // tool call must not be able to take the session down if one does.
        let state = &mut self.state;
        let outcome = catch_unwind(AssertUnwindSafe(|| tools::call(state, &name, &arguments)));
        let outcome = outcome.unwrap_or_else(|_| {
            self.state.forget();
            eprintln!("buzzard mcp: internal error in {name}");
            Err(format!(
                "internal error in {name}; this is a bug in buzzard"
            ))
        });
        Ok(match outcome {
            Ok(output) => {
                let mut result = json!({
                    "content": [{ "type": "text", "text": output.text }],
                    "isError": false,
                });
                if let Some(structured) = output.structured {
                    result["structuredContent"] = structured;
                }
                result
            }
            Err(message) => json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            }),
        })
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = SUPPORTED_PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| Some(*supported) == requested)
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "buzzard",
            "title": "buzzard static analyzer",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": tools::INSTRUCTIONS,
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn parse_error(reason: &str) -> String {
    error(Value::Null, PARSE_ERROR, &format!("Parse error: {reason}")).to_string()
}

enum Line {
    Complete,
    TooLong,
    Eof,
}

/// Read up to the next newline into `line`, without the terminator. Bytes
/// past [`MAX_LINE_BYTES`] are consumed but not stored.
fn read_line<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<Line> {
    line.clear();
    let mut seen_any = false;
    let mut too_long = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        if available.is_empty() {
            if !seen_any {
                return Ok(Line::Eof);
            }
            break;
        }
        seen_any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        if too_long || line.len() + chunk.len() > MAX_LINE_BYTES {
            too_long = true;
        } else {
            line.extend_from_slice(chunk);
        }
        let consumed = chunk.len() + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    Ok(if too_long {
        Line::TooLong
    } else {
        Line::Complete
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Vec<Option<Vec<u8>>> {
        let mut reader = io::BufReader::with_capacity(7, input);
        let mut line = Vec::new();
        let mut out = Vec::new();
        loop {
            match read_line(&mut reader, &mut line).unwrap() {
                Line::Eof => return out,
                Line::Complete => out.push(Some(line.clone())),
                Line::TooLong => out.push(None),
            }
        }
    }

    #[test]
    fn splits_lines_across_buffer_refills() {
        assert_eq!(
            lines(b"first line here\n\nlast without newline"),
            vec![
                Some(b"first line here".to_vec()),
                Some(Vec::new()),
                Some(b"last without newline".to_vec()),
            ]
        );
        assert!(lines(b"").is_empty());
    }

    #[test]
    fn drops_an_oversized_line_and_recovers() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1];
        input.extend_from_slice(b"\nnext\n");
        assert_eq!(lines(&input), vec![None, Some(b"next".to_vec())]);
        let mut exact = vec![b'x'; MAX_LINE_BYTES];
        exact.push(b'\n');
        let exact = lines(&exact);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].as_ref().map(Vec::len), Some(MAX_LINE_BYTES));
    }

    #[test]
    fn negotiates_the_protocol_version() {
        let version = |params: Value| initialize(&params)["protocolVersion"].clone();
        assert_eq!(
            version(json!({ "protocolVersion": "2024-11-05" })),
            "2024-11-05"
        );
        assert_eq!(
            version(json!({ "protocolVersion": "2025-03-26" })),
            "2025-03-26"
        );
        assert_eq!(
            version(json!({ "protocolVersion": "1999-01-01" })),
            "2025-06-18"
        );
        assert_eq!(version(json!({ "protocolVersion": 7 })), "2025-06-18");
        assert_eq!(version(Value::Null), "2025-06-18");
    }
}
