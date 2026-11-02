//! The hotkey recorder state machine: the rules behind the preferences
//! window rows, free of AppKit.
//!
//! A row click starts a recording; the first keydown (fed to `feed` by the
//! local event monitor in `app.rs`, after NSEvent unpacking) fixes the new
//! combination or is swallowed. Bare Esc cancels. A key with no modifiers
//! is not a combination — it is swallowed and the recording keeps going,
//! same as unknown key codes; a combination needs at least one modifier.

use global_hotkey::hotkey::{HotKey, Modifiers};

use crate::hotkeys::scancode_to_key;
use crate::prefs;

/// The hotkey recorder: which preferences row (if any) is waiting for a
/// keydown. The row is the `NSButton` tag (`prefs::ROW_*`).
pub struct Recorder {
    recording: Option<isize>,
}

/// What one fed keydown means for the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedOutcome {
    /// The combination is fixed for this row; the recording is over.
    Recorded { row: isize, hotkey: HotKey },
    /// Bare Esc: the recording is cancelled, the previous combination stays.
    Cancelled,
    /// Unknown key code: the event is swallowed, the recording continues.
    Swallowed,
    /// Nothing is being recorded: the event passes through untouched.
    Passthrough,
}

impl Recorder {
    /// No row is recording.
    pub fn idle() -> Self {
        Recorder { recording: None }
    }

    /// Start recording for the row with the given tag. `false` means the
    /// tag is not a recorder row or another row already records — the
    /// caller stays silent either way.
    pub fn begin(&mut self, row: isize) -> bool {
        if row != prefs::ROW_CAPTURE
            && row != prefs::ROW_CAPTURE_EDIT
            && row != prefs::ROW_OPEN_LATEST
        {
            return false;
        }
        if self.recording.is_some() {
            return false;
        }
        self.recording = Some(row);
        true
    }

    /// The row currently recording, if any; the row button shows the
    /// "Press keys…" prompt while it is `Some`.
    pub fn recording_row(&self) -> Option<isize> {
        self.recording
    }

    /// Apply the recorder rules to one keydown. `Recorded` and `Cancelled`
    /// end the recording; `Swallowed` keeps it going; `Passthrough` means
    /// no row was recording.
    pub fn feed(&mut self, mods: Modifiers, keycode: u16) -> FeedOutcome {
        let Some(row) = self.recording else {
            return FeedOutcome::Passthrough;
        };
        if keycode == 0x35 && mods.is_empty() {
            // Bare Esc: cancel the recording, keep the previous combination.
            self.recording = None;
            return FeedOutcome::Cancelled;
        }
        if mods.is_empty() {
            // A bare key is not a combination: swallowing it keeps the
            // recording going, so the next press can still land.
            return FeedOutcome::Swallowed;
        }
        let Some(key) = scancode_to_key(keycode) else {
            return FeedOutcome::Swallowed;
        };
        self.recording = None;
        FeedOutcome::Recorded { row, hotkey: HotKey::new(Some(mods), key) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use global_hotkey::hotkey::Code;
    use prefs::{ROW_CAPTURE, ROW_CAPTURE_EDIT, ROW_OPEN_LATEST};

    const ESC: u16 = 0x35;
    const KEY_A: u16 = 0x00;
    const KEY_T: u16 = 0x11;
    const UNKNOWN: u16 = 0x80;

    fn recorded(outcome: FeedOutcome, row: isize) -> HotKey {
        match outcome {
            FeedOutcome::Recorded { row: r, hotkey } => {
                assert_eq!(r, row, "the combination lands on the recording row");
                hotkey
            }
            other => panic!("expected Recorded, got {other:?}"),
        }
    }

    #[test]
    fn begin_rejects_unknown_rows_and_re_entry_but_accepts_the_three_rows() {
        for row in [ROW_CAPTURE, ROW_CAPTURE_EDIT, ROW_OPEN_LATEST] {
            let mut machine = Recorder::idle();
            assert!(machine.begin(row), "each recorder row can start a recording");
            assert_eq!(machine.recording_row(), Some(row));
        }
        // Not a recorder row at all (a stray tag).
        let mut machine = Recorder::idle();
        assert!(!machine.begin(99));
        assert_eq!(machine.recording_row(), None);
        // While a row records, another row does not steal the recording.
        let mut machine = Recorder::idle();
        assert!(machine.begin(ROW_CAPTURE));
        assert!(!machine.begin(ROW_CAPTURE_EDIT));
        assert!(!machine.begin(ROW_OPEN_LATEST));
        assert_eq!(machine.recording_row(), Some(ROW_CAPTURE));
    }

    #[test]
    fn feeding_an_idle_machine_passes_everything_through() {
        let mut machine = Recorder::idle();
        assert_eq!(machine.feed(Modifiers::empty(), KEY_A), FeedOutcome::Passthrough);
        assert_eq!(machine.feed(Modifiers::META, ESC), FeedOutcome::Passthrough);
        assert_eq!(machine.recording_row(), None);
    }

    #[test]
    fn bare_esc_cancels_and_esc_with_a_modifier_records_the_combo() {
        let mut machine = Recorder::idle();
        machine.begin(ROW_CAPTURE_EDIT);
        assert_eq!(machine.feed(Modifiers::empty(), ESC), FeedOutcome::Cancelled);
        assert_eq!(machine.recording_row(), None, "the recording is over");
        // After the cancel the machine is idle: events pass through again.
        assert_eq!(machine.feed(Modifiers::empty(), KEY_A), FeedOutcome::Passthrough);
        // Esc pressed together with a modifier is a combination, not a cancel.
        let mut machine = Recorder::idle();
        machine.begin(ROW_OPEN_LATEST);
        let hotkey = recorded(
            machine.feed(Modifiers::META, ESC),
            ROW_OPEN_LATEST,
        );
        assert_eq!(hotkey, HotKey::new(Some(Modifiers::META), Code::Escape));
    }

    #[test]
    fn unknown_key_code_is_swallowed_but_the_recording_continues() {
        let mut machine = Recorder::idle();
        machine.begin(ROW_CAPTURE);
        assert_eq!(machine.feed(Modifiers::META, UNKNOWN), FeedOutcome::Swallowed);
        assert_eq!(machine.recording_row(), Some(ROW_CAPTURE), "still recording");
        // The next valid combination still lands.
        let hotkey = recorded(machine.feed(Modifiers::CONTROL, KEY_A), ROW_CAPTURE);
        assert_eq!(hotkey, HotKey::new(Some(Modifiers::CONTROL), Code::KeyA));
    }

    #[test]
    fn bare_key_is_not_a_combination_and_the_recording_continues() {
        let mut machine = Recorder::idle();
        machine.begin(ROW_CAPTURE);
        assert_eq!(machine.feed(Modifiers::empty(), KEY_A), FeedOutcome::Swallowed);
        assert_eq!(machine.recording_row(), Some(ROW_CAPTURE), "still recording");
        // A combination with modifiers still lands after a bare-key press.
        let hotkey = recorded(machine.feed(Modifiers::META, KEY_A), ROW_CAPTURE);
        assert_eq!(hotkey, HotKey::new(Some(Modifiers::META), Code::KeyA));
        assert_eq!(machine.recording_row(), None);
    }

    #[test]
    fn the_first_keydown_fixes_the_modifiers_into_the_combination() {
        for (mods, keycode, key) in [
            (Modifiers::META | Modifiers::ALT, KEY_T, Code::KeyT),
            (Modifiers::CONTROL | Modifiers::SHIFT, KEY_T, Code::KeyT),
        ] {
            let mut machine = Recorder::idle();
            machine.begin(ROW_CAPTURE_EDIT);
            let hotkey = recorded(machine.feed(mods, keycode), ROW_CAPTURE_EDIT);
            assert_eq!(hotkey, HotKey::new(Some(mods), key));
            assert_eq!(machine.recording_row(), None, "one keydown is enough");
        }
    }
}
