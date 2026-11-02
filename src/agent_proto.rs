//! Wire types of the agent protocol, shared by the in-app agent server and
//! the `minisnap-mcp` bridge: serde JSON on both sides. The app-side
//! documents (`Element`, `Command`) serialize in the same shape as the
//! session journal, so an agent reads the live document in journal terms.

use serde::{Deserialize, Serialize};

use crate::editor::{Command, Element, Rgba};

/// Who answered: every agent-server response carries this, so an agent
/// never mistakes one running instance for another (the port is not
/// identity — a stale client of a dead instance could hit a second copy).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub pid: u32,
    /// Canonical path of the running executable.
    pub exe: String,
    /// Checkout label: branch name for a worktree build, "main" otherwise.
    pub checkout: String,
    pub port: u16,
}

/// One of the app's own windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowInfo {
    /// "editor" | "preferences".
    pub title: String,
    /// Screen frame: origin top-left, size in points.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub visible: bool,
}

/// What the agent needs to know about the config.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigSummary {
    pub capture_hotkey: String,
    pub capture_edit_hotkey: String,
    pub open_hotkey: String,
    pub save_dir: String,
    pub retention: u32,
}

/// The open editor window and its document; `None` while the editor is
/// closed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorState {
    pub shot_path: String,
    pub file_label: String,
    /// Active tool, or `None` for viewing mode. One of "arrow", "rect",
    /// "line", "pen", "text", "callout", "blur", "crop", "resize".
    pub tool: Option<String>,
    /// Color for new elements, `[r, g, b, a]` in 0..1 floats.
    pub color: Rgba,
    /// Stroke width for new elements.
    pub stroke_width: f64,
    pub selection: Option<u64>,
    /// A text input is open (new text, callout phase two, or an edit).
    pub text_input_active: bool,
    /// The font size the slider rides while the input is open.
    pub text_size: Option<f64>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_page_prev: bool,
    pub can_page_next: bool,
    /// Base frame size in image pixels.
    pub image_width: u32,
    pub image_height: u32,
    /// The document: elements in journal shape (id, kind, geometry, color).
    pub elements: Vec<Element>,
}

/// Full application state: what is open, what the document holds, what the
/// config says.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppState {
    pub editor: Option<EditorState>,
    /// The frozen-frame overlay (capture selection) is up.
    pub overlay_active: bool,
    pub prefs_open: bool,
    pub windows: Vec<WindowInfo>,
    pub config: ConfigSummary,
}

/// Actions of the whole app, mirroring the tray menu and hotkeys.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentAction {
    /// Start the frozen-area capture; `edit` opens the editor afterwards.
    Capture { edit: bool },
    /// Open the newest saved shot in the editor (silent no-op when the
    /// folder is empty).
    OpenLatest,
    /// Open a specific image file in the editor.
    OpenEditor { path: String },
    /// Hide the editor window (the document stays reusable, like closing
    /// it by hand).
    CloseEditor,
    /// Finish the active overlay as a selection of the given rect — the
    /// same outcome path as a user drag. The rect is in the captured
    /// frame's pixel space of the target display (origin top-left, the
    /// same space the composite screenshot reports).
    OverlaySelect { x: f64, y: f64, w: f64, h: f64 },
    /// Cancel the active overlay without producing a shot.
    OverlayCancel,
    /// Open (or focus) the preferences window.
    PrefsOpen,
    /// Close the preferences window.
    PrefsClose,
    /// Terminate the application.
    Quit,
}

/// Operations on the open editor document, mirroring the toolbar and the
/// mouse handlers. All of them go through `OpenShot::apply` and the same
/// sync path as user actions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentEditorOp {
    /// Select a tool (as a toolbar click does) or deselect with `None`.
    SetTool { tool: Option<String> },
    /// Set the color for new elements; a selected element is recolored,
    /// exactly like picking from the palette.
    SetColor { rgba: Rgba },
    /// Set the stroke width: for new elements, or the selected element
    /// when it carries a stroke (the slider rule).
    SetWidth { width: f64 },
    /// Apply one journal command in journal shape, e.g.
    /// `{"AddElement":{"id":1,"kind":{"Arrow":{...}}}}` or
    /// `{"DeleteElement":1}` — the same enum the session journal stores.
    Command { command: Command },
    Undo,
    Redo,
    /// Select an element (click on the canvas), or drop the selection.
    Select { id: Option<u64> },
    /// Leaf to the previous ("prev") or next ("next") shot of the folder.
    Page { direction: String },
}

/// What an action or editor operation did. `status` is one of "done",
/// "initiated" (started async work — the capture freeze), "rejected"
/// (the current app state forbids it), "noop" (nothing to do).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionResult {
    pub status: String,
    pub detail: String,
}

impl ActionResult {
    pub fn done(detail: impl Into<String>) -> ActionResult {
        ActionResult { status: "done".into(), detail: detail.into() }
    }
    pub fn initiated(detail: impl Into<String>) -> ActionResult {
        ActionResult { status: "initiated".into(), detail: detail.into() }
    }
    pub fn rejected(detail: impl Into<String>) -> ActionResult {
        ActionResult { status: "rejected".into(), detail: detail.into() }
    }
    pub fn noop(detail: impl Into<String>) -> ActionResult {
        ActionResult { status: "noop".into(), detail: detail.into() }
    }
}

/// Result of `GET /screenshot`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub target: String,
    /// The PNG written on disk.
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// Result of `GET /log`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogResult {
    pub lines: Vec<String>,
    /// Absolute number of the next line: pass it as the next `offset`.
    pub next_offset: u64,
}

/// What the main thread produced for one agent request; the HTTP layer
/// wraps it with the instance identity. Untagged: the payload serializes
/// bare (the envelope's key already names the kind), and the app only
/// ever serializes this enum — nothing deserializes it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentResponse {
    State(AppState),
    Result(ActionResult),
    Screenshot(ScreenshotResult),
    Log(LogResult),
}

/// Parse a tool name into the editor's `Tool`; the inverse of the `tool`
/// field in `EditorState`.
pub fn parse_tool(name: &str) -> Option<crate::editor::Tool> {
    use crate::editor::Tool::*;
    Some(match name {
        "arrow" => Arrow,
        "rect" => Rect,
        "line" => Line,
        "pen" => Pen,
        "text" => Text,
        "callout" => Callout,
        "blur" => Blur,
        "crop" => Crop,
        "resize" => Resize,
        _ => return None,
    })
}

/// The name of a tool, as `EditorState.tool` reports it.
pub fn tool_name(tool: crate::editor::Tool) -> &'static str {
    use crate::editor::Tool::*;
    match tool {
        Arrow => "arrow",
        Rect => "rect",
        Line => "line",
        Pen => "pen",
        Text => "text",
        Callout => "callout",
        Blur => "blur",
        Crop => "crop",
        Resize => "resize",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A journal command round-trips through the agent protocol unchanged:
    /// the MCP and the app agree on the journal shape.
    #[test]
    fn command_roundtrip() {
        let op = AgentEditorOp::Command {
            command: Command::AddElement(crate::editor::Element {
                id: 7,
                kind: crate::editor::ElementKind::Arrow {
                    a: crate::editor::Point::new(1.0, 2.0),
                    b: crate::editor::Point::new(30.0, 40.0),
                    color: [1.0, 0.0, 0.0, 1.0],
                    width: 4.0,
                },
            }),
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: AgentEditorOp = serde_json::from_str(&json).unwrap();
        assert_eq!(json, serde_json::to_string(&back).unwrap());
        assert!(json.contains("AddElement"));
    }

    #[test]
    fn tool_names_roundtrip() {
        use crate::editor::Tool::*;
        for tool in [Arrow, Rect, Line, Pen, Text, Callout, Blur, Crop, Resize] {
            assert_eq!(parse_tool(tool_name(tool)), Some(tool));
        }
        assert_eq!(parse_tool("nonsense"), None);
    }
}
