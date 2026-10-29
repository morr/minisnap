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

/// Map AppKit event modifier flags to `global_hotkey` modifiers (used by
/// the preferences recorder).
pub fn modifiers_from_flags(flags: NSEventModifierFlags) -> Modifiers {
    let mut mods = Modifiers::empty();
    if flags.contains(NSEventModifierFlags::Control) {
        mods.insert(Modifiers::CONTROL);
    }
    if flags.contains(NSEventModifierFlags::Option) {
        mods.insert(Modifiers::ALT);
    }
    if flags.contains(NSEventModifierFlags::Shift) {
        mods.insert(Modifiers::SHIFT);
    }
    if flags.contains(NSEventModifierFlags::Command) {
        mods.insert(Modifiers::META);
    }
    mods
}

/// Reverse of the Carbon keycodes `global-hotkey` registers with:
/// `NSEvent.keyCode` → `Code`, for the preferences recorder.
pub fn scancode_to_key(scan: u16) -> Option<Code> {
    Some(match scan {
        0x00 => Code::KeyA,
        0x01 => Code::KeyS,
        0x02 => Code::KeyD,
        0x03 => Code::KeyF,
        0x04 => Code::KeyH,
        0x05 => Code::KeyG,
        0x06 => Code::KeyZ,
        0x07 => Code::KeyX,
        0x08 => Code::KeyC,
        0x09 => Code::KeyV,
        0x0a => Code::IntlBackslash,
        0x0b => Code::KeyB,
        0x0c => Code::KeyQ,
        0x0d => Code::KeyW,
        0x0e => Code::KeyE,
        0x0f => Code::KeyR,
        0x10 => Code::KeyY,
        0x11 => Code::KeyT,
        0x12 => Code::Digit1,
        0x13 => Code::Digit2,
        0x14 => Code::Digit3,
        0x15 => Code::Digit4,
        0x16 => Code::Digit6,
        0x17 => Code::Digit5,
        0x18 => Code::Equal,
        0x19 => Code::Digit9,
        0x1a => Code::Digit7,
        0x1b => Code::Minus,
        0x1c => Code::Digit8,
        0x1d => Code::Digit0,
        0x1e => Code::BracketRight,
        0x1f => Code::KeyO,
        0x20 => Code::KeyU,
        0x21 => Code::BracketLeft,
        0x22 => Code::KeyI,
        0x23 => Code::KeyP,
        0x24 => Code::Enter,
        0x25 => Code::KeyL,
        0x26 => Code::KeyJ,
        0x27 => Code::Quote,
        0x28 => Code::KeyK,
        0x29 => Code::Semicolon,
        0x2a => Code::Backslash,
        0x2b => Code::Comma,
        0x2c => Code::Slash,
        0x2d => Code::KeyN,
        0x2e => Code::KeyM,
        0x2f => Code::Period,
        0x30 => Code::Tab,
        0x31 => Code::Space,
        0x32 => Code::Backquote,
        0x33 => Code::Backspace,
        0x35 => Code::Escape,
        0x39 => Code::CapsLock,
        0x40 => Code::F17,
        0x41 => Code::NumpadDecimal,
        0x43 => Code::NumpadMultiply,
        0x45 => Code::NumpadAdd,
        0x46 => Code::PrintScreen,
        0x47 => Code::NumLock,
        0x48 => Code::AudioVolumeUp,
        0x49 => Code::AudioVolumeDown,
        0x4a => Code::AudioVolumeMute,
        0x4b => Code::NumpadDivide,
        0x4c => Code::NumpadEnter,
        0x4e => Code::NumpadSubtract,
        0x4f => Code::F18,
        0x50 => Code::F19,
        0x51 => Code::NumpadEqual,
        0x52 => Code::Numpad0,
        0x53 => Code::Numpad1,
        0x54 => Code::Numpad2,
        0x55 => Code::Numpad3,
        0x56 => Code::Numpad4,
        0x57 => Code::Numpad5,
        0x58 => Code::Numpad6,
        0x59 => Code::Numpad7,
        0x5a => Code::F20,
        0x5b => Code::Numpad8,
        0x5c => Code::Numpad9,
        0x5d => Code::IntlYen,
        0x5e => Code::IntlRo,
        0x5f => Code::NumpadComma,
        0x60 => Code::F5,
        0x61 => Code::F6,
        0x62 => Code::F7,
        0x63 => Code::F3,
        0x64 => Code::F8,
        0x65 => Code::F9,
        0x66 => Code::Lang2,
        0x67 => Code::F11,
        0x68 => Code::Lang1,
        0x69 => Code::F13,
        0x6a => Code::F16,
        0x6b => Code::F14,
        0x6d => Code::F10,
        0x6f => Code::F12,
        0x71 => Code::F15,
        0x72 => Code::Help,
        0x73 => Code::Home,
        0x74 => Code::PageUp,
        0x75 => Code::Delete,
        0x76 => Code::F4,
        0x77 => Code::End,
        0x78 => Code::F2,
        0x79 => Code::PageDown,
        0x7a => Code::F1,
        0x7b => Code::ArrowLeft,
        0x7c => Code::ArrowRight,
        0x7d => Code::ArrowDown,
        0x7e => Code::ArrowUp,
        _ => return None,
    })
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
    fn event_flags_decode_into_modifiers() {
        assert_eq!(
            modifiers_from_flags(NSEventModifierFlags::Option | NSEventModifierFlags::Command),
            Modifiers::ALT | Modifiers::META
        );
        assert_eq!(
            modifiers_from_flags(NSEventModifierFlags(0)),
            Modifiers::empty()
        );
        // CapsLock and Function bits are ignored.
        let extra = NSEventModifierFlags(1 << 16) | NSEventModifierFlags(1 << 23);
        assert_eq!(modifiers_from_flags(extra), Modifiers::empty());
    }

    #[test]
    fn scancodes_decode_for_recorder() {
        assert_eq!(scancode_to_key(0x17), Some(Code::Digit5));
        assert_eq!(scancode_to_key(0x1d), Some(Code::Digit0));
        assert_eq!(scancode_to_key(0x00), Some(Code::KeyA));
        assert_eq!(scancode_to_key(0x35), Some(Code::Escape));
        assert_eq!(scancode_to_key(0x7a), Some(Code::F1));
        assert_eq!(scancode_to_key(0x7b), Some(Code::ArrowLeft));
        assert_eq!(scancode_to_key(0x31), Some(Code::Space));
        assert_eq!(scancode_to_key(0x36), None, "unknown keycode");
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
