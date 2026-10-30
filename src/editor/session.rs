//! Edit sessions on disk: `<root>/<shot file stem>/` holds the pristine
//! frame (`original.png`) plus the applied command journal
//! (`commands.json`), so the edit history outlives the editor window —
//! until the OS clears the folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_core_foundation::CFURL;
use objc2_core_graphics::CGImage;
use objc2_image_io::CGImageSource;

use super::render;
use super::{Command, EditorDoc};

/// An open shot being edited: the document plus everything needed to
/// persist it. Every applied command (and undo/redo) immediately rewrites
/// the shot's PNG file and the session journal.
pub struct OpenShot {
    pub doc: EditorDoc,
    /// The shot's PNG file in the save folder.
    pub shot_path: PathBuf,
    sessions_root: PathBuf,
}

impl OpenShot {
    /// Open a shot file for editing. With a live session — the original
    /// frame plus the replayed journal; without one — the file itself
    /// becomes the original of a fresh session.
    pub fn open(shot_path: PathBuf, sessions_root: &Path) -> Option<OpenShot> {
        let (doc, fresh) = match load(sessions_root, &shot_path) {
            Some((original, commands)) => {
                let mut doc = EditorDoc::new(original);
                for cmd in commands {
                    doc.apply(cmd);
                }
                (doc, false)
            }
            None => {
                let original = decode_file(&shot_path)?;
                (EditorDoc::new(original), true)
            }
        };
        if fresh {
            if let Err(e) = store_original(sessions_root, &shot_path, doc.original()) {
                eprintln!("session: cannot store original: {e}");
            }
        }
        Some(OpenShot {
            doc,
            shot_path,
            sessions_root: sessions_root.to_path_buf(),
        })
    }

    /// The shot's file name (the window title).
    pub fn file_name(&self) -> String {
        self.shot_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Apply a command and auto-save.
    pub fn apply(&mut self, cmd: Command) -> Result<(), String> {
        self.doc.apply(cmd);
        self.persist()
    }

    /// Apply a stroke-width change (consecutive slider events coalesce in
    /// the journal) and auto-save.
    pub fn apply_width(&mut self, id: u64, width: f64) -> Result<(), String> {
        self.doc.apply_width(id, width);
        self.persist()
    }

    /// Undo the last command and auto-save. False when nothing to undo.
    pub fn undo(&mut self) -> Result<bool, String> {
        if !self.doc.undo() {
            return Ok(false);
        }
        self.persist().map(|()| true)
    }

    /// Redo the next retracted command and auto-save.
    pub fn redo(&mut self) -> Result<bool, String> {
        if !self.doc.redo() {
            return Ok(false);
        }
        self.persist().map(|()| true)
    }

    /// Render the current state and write it to the shot's PNG file plus the
    /// session journal.
    fn persist(&self) -> Result<(), String> {
        let composite = render::render_composite(self.doc.base(), self.doc.elements())
            .ok_or("composite render failed")?;
        crate::output::encode_png_atomic(&composite, &self.shot_path)?;
        store_commands(&self.sessions_root, &self.shot_path, self.doc.applied_log())
            .map_err(|e| format!("session save: {e}"))
    }
}

/// At most this many session directories survive on disk.
pub const MAX_SESSIONS: usize = 50;

/// The default sessions root (cleared by the OS on reboot — the degraded
/// mode is "no history", not a broken editor).
pub fn default_root() -> PathBuf {
    PathBuf::from("/tmp/.minisnap")
}

/// Session directory for a shot file: root + file name without `.png`.
fn session_dir(root: &Path, shot_path: &Path) -> PathBuf {
    let stem = shot_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shot".into());
    root.join(stem)
}

/// Load the session for `shot_path`: the original frame and the applied
/// commands. A missing or unreadable session reads as "no history".
pub fn load(root: &Path, shot_path: &Path) -> Option<(Retained<CGImage>, Vec<Command>)> {
    let dir = session_dir(root, shot_path);
    let image = decode_file(&dir.join("original.png"))?;
    let text = match fs::read_to_string(dir.join("commands.json")) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("session: no commands for {}: {e}", shot_path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(commands) => Some((image, commands)),
        Err(e) => {
            eprintln!("session: bad commands.json for {}: {e}", shot_path.display());
            None
        }
    }
}

/// Persist the session for `shot_path`: the pristine frame and the applied
/// command journal (rewritten whole after every edit). Runs retention.
pub fn save(
    root: &Path,
    shot_path: &Path,
    original: &CGImage,
    commands: &[Command],
) -> io::Result<()> {
    store_original(root, shot_path, original)?;
    store_commands(root, shot_path, commands)
}

/// Store the pristine frame of a session (once per session, at open).
pub fn store_original(root: &Path, shot_path: &Path, original: &CGImage) -> io::Result<()> {
    let dir = session_dir(root, shot_path);
    fs::create_dir_all(&dir)?;
    write_atomic(&dir.join("original.tmp"), &dir.join("original.png"), |path| {
        crate::output::encode_png(original, path).map_err(io::Error::other)
    })
}

/// Rewrite the command journal of a session and run retention.
pub fn store_commands(root: &Path, shot_path: &Path, commands: &[Command]) -> io::Result<()> {
    let dir = session_dir(root, shot_path);
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(commands)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&dir.join("commands.tmp"), &dir.join("commands.json"), |path| {
        fs::write(path, json)
    })?;
    enforce_retention(root, MAX_SESSIONS);
    Ok(())
}

/// Write via a temporary file and rename, so a crash mid-write never leaves
/// a half-written file behind.
fn write_atomic(
    tmp: &Path,
    target: &Path,
    write: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    write(tmp)?;
    fs::rename(tmp, target)
}

/// Keep at most `limit` session directories, deleting the oldest by mtime.
/// Best-effort: a directory that cannot be removed is logged and skipped.
pub fn enforce_retention(root: &Path, limit: usize) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), modified))
        })
        .collect();
    dirs.sort_by_key(|(_, modified)| *modified);
    let excess = dirs.len().saturating_sub(limit);
    for (path, _) in dirs.into_iter().take(excess) {
        if let Err(e) = fs::remove_dir_all(&path) {
            eprintln!("session retention: cannot remove {}: {e}", path.display());
        } else {
            eprintln!("session retention: removed {}", path.display());
        }
    }
}

/// Decode a PNG file to a CGImage.
pub(crate) fn decode_file(path: &Path) -> Option<Retained<CGImage>> {
    let url = CFURL::from_file_path(path)?;
    // SAFETY: url is a live CF object; no read options needed.
    let source = unsafe { CGImageSource::with_url(&url, None) }?;
    // SAFETY: plain decode of the first image of a live source.
    let image = unsafe { source.image_at_index(0, None) }?;
    Some(image.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::tests::solid_frame;
    use crate::editor::{Element, ElementKind, Point};
    use std::time::{Duration, SystemTime};

    fn tmp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minisnap-session-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_commands() -> Vec<Command> {
        vec![
            Command::AddElement(Element {
                id: 1,
                kind: ElementKind::Arrow {
                    a: Point::new(5.0, 5.0),
                    b: Point::new(20.0, 25.0),
                    color: [1.0, 0.0, 1.0, 1.0],
                    width: 3.0,
                },
            }),
            Command::MoveElement(1, Point::new(2.0, -1.0)),
            Command::EditText(1, "правка".into()),
            Command::SetWidth(1, 6.5),
        ]
    }

    #[test]
    fn round_trips_original_and_journal() {
        let root = tmp_root("roundtrip");
        let shot = Path::new("MiniSnap_2026-10-29_12-00-00.png");
        let frame = solid_frame(32, 32);
        let commands = sample_commands();
        save(&root, shot, &frame, &commands).expect("save");

        let (image, loaded) = load(&root, shot).expect("session exists");
        assert_eq!(CGImage::width(Some(&image)), 32);
        assert_eq!(CGImage::height(Some(&image)), 32);
        assert_eq!(loaded, commands);
    }

    #[test]
    fn missing_session_reads_as_no_history() {
        let root = tmp_root("missing");
        let shot = Path::new("MiniSnap_2026-10-29_12-00-01.png");
        assert!(load(&root, shot).is_none());
        // A corrupted journal also degrades to "no history" instead of
        // poisoning the editor.
        let dir = session_dir(&root, shot);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("commands.json"), "not json").unwrap();
        assert!(load(&root, shot).is_none());
    }

    #[test]
    fn edit_persists_and_reopening_restores_history() {
        use crate::editor::{Element, ElementKind};

        let root = tmp_root("openshot");
        let shot = root.join("MiniSnap_2026-10-29_12-00-00.png");
        let frame = solid_frame(32, 32);
        crate::output::encode_png(&frame, &shot).expect("write the shot file");

        // First open: no session, the file itself is the original.
        let mut open = OpenShot::open(shot.clone(), &root).expect("open");
        assert!(!open.doc.can_undo());
        assert_eq!(open.file_name(), "MiniSnap_2026-10-29_12-00-00.png");

        // One edit: the file on disk updates immediately.
        open.apply(Command::AddElement(Element {
            id: 1,
            kind: ElementKind::Arrow {
                a: Point::new(2.0, 2.0),
                b: Point::new(28.0, 28.0),
                color: [1.0, 0.0, 1.0, 1.0],
                width: 4.0,
            },
        }))
        .expect("apply");
        assert!(!open.doc.can_redo());
        drop(open);

        // "Close and reopen": the history comes back from the session.
        let mut reopened = OpenShot::open(shot.clone(), &root).expect("reopen");
        assert!(reopened.doc.can_undo(), "history restored");
        assert_eq!(reopened.doc.applied_log().len(), 1);

        // The file on disk shows the arrow: some pixel differs from the
        // solid gray original.
        let edited = decode_file(&shot).expect("decode edited file");
        let bytes = crate::editor::tests::pixel_bytes(&edited);
        assert!(bytes.chunks_exact(4).any(|px| px[0] > 200 && px[2] > 200));

        // Undo: the file reverts to the pristine frame.
        reopened.undo().expect("undo").then_some(()).expect("undo applied");
        let reverted = decode_file(&shot).expect("decode reverted file");
        let bytes = crate::editor::tests::pixel_bytes(&reverted);
        assert!(bytes.iter().all(|&v| v == 128 || v == 255), "back to solid gray");
    }

    #[test]
    fn retention_keeps_the_newest_directories() {
        let root = tmp_root("retention");
        let extra = 2;
        // Sequential creation with a small pause: strictly increasing dir
        // mtimes (std cannot set a directory's mtime directly).
        for i in 0..(MAX_SESSIONS + extra) {
            fs::create_dir_all(root.join(format!("MiniSnap_2026-10-29_12-{i:02}-00"))).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        enforce_retention(&root, MAX_SESSIONS);
        let left = fs::read_dir(&root).unwrap().count();
        assert_eq!(left, MAX_SESSIONS);
        assert!(!root.join("MiniSnap_2026-10-29_12-00-00").exists(), "oldest gone");
        assert!(!root.join("MiniSnap_2026-10-29_12-01-00").exists(), "second oldest gone");
        assert!(root
            .join(format!("MiniSnap_2026-10-29_12-{:02}-00", MAX_SESSIONS + extra - 1))
            .exists(), "newest kept");
    }
}
