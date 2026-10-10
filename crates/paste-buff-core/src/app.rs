//! Process-level AppKit helpers. All of these must run on the main thread.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Hide the Dock icon and app switcher entry (menu-bar-utility style).
pub fn set_accessory_policy() -> bool {
    match MainThreadMarker::new() {
        Some(mtm) => NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory),
        None => false,
    }
}

/// `[NSApp hide:]` — hides every window and hands activation to the
/// previously active app.
pub fn hide() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
    }
}

/// Make this app the active app, un-hiding it if needed.
pub fn activate() {
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}
