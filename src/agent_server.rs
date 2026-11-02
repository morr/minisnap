//! The agent server: a small HTTP listener on `127.0.0.1` that forwards
//! requests to the main thread through the `MainJob` pump and answers with
//! an identity-stamped envelope. This is minisnap's control channel for
//! agents — the analog of Bevy's Remote Protocol — replacing the ad-hoc
//! AppleScript/CGEvent/screencapture tooling with in-app calls: every
//! request executes the same code paths the menus and mouse handlers do.
//!
//! Threading: the listener owns one thread; the main thread owns all
//! state. A request parsed here travels as `MainJobAgent`, the `tick:`
//! pump hands it to the controller, the reply comes back over an mpsc
//! pair with a timeout — a stuck main thread degrades to per-request
//! timeouts, not a wedged listener.

use std::collections::HashMap;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agent_proto::{AgentAction, AgentEditorOp, AgentIdentity, AgentResponse};
use crate::config::Config;

/// The port the agent server listens on unless the config or the
/// `MINISNAP_AGENT_PORT` override says otherwise. Deliberately outside the
/// qwe BRP range (15702…).
pub const DEFAULT_AGENT_PORT: u16 = 15790;

/// How long a server-thread request waits for the main thread before
/// answering with a timeout error.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the HTTP layer can ask the main thread to do; the request
/// shape mirrors the endpoints one to one.
#[derive(Clone, Debug)]
pub enum AgentRequest {
    State,
    Action(AgentAction),
    Editor(AgentEditorOp),
    Screenshot { target: String, path: Option<String> },
    Log { offset: u64 },
}

/// One request to run, answered by the main thread.
pub struct MainJobAgent {
    pub request: AgentRequest,
    pub reply: mpsc::Sender<Result<AgentResponse, String>>,
}

/// The per-instance registry entry: one file per process, so instances
/// never merge-write the registry and a dead process leaves a stale file
/// the reader prunes by pid.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub pid: u32,
    pub port: u16,
    pub exe: String,
    pub checkout: String,
    /// Unix seconds at server start.
    pub started_at: u64,
}

/// Directory holding `<pid>.json` records, next to the config.
pub fn instances_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("minisnap").join("instances"))
}

fn record_path(pid: u32) -> Option<PathBuf> {
    instances_dir().map(|dir| dir.join(format!("{pid}.json")))
}

/// Write this instance's registry record; failure is logged, not fatal —
/// the server works without the registry, the MCP just won't discover it.
fn register(record: &InstanceRecord) {
    let Some(path) = record_path(record.pid) else {
        crate::log!("agent server: cannot resolve registry directory");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log!("agent server: cannot create registry directory: {e}");
            return;
        }
    }
    let json = match serde_json::to_string_pretty(record) {
        Ok(json) => json,
        Err(e) => {
            crate::log!("agent server: cannot serialize registry record: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, json) {
        crate::log!("agent server: cannot write registry record: {e}");
    }
}

/// Remove this instance's registry record (graceful quit).
pub fn deregister(pid: u32) {
    if let Some(path) = record_path(pid) {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                crate::log!("agent server: cannot remove registry record: {e}");
            }
        }
    }
}

/// The port the server listens on: `MINISNAP_AGENT_PORT`, then the config
/// override, then the module default.
pub fn resolve_port(config: &Config) -> u16 {
    if let Ok(port) = std::env::var("MINISNAP_AGENT_PORT") {
        if let Ok(port) = port.parse::<u16>() {
            return port;
        }
        crate::log!("agent server: bad MINISNAP_AGENT_PORT '{port}', falling back");
    }
    config.agent_server_port.unwrap_or(DEFAULT_AGENT_PORT)
}

/// Raise the server on a background thread. Returns without a thread when
/// the server is disabled in the config or the port is taken — the app
/// keeps running either way, the failure is logged loudly instead (a
/// silent bind failure is how a client ends up talking to the wrong
/// process). Requests travel as `MainJob::Agent` on the app's existing
/// job channel.
pub fn start(config: &Config, mut identity: AgentIdentity, jobs_tx: mpsc::Sender<crate::app::MainJob>) {
    if !config.agent_server_enabled {
        crate::log!("agent server: disabled by config");
        return;
    }
    let port = resolve_port(config);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(e) => {
            crate::log!(
                "agent server: cannot bind 127.0.0.1:{port} ({e}); running without the agent server"
            );
            return;
        }
    };
    let record = InstanceRecord {
        pid: identity.pid,
        port,
        exe: identity.exe.clone(),
        checkout: identity.checkout.clone(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    register(&record);
    identity.port = port;
    crate::log!("agent server: listening on 127.0.0.1:{port}");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            // One request at a time: the agent is a single caller, and
            // serial handling keeps the main-thread pump predictable.
            handle_connection(stream, &identity, &jobs_tx);
        }
    });
}

fn handle_connection(
    mut stream: TcpStream,
    identity: &AgentIdentity,
    jobs_tx: &mpsc::Sender<crate::app::MainJob>,
) {
    let (status, body) = match serve_one(&mut stream, jobs_tx) {
        Ok((payload_key, payload)) => (
            "200 OK".to_string(),
            serde_json::json!({
                "identity": identity,
                payload_key: payload,
            }),
        ),
        Err((status, message)) => (
            status,
            serde_json::json!({ "identity": identity, "error": message }),
        ),
    };
    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    use std::io::Write;
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// Parse and dispatch one request; `Ok` is a successful payload, `Err` is
/// an HTTP status plus an error message.
fn serve_one(
    stream: &mut TcpStream,
    jobs_tx: &mpsc::Sender<crate::app::MainJob>,
) -> Result<(&'static str, AgentResponse), (String, String)> {
    let (head, body) = read_request(stream)?;
    let request_line = head.lines().next().unwrap_or_default().to_string();
    let (method, path, query) = split_request_line(&request_line)?;

    let request = match (method.as_str(), path.as_str()) {
        ("GET", "/state") => AgentRequest::State,
        ("GET", "/screenshot") => AgentRequest::Screenshot {
            target: query
                .get("target")
                .cloned()
                .ok_or_else(|| ("400 Bad Request".into(), "missing ?target=".to_string()))?,
            path: query.get("path").cloned(),
        },
        ("GET", "/log") => AgentRequest::Log {
            offset: query
                .get("offset")
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|_| ("400 Bad Request".to_string(), "bad ?offset=".to_string()))?
                .unwrap_or(0),
        },
        ("POST", "/action") => {
            let action: AgentAction = serde_json::from_slice(&body)
                .map_err(|e| ("400 Bad Request".to_string(), format!("bad action JSON: {e}")))?;
            AgentRequest::Action(action)
        }
        ("POST", "/editor") => {
            let op: AgentEditorOp = serde_json::from_slice(&body)
                .map_err(|e| ("400 Bad Request".to_string(), format!("bad editor JSON: {e}")))?;
            AgentRequest::Editor(op)
        }
        _ => {
            return Err((
                "404 Not Found".to_string(),
                format!("no route {method} {path}"),
            ))
        }
    };

    let (reply_tx, reply_rx) = mpsc::channel();
    if jobs_tx
        .send(crate::app::MainJob::Agent(MainJobAgent {
            request,
            reply: reply_tx,
        }))
        .is_err()
    {
        return Err((
            "503 Service Unavailable".to_string(),
            "main thread is gone".to_string(),
        ));
    }
    match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(response)) => Ok((payload_key(&response), response)),
        Ok(Err(message)) => Err(("409 Conflict".to_string(), message)),
        Err(_) => Err((
            "504 Gateway Timeout".to_string(),
            "main thread did not answer in time".to_string(),
        )),
    }
}

fn payload_key(response: &AgentResponse) -> &'static str {
    match response {
        AgentResponse::State(_) => "state",
        AgentResponse::Result(_) => "result",
        AgentResponse::Screenshot(_) => "screenshot",
        AgentResponse::Log(_) => "log",
    }
}

/// Read one request: the head up to the blank line plus the body per
/// Content-Length (empty when absent).
fn read_request(stream: &mut TcpStream) -> Result<(String, Vec<u8>), (String, String)> {
    const MAX_HEAD: usize = 64 * 1024;
    const MAX_BODY: usize = 1024 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            return Err(("400 Bad Request".into(), "request head too large".into()));
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(("400 Bad Request".into(), "connection closed mid-request".into())),
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(e) => return Err(("400 Bad Request".into(), format!("read: {e}"))),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let content_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().to_string())
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY {
        return Err(("400 Bad Request".into(), "request body too large".into()));
    }
    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(("400 Bad Request".into(), "connection closed mid-body".into())),
            Ok(read) => body.extend_from_slice(&chunk[..read]),
            Err(e) => return Err(("400 Bad Request".into(), format!("read body: {e}"))),
        }
    }
    body.truncate(content_length);
    Ok((head, body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Split a request line into method, path and decoded query pairs.
fn split_request_line(
    line: &str,
) -> Result<(String, String, HashMap<String, String>), (String, String)> {
    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ("400 Bad Request".to_string(), "empty request line".to_string()))?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| ("400 Bad Request".to_string(), "no path in request line".to_string()))?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let mut pairs = HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        pairs.insert(percent_decode(key), percent_decode(value));
    }
    Ok((method, path.to_string(), pairs))
}

/// Minimal percent-decoding for query values (open-editor paths arrive in
/// POST JSON bodies; only `?path=` needs this).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_splits_query_pairs() {
        let (method, path, query) =
            split_request_line("GET /screenshot?target=window&path=/tmp/a%20b.png HTTP/1.1")
                .unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/screenshot");
        assert_eq!(query.get("target").map(String::as_str), Some("window"));
        assert_eq!(query.get("path").map(String::as_str), Some("/tmp/a b.png"));
    }

    #[test]
    fn percent_decode_passes_plain_text() {
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
