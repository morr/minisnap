//! Global hotkeys: registration and main-thread dispatch.

use std::cell::RefCell;
use std::collections::HashMap;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyManager, GlobalHotKeyEvent, HotKeyState};
use objc2_app_kit::NSEventModifierFlags;

/// What a registered global hotkey does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Start the frozen-area capture mode.
    Capture,
    /// Open the latest saved shot.
    OpenLatest,
}

/// Owns the Carbon registrations and maps event ids to actions.
///
/// Dropping the inner manager unregisters everything, so re-registration
/// is "drop the manager, register fresh".
pub struct HotkeyRegistry {
    manager: RefCell<Option<GlobalHotKeyManager>>,
    actions: HashMap<u32, HotkeyAction>,
}

impl HotkeyRegistry {
    pub fn new() -> Self {
        HotkeyRegistry {
            manager: RefCell::new(None),
            actions: HashMap::new(),
        }
    }

    /// Replace the current registrations with `capture` and `open`.
    /// If one combination is already taken by another app it is reported,
    /// and the other one is still registered.
    pub fn apply(&mut self, capture: HotKey, open: HotKey) {
        self.manager.borrow_mut().take();
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("hotkeys: cannot create manager: {e:?}");
                return;
            }
        };
        self.actions.clear();
        for (hotkey, action) in
            [(capture, HotkeyAction::Capture), (open, HotkeyAction::OpenLatest)]
        {
            match manager.register(hotkey) {
                Ok(()) => {
                    self.actions.insert(hotkey.id(), action);
                }
                Err(e) => {
                    eprintln!("hotkeys: cannot register {hotkey}: {e:?}");
                }
            }
        }
        self.manager.borrow_mut().replace(manager);
    }

    /// Drain pending global-hotkey events, returning the actions to run.
    /// Events without a live registration (e.g. a just-replaced combination)
    /// are dropped.
    pub fn pending_actions(&self) -> Vec<HotkeyAction> {
        let receiver = GlobalHotKeyEvent::receiver();
        let mut actions = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if event.state() == HotKeyState::Pressed {
                if let Some(action) = self.actions.get(&event.id()) {
                    actions.push(*action);
                }
            }
        }
        actions
    }
}

/// `NSMenuItem.keyEquivalent` string for a key code. Function and arrow keys
/// use the private-use characters AppKit maps to those physical keys.
pub fn key_equivalent_string(key: Code) -> String {
    // NSF1FunctionKey = 0xF704 … NSF35FunctionKey = 0xF726.
    const NSF_FUNCTION_KEY_BASE: u32 = 0xF704;
    match key {
        Code::ArrowUp => "\u{F700}".into(),
        Code::ArrowDown => "\u{F701}".into(),
        Code::ArrowLeft => "\u{F702}".into(),
        Code::ArrowRight => "\u{F703}".into(),
        Code::Enter | Code::NumpadEnter => "\r".into(),
        Code::Escape => "\u{1b}".into(),
        Code::Tab => "\t".into(),
        Code::Space => " ".into(),
        Code::Backspace => "\u{7f}".into(),
        Code::Minus => "-".into(),
        Code::Equal => "=".into(),
        Code::BracketLeft => "[".into(),
        Code::BracketRight => "]".into(),
        Code::Backslash => "\\".into(),
        Code::Semicolon => ";".into(),
        Code::Quote => "'".into(),
        Code::Backquote => "`".into(),
        Code::Comma => ",".into(),
        Code::Period => ".".into(),
        Code::Slash => "/".into(),
        Code::Digit0 => "0".into(),
        Code::Digit1 => "1".into(),
        Code::Digit2 => "2".into(),
        Code::Digit3 => "3".into(),
        Code::Digit4 => "4".into(),
        Code::Digit5 => "5".into(),
        Code::Digit6 => "6".into(),
        Code::Digit7 => "7".into(),
        Code::Digit8 => "8".into(),
        Code::Digit9 => "9".into(),
        Code::KeyA => "a".into(),
        Code::KeyB => "b".into(),
        Code::KeyC => "c".into(),
        Code::KeyD => "d".into(),
        Code::KeyE => "e".into(),
        Code::KeyF => "f".into(),
        Code::KeyG => "g".into(),
        Code::KeyH => "h".into(),
        Code::KeyI => "i".into(),
        Code::KeyJ => "j".into(),
        Code::KeyK => "k".into(),
        Code::KeyL => "l".into(),
        Code::KeyM => "m".into(),
        Code::KeyN => "n".into(),
        Code::KeyO => "o".into(),
        Code::KeyP => "p".into(),
        Code::KeyQ => "q".into(),
        Code::KeyR => "r".into(),
        Code::KeyS => "s".into(),
        Code::KeyT => "t".into(),
        Code::KeyU => "u".into(),
        Code::KeyV => "v".into(),
        Code::KeyW => "w".into(),
        Code::KeyX => "x".into(),
        Code::KeyY => "y".into(),
        Code::KeyZ => "z".into(),
        Code::NumpadAdd => "+".into(),
        Code::NumpadSubtract => "-".into(),
        Code::NumpadMultiply => "*".into(),
        Code::NumpadDivide => "/".into(),
        other => {
            let name = format!("{other:?}");
            if let Some(rest) = name.strip_prefix('F') {
                if let Ok(n) = rest.parse::<u32>() {
                    if (1..=35).contains(&n) {
                        return char::from_u32(NSF_FUNCTION_KEY_BASE + n - 1)
                            .map(String::from)
                            .unwrap_or_default();
                    }
                }
            }
            name
        }
    }
}

/// Map `global_hotkey` modifiers to AppKit menu modifier flags.
pub fn modifier_flags(mods: Modifiers) -> NSEventModifierFlags {
    let mut flags = NSEventModifierFlags(0);
    if mods.contains(Modifiers::CONTROL) {
        flags.insert(NSEventModifierFlags::Control);
    }
    if mods.contains(Modifiers::ALT) {
        flags.insert(NSEventModifierFlags::Option);
    }
    if mods.contains(Modifiers::SHIFT) {
        flags.insert(NSEventModifierFlags::Shift);
    }
    if mods.contains(Modifiers::META) {
        flags.insert(NSEventModifierFlags::Command);
    }
    flags
}

/// Render a modifier+key combo as glyphs in the system order (⌃⌥⇧⌘).
/// Used by minisnap's own UI (the preferences recorder); the tray menu
/// renders shortcuts itself via `NSMenuItem.keyEquivalent`.
#[allow(dead_code)] // wired up by the preferences window
pub fn render_combo(mods: Modifiers, key: Code) -> String {
    let mut out = String::new();
    if mods.contains(Modifiers::CONTROL) {
        out.push('⌃');
    }
    if mods.contains(Modifiers::ALT) {
        out.push('⌥');
    }
    if mods.contains(Modifiers::SHIFT) {
        out.push('⇧');
    }
    if mods.contains(Modifiers::META) {
        out.push('⌘');
    }
    out.push_str(&key_symbol(key));
    out
}

/// Human-readable symbol for a key code (used by `render_combo`).
#[allow(dead_code)] // wired up by the preferences window
pub fn key_symbol(key: Code) -> String {
    match key {
        Code::Space => "␣".into(),
        Code::Enter => "↩".into(),
        Code::Escape => "⎋".into(),
        Code::Tab => "⇥".into(),
        Code::Backspace => "⌫".into(),
        Code::Delete => "⌦".into(),
        Code::ArrowLeft => "←".into(),
        Code::ArrowRight => "→".into(),
        Code::ArrowUp => "↑".into(),
        Code::ArrowDown => "↓".into(),
        Code::Minus => "−".into(),
        Code::CapsLock => "⇪".into(),
        Code::NumpadAdd => "+".into(),
        Code::NumpadSubtract => "−".into(),
        Code::NumpadMultiply => "*".into(),
        Code::NumpadDivide => "/".into(),
        Code::NumpadEnter => "↩".into(),
        other => {
            // Digits ("Digit5" → "5"), letters ("KeyA" → "A"), F-keys
            // ("F12" → "F12"), punctuation names as-is.
            let name = format!("{other:?}");
            if let Some(rest) = name.strip_prefix("Digit") {
                return rest.to_string();
            }
            if let Some(rest) = name.strip_prefix("Key") {
                return rest.to_uppercase();
            }
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(mods: Modifiers, key: Code) -> String {
        render_combo(mods, key)
    }

    #[test]
    fn key_equivalents_map_to_menu_characters() {
        assert_eq!(key_equivalent_string(Code::Digit5), "5");
        assert_eq!(key_equivalent_string(Code::Digit0), "0");
        assert_eq!(key_equivalent_string(Code::KeyA), "a");
        assert_eq!(key_equivalent_string(Code::F1), "\u{F704}");
        assert_eq!(key_equivalent_string(Code::F5), "\u{F708}");
        assert_eq!(key_equivalent_string(Code::ArrowDown), "\u{F701}");
        assert_eq!(key_equivalent_string(Code::Space), " ");
    }

    #[test]
    fn modifier_flags_map_to_appkit() {
        let flags = modifier_flags(Modifiers::ALT | Modifiers::META);
        assert!(flags.contains(NSEventModifierFlags::Option));
        assert!(flags.contains(NSEventModifierFlags::Command));
        assert!(!flags.contains(NSEventModifierFlags::Shift));
    }

    #[test]
    fn system_modifier_order_is_control_option_shift_command() {
        let all = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META;
        assert_eq!(combo(all, Code::Digit5), "⌃⌥⇧⌘5");
    }

    #[test]
    fn typical_combos_render() {
        assert_eq!(
            combo(Modifiers::ALT | Modifiers::META, Code::Digit5),
            "⌥⌘5"
        );
        assert_eq!(
            combo(Modifiers::ALT | Modifiers::META, Code::Digit0),
            "⌥⌘0"
        );
        assert_eq!(combo(Modifiers::META, Code::KeyQ), "⌘Q");
        assert_eq!(
            combo(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyA),
            "⌃⇧A"
        );
        assert_eq!(combo(Modifiers::empty(), Code::Escape), "⎋");
        assert_eq!(combo(Modifiers::META, Code::F5), "⌘F5");
    }
}
