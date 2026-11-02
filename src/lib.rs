//! Shared library of minisnap: the app binary and the `minisnap-mcp`
//! agent bridge are both built on these modules.

pub mod app;
pub mod capture;
pub mod config;
pub mod editor;
pub mod editor_arrow_tuner;
pub mod editor_toolbar;
pub mod editor_window;
pub mod hotkey_recorder;
pub mod hotkeys;
pub mod overlay;
pub mod overlay_hud;
pub mod output;
pub mod permission;
pub mod plates;
pub mod prefs;
pub mod sound;
