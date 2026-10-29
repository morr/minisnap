//! Global hotkeys: registration and main-thread dispatch.

use std::cell::RefCell;
use std::collections::HashMap;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyManager, GlobalHotKeyEvent, HotKeyState};

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
