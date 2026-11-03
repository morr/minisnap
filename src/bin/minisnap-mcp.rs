//! `minisnap-mcp` — the MCP stdio bridge to a running minisnap's agent
//! server. Speaks JSON-RPC 2.0 on stdin/stdout (MCP stdio framing:
//! newline-delimited messages), finds live instances through the
//! per-pid registry under `~/Library/Application Support/minisnap/instances/`
//! and forwards every tool call to the app's HTTP endpoints.
//!
//! Conventions borrowed from the qwe live-app setup: a dead app is a
//! short readable message, never a panic or a traceback; every answer
//! names the instance it came from; the bridge adds no behavior of its
//! own — it drives the same code paths the menus and hotkeys do.

use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};

use minisnap::agent_server::{instances_dir, InstanceRecord};

/// One discovered live instance: the registry record plus the port the
/// registry claims (the record is the truth; a connection proves it).
struct Instance {
    record: InstanceRecord,
}

impl Instance {
    fn label(&self) -> String {
        format!(
            "{} (pid {}, port {})",
            self.record.checkout, self.record.pid, self.record.port
        )
    }
}

/// How long one HTTP request may take before the bridge reports a stuck
/// app instead of hanging the agent.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Every step is an HTTP round trip with this timeout, so a pathological
/// batch would occupy the app's main thread and the agent for minutes.
const MAX_BATCH_STEPS: usize = 50;

fn main() {
    // stdout is the protocol; diagnostics go to stderr only.
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let incoming: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(e) => {
                let _ = write_response(json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}
                }));
                continue;
            }
        };
        if let Some(response) = handle_message(incoming) {
            let _ = write_response(response);
        }
    }
}

fn write_response(response: Value) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{response}")?;
    stdout.flush()
}

/// Handle one JSON-RPC message; `None` for notifications (no reply).
fn handle_message(incoming: Value) -> Option<Value> {
    let method = incoming.get("method").and_then(Value::as_str).unwrap_or("");
    let id = incoming.get("id").cloned();
    let is_notification = id.is_none() || id.as_ref() == Some(&Value::Null);
    match method {
        // Notifications: acknowledge nothing, answer nothing.
        "notifications/initialized" | "notifications/cancelled" => None,
        "initialize" => Some(reply(
            id,
            Ok(json!({
                "protocolVersion": incoming
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-06-18")),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "minisnap-mcp", "version": env!("CARGO_PKG_VERSION")},
            })),
        )),
        "ping" => Some(reply(id, Ok(json!({})))),
        "tools/list" => Some(reply(id, Ok(tools_list()))),
        "tools/call" => {
            let name = incoming
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = incoming
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call_tool(&name, &args);
            let (content, is_error) = match result {
                Ok(content) => (content, false),
                Err(message) => (vec![text_content(message)], true),
            };
            Some(reply(id, Ok(json!({"content": content, "isError": is_error}))))
        }
        other => {
            if is_notification {
                None
            } else {
                Some(reply(
                    id,
                    Err(json!({
                        "code": -32601,
                        "message": format!("method not found: {other}")
                    })),
                ))
            }
        }
    }
}

fn reply(id: Option<Value>, result: Result<Value, Value>) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }
}

fn text_content(text: impl Into<String>) -> Value {
    json!({"type": "text", "text": text.into()})
}

/// The tool catalog. `action` and `editor` take the app's own serde enums
/// verbatim — the same shapes the session journal uses.
fn tools_list() -> Value {
    json!({"tools": [
        {
            "name": "instances",
            "description": "List running minisnap instances (checkout label, pid, port, exe). Use it first when several checkouts are up; other tools default to the main checkout when several instances are alive.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "state",
            "description": "Full state of a running minisnap: open windows (editor/overlay/preferences with frames), the editor document (shot path, elements with geometry, selection, tool, color, stroke width, undo/redo), config (hotkeys, save dir).",
            "inputSchema": {
                "type": "object",
                "properties": {"instance": {"type": "string", "description": "checkout label or pid; default: the main checkout, or the only live instance"}}
            }
        },
        {
            "name": "screenshot",
            "description": "PNG without touching the screen: target=composite renders the editor document (deterministic colors), target=window draws the editor window content (toolbar + canvas) — works while the window is occluded or on another Space. Returns the image and the file path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": {"type": "string", "enum": ["composite", "window"]},
                    "path": {"type": "string", "description": "where to write the PNG; default under the temp dir"},
                    "instance": {"type": "string"}
                },
                "required": ["target"]
            }
        },
        {
            "name": "action",
            "description": "Run an app-level action through the same code paths as the tray menu and hotkeys. For several sequential actions prefer the `batch` tool. The `action` value is a serde enum: {\"Capture\":{\"edit\":true}}, {\"OpenLatest\"}, {\"OpenEditor\":{\"path\":\"…\"}}, \"CloseEditor\", {\"OverlaySelect\":{\"x\":0,\"y\":0,\"w\":800,\"h\":600}} (captured-frame pixels, finishes the frozen overlay like a drag: crop, shutter, clipboard, save), \"OverlayCancel\", \"PrefsOpen\", \"PrefsClose\", \"Quit\".",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "object", "description": "AgentAction enum shape, e.g. {\"Capture\":{\"edit\":false}} or \"CloseEditor\""},
                    "instance": {"type": "string"}
                },
                "required": ["action"]
            }
        },
        {
            "name": "editor",
            "description": "Operate on the open editor document (same paths as the toolbar and mouse handlers). For several sequential operations prefer the `batch` tool. The `op` value is a serde enum: {\"SetTool\":{\"tool\":\"arrow\"|\"rect\"|\"line\"|\"pen\"|\"text\"|\"callout\"|\"blur\"|\"crop\"|\"resize\"|null}}, {\"SetColor\":{\"rgba\":[1,0,0,1]}}, {\"SetWidth\":{\"width\":4.0}}, {\"Command\":{\"AddElement\":{\"id\":1,\"kind\":{\"Arrow\":{\"a\":[10,10],\"b\":[200,80],\"color\":[1,0,0,1],\"width\":4}}}}} (journal shape), \"Undo\", \"Redo\", {\"Select\":{\"id\":1}}, {\"Page\":{\"direction\":\"prev\"|\"next\"}}, {\"BeginText\":{\"target\":{\"new\":{\"x\":100,\"y\":80}},\"text\":\"WWWW\"}} (new text; target may also be {\"new_callout\":{\"from\":[x,y],\"to\":[x,y]}} — an arrow drag that opens the phase-two input at its base — or {\"existing\":1}; `text` is optional and lands in the field editor like typed text), {\"FinishText\":{\"commit\":true}} (commit like Enter; false cancels like Esc). Points are frame pixels, origin top-left.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {"type": "object", "description": "AgentEditorOp enum shape"},
                    "instance": {"type": "string"}
                },
                "required": ["op"]
            }
        },
        {
            "name": "batch",
            "description": "Execute several sequential steps against one instance in a single call — prefer batch over repeated action/editor calls whenever more than one step is needed. `steps` is an ordered list; each step is an object with exactly one key: {\"action\": <AgentAction shape>} or {\"editor\": <AgentEditorOp shape>} — the same shapes the action/editor tools take. Steps run in order through the same code paths; the answer reports every step with its number and outcome (done/initiated/rejected/noop/step error). App-level statuses do not stop the chain — a rejected step does not invalidate the rest; a transport error stops it and reports the executed steps. Reads (state, screenshot, log) are not steps, and polling does not batch: Capture only initiates the overlay — poll state between Capture and OverlaySelect. Max 50 steps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": {"type": "object"},
                        "description": "ordered steps; each is {\"action\": …} or {\"editor\": …}, values in the same serde enum shapes as the action/editor tools; max 50"
                    },
                    "instance": {"type": "string"}
                },
                "required": ["steps"]
            }
        },
        {
            "name": "log",
            "description": "Recent lines of the app's log ring buffer (state transitions, capture/output errors) with an offset to continue from.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "offset": {"type": "number", "description": "absolute line number to read from; default 0 (the newest tail)"},
                    "instance": {"type": "string"}
                }
            }
        }
    ]})
}

fn call_tool(name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "instances" => {
            let instances = discover()?;
            if instances.is_empty() {
                return Ok(vec![text_content(
                    "no live minisnap instance found; launch the app first \
                     (scripts/build_app.sh release && open target/release/minisnap.app)",
                )]);
            }
            let lines: Vec<String> = instances.iter().map(|i| i.label()).collect();
            Ok(vec![text_content(lines.join("\n"))])
        }
        "state" => {
            let instance = pick_instance(instance_arg(args))?;
            let response = http_json(instance.record.port, "GET", "/state", None)?;
            check_envelope_error(&response)?;
            Ok(vec![text_content(format!(
                "state of {}:\n{}",
                instance.label(),
                serde_json::to_string_pretty(&response).unwrap_or_default()
            ))])
        }
        "screenshot" => {
            let instance = pick_instance(instance_arg(args))?;
            let target = args
                .get("target")
                .and_then(Value::as_str)
                .ok_or("screenshot: missing 'target' (composite|window)")?;
            let path = args.get("path").and_then(Value::as_str);
            let mut query = format!("target={}", url_encode(target));
            if let Some(path) = path {
                query.push_str("&path=");
                query.push_str(&url_encode(path));
            }
            let response = http_json(instance.record.port, "GET", &format!("/screenshot?{query}"), None)?;
            check_envelope_error(&response)?;
            let screenshot = response
                .get("screenshot")
                .ok_or_else(|| format!("unexpected response: {response}"))?;
            let png_path = screenshot
                .get("path")
                .and_then(Value::as_str)
                .ok_or("screenshot: response has no path")?;
            let width = screenshot.get("width").and_then(Value::as_u64).unwrap_or(0);
            let height = screenshot.get("height").and_then(Value::as_u64).unwrap_or(0);
            let bytes = std::fs::read(png_path)
                .map_err(|e| format!("cannot read the written PNG {png_path}: {e}"))?;
            let mut content = vec![text_content(format!(
                "{} — {png_path} ({width}x{height})",
                instance.label()
            ))];
            content.push(json!({
                "type": "image",
                "data": base64_encode(&bytes),
                "mimeType": "image/png"
            }));
            Ok(content)
        }
        "action" => {
            let instance = pick_instance(instance_arg(args))?;
            let action = args.get("action").ok_or("action: missing 'action' value")?;
            let response = http_json(
                instance.record.port,
                "POST",
                "/action",
                Some(serde_json::to_string(action).map_err(|e| e.to_string())?.as_str()),
            )?;
            Ok(vec![text_content(format_result(&instance, &response))])
        }
        "editor" => {
            let instance = pick_instance(instance_arg(args))?;
            let op = args.get("op").ok_or("editor: missing 'op' value")?;
            let response = http_json(
                instance.record.port,
                "POST",
                "/editor",
                Some(serde_json::to_string(op).map_err(|e| e.to_string())?.as_str()),
            )?;
            Ok(vec![text_content(format_result(&instance, &response))])
        }
        "batch" => {
            let instance = pick_instance(instance_arg(args))?;
            let steps = args
                .get("steps")
                .and_then(Value::as_array)
                .ok_or("batch: missing 'steps' array")?;
            validate_batch_steps(steps).map_err(|e| format!("batch: {e}"))?;
            let mut report: Vec<String> = Vec::new();
            for (index, step) in steps.iter().enumerate() {
                let (key, value) = batch_step_parts(step).expect("steps validated before execution");
                let endpoint = if key == "action" { "/action" } else { "/editor" };
                let body = serde_json::to_string(value).map_err(|e| e.to_string())?;
                let response = match http_json(instance.record.port, "POST", endpoint, Some(&body)) {
                    Ok(response) => response,
                    Err(e) => {
                        // Nothing more to collect: name the instance, the
                        // step the chain died on, and what did execute.
                        let mut message = format!(
                            "{} — batch stopped before step {} (transport error: {e})",
                            instance.label(),
                            index + 1
                        );
                        if !report.is_empty() {
                            message.push_str("; executed steps:\n");
                            message.push_str(&report.join("\n"));
                        }
                        return Err(message);
                    }
                };
                report.push(format!(
                    "step {} — {}",
                    index + 1,
                    describe_step_result(&response)
                ));
            }
            Ok(vec![text_content(format!(
                "{} — batch of {} steps:\n{}",
                instance.label(),
                steps.len(),
                report.join("\n")
            ))])
        }
        "log" => {
            let instance = pick_instance(instance_arg(args))?;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let response = http_json(instance.record.port, "GET", &format!("/log?offset={offset}"), None)?;
            Ok(vec![text_content(format!(
                "log of {}:\n{}",
                instance.label(),
                serde_json::to_string_pretty(&response).unwrap_or_default()
            ))])
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

/// Surface the app's own error (an envelope with `"error"`) as a tool
/// error, so an agent sees the app's message instead of "unexpected
/// response".
fn check_envelope_error(response: &Value) -> Result<(), String> {
    match response.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

/// Pull `"result": {"status": …, "detail": …}` (or the error) out of an
/// envelope and phrase it with the instance label.
fn format_result(instance: &Instance, response: &Value) -> String {
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        return format!("{} — error: {error}", instance.label());
    }
    match response.get("result") {
        Some(result) => format!(
            "{} — {}",
            instance.label(),
            serde_json::to_string_pretty(result).unwrap_or_default()
        ),
        None => format!("{} — unexpected response: {response}", instance.label()),
    }
}

/// Validate the step list before anything executes: not empty, within the
/// limit, every step shaped as one `action`/`editor` key — errors name the
/// step number.
fn validate_batch_steps(steps: &[Value]) -> Result<(), String> {
    if steps.is_empty() {
        return Err("empty 'steps'; nothing to execute".into());
    }
    if steps.len() > MAX_BATCH_STEPS {
        return Err(format!(
            "{} steps exceed the limit of {MAX_BATCH_STEPS}; split the chain into several batch calls",
            steps.len()
        ));
    }
    for (index, step) in steps.iter().enumerate() {
        batch_step_parts(step).map_err(|e| format!("step {}: {e}", index + 1))?;
    }
    Ok(())
}

/// Split one batch step: exactly one of the keys `action`/`editor`, and
/// its value goes to the matching endpoint verbatim — the same shapes the
/// single tools take.
fn batch_step_parts(step: &Value) -> Result<(&'static str, &Value), String> {
    let object = step.as_object().ok_or_else(|| {
        format!("expected an object with one of the keys \"action\"/\"editor\", got {step}")
    })?;
    let (key, value) = match (object.get("action"), object.get("editor")) {
        (Some(_), Some(_)) => {
            return Err("has both \"action\" and \"editor\" keys; one per step".into())
        }
        (Some(value), None) => ("action", value),
        (None, Some(value)) => ("editor", value),
        (None, None) => {
            return Err(format!("has neither \"action\" nor \"editor\" key: {step}"));
        }
    };
    if object.len() > 1 {
        let extra: Vec<&str> = object
            .keys()
            .map(String::as_str)
            .filter(|key| *key != "action" && *key != "editor")
            .collect();
        return Err(format!("unexpected extra key(s): {}", extra.join(", ")));
    }
    Ok((key, value))
}

/// One report line for a step's answer: the app's status/detail, the app's
/// error, or the raw envelope when it is neither.
fn describe_step_result(response: &Value) -> String {
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        return format!("error: {error}");
    }
    match response.get("result") {
        Some(result) => result.to_string(),
        None => format!("unexpected response: {response}"),
    }
}

fn instance_arg(args: &Value) -> Option<String> {
    args.get("instance")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Live instances from the registry: entries whose port answers a ping.
fn discover() -> Result<Vec<Instance>, String> {
    let Some(dir) = instances_dir() else {
        return Err("cannot resolve the instances registry directory".into());
    };
    let mut instances = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(instances);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<InstanceRecord>(&text) else {
            // A broken record is a registry wart, not a tool error.
            continue;
        };
        match http_json(record.port, "GET", "/ping", None) {
            // The identity must match the record: a port can outlive its
            // instance and land on a different one.
            Ok(response)
                if response.get("error").is_none()
                    && response.pointer("/identity/pid").and_then(Value::as_u64)
                        == Some(u64::from(record.pid)) =>
            {
                instances.push(Instance { record })
            }
            // Dead, foreign, or a port that moved on: prune the stale
            // record so the registry self-heals.
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(instances)
}

/// Choose the instance for a tool call: an explicit label/pid wins, a
/// single live instance is taken as is, several fall back to the main
/// checkout with the rest named in the answer.
fn pick_instance(selector: Option<String>) -> Result<Instance, String> {
    let mut instances = discover()?;
    if instances.is_empty() {
        return Err(
            "no live minisnap instance found; launch the app first (scripts/build_app.sh release && open target/release/minisnap.app)"
                .into(),
        );
    }
    if let Some(selector) = selector {
        let labels: Vec<String> = instances.iter().map(|i| i.label()).collect();
        return instances
            .into_iter()
            .find(|instance| {
                instance.record.checkout == selector
                    || instance.record.pid.to_string() == selector
            })
            .ok_or_else(|| format!("no instance matches '{selector}'; live: {}", labels.join(", ")));
    }
    if instances.len() == 1 {
        return Ok(instances.pop().expect("len checked"));
    }
    // Several live: the main checkout is the default; the agent sees the
    // rest in the answer (and can pass `instance` to reach them).
    let main = instances
        .iter()
        .position(|instance| instance.record.checkout == "main")
        .unwrap_or(0);
    let chosen = instances.remove(main);
    let others: Vec<String> = instances.iter().map(|i| i.label()).collect();
    eprintln!(
        "minisnap-mcp: several instances live, defaulting to {} (others: {})",
        chosen.label(),
        others.join(", ")
    );
    Ok(chosen)
}

/// One short HTTP request over a fresh connection; the JSON body parsed.
fn http_json(port: u16, method: &str, path_qs: &str, body: Option<&str>) -> Result<Value, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("cannot connect to 127.0.0.1:{port}: {e}"))?;
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path_qs} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use std::io::Read;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(&text);
    // The body is the JSON envelope; a non-2xx status surfaces through the
    // envelope's error field, so the status line itself is not parsed.
    serde_json::from_str(body).map_err(|e| {
        format!(
            "bad JSON from 127.0.0.1:{port}: {e}; body: {}",
            &body[..body.len().min(200)]
        )
    })
}

/// Percent-encode everything outside the unreserved set (query values).
fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let triple = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(triple >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[triple as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(url_encode("/tmp/a b.png"), "/tmp/a%20b.png");
        assert_eq!(url_encode("plain.png"), "plain.png");
    }

    #[test]
    fn tools_list_has_seven_tools_and_advertises_batch() {
        let list = tools_list();
        let tools = list.pointer("/tools").and_then(Value::as_array).unwrap();
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"batch"), "tools: {names:?}");
        for name in ["action", "editor"] {
            let description = tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|tool| tool.get("description").and_then(Value::as_str))
                .expect("tool present");
            assert!(
                description.contains("batch"),
                "{name} description must point at batch"
            );
        }
    }

    #[test]
    fn batch_step_parts_accepts_exactly_one_key() {
        let action_step = json!({"action": "CloseEditor"});
        let (endpoint, value) = batch_step_parts(&action_step).unwrap();
        assert_eq!(endpoint, "action");
        assert_eq!(value, &json!("CloseEditor"));
        let editor_step = json!({"editor": "Undo"});
        let (endpoint, value) = batch_step_parts(&editor_step).unwrap();
        assert_eq!(endpoint, "editor");
        assert_eq!(value, &json!("Undo"));
    }

    #[test]
    fn batch_step_parts_rejects_wrong_shapes() {
        for step in [
            json!({"action": "CloseEditor", "editor": "Undo"}),
            json!({"tool": "arrow"}),
            json!("CloseEditor"),
            json!({"action": "CloseEditor", "instance": "main"}),
        ] {
            assert!(batch_step_parts(&step).is_err(), "must reject {step}");
        }
    }

    #[test]
    fn batch_validation_enforces_limit_and_names_the_bad_step() {
        assert!(validate_batch_steps(&[]).is_err());
        let many: Vec<Value> = (0..=MAX_BATCH_STEPS)
            .map(|_| json!({"action": "CloseEditor"}))
            .collect();
        let error = validate_batch_steps(&many).unwrap_err();
        assert!(error.contains("50"), "limit named: {error}");
        let steps = vec![json!({"action": "CloseEditor"}), json!({"bogus": 1})];
        let error = validate_batch_steps(&steps).unwrap_err();
        assert!(error.contains("step 2"), "bad step named: {error}");
    }
}
