//! Everything the paste-buff frontends share: the history store, the
//! pasteboard watcher, paste-into-previous-app, and macOS app helpers.
//!
//! Frontends (Tauri, GPUI, ...) only add a window and a hotkey on top.

pub mod app;
pub mod clipboard;
pub mod history;
pub mod paste;
pub mod signal;
pub mod watcher;

pub use history::{age_label, human_bytes, now_ms, relative_time, Clip, Content, Entry, History, DEFAULT_TTL};
pub use paste::FrontApp;

use std::path::PathBuf;
use std::time::Duration;

pub const APP_NAME: &str = "paste-buff";

/// How often the pasteboard change counter is polled.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Delay between re-activating the target app and sending Cmd+V.
pub const PASTE_DELAY: Duration = Duration::from_millis(120);

/// Environment variable that overrides the history file location
/// (handy for benchmarking each frontend against its own file).
pub const HISTORY_ENV: &str = "PASTE_BUFF_HISTORY";

/// `~/Library/Application Support/paste-buff`
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library")
        .join("Application Support")
        .join(APP_NAME)
}

/// Path of the persisted history (JSON). Overridable with `PASTE_BUFF_HISTORY`.
pub fn history_path() -> PathBuf {
    match std::env::var_os(HISTORY_ENV) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => data_dir().join("history.json"),
    }
}
