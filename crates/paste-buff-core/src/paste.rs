//! Find the app to paste into, hand focus back to it, and send Cmd+V.

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use std::fmt;
use std::thread;
use std::time::Duration;

pub const KEY_V: CGKeyCode = 9;
pub const KEY_ESCAPE: CGKeyCode = 53;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontApp {
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
}

#[derive(Debug)]
pub enum PasteError {
    EventSource,
    Event,
}

impl fmt::Display for PasteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasteError::EventSource => write!(f, "could not create CGEventSource"),
            PasteError::Event => write!(f, "could not create CGEvent"),
        }
    }
}

impl std::error::Error for PasteError {}

/// The app that currently owns the keyboard (call this *before* showing
/// our own window).
pub fn frontmost_app() -> Option<FrontApp> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(FrontApp {
        pid: app.processIdentifier(),
        name: app
            .localizedName()
            .map(|n| n.to_string())
            .unwrap_or_default(),
        bundle_id: app.bundleIdentifier().map(|b| b.to_string()),
    })
}

/// Bring `pid` to the front. Returns false if the process is gone.
pub fn activate_app(pid: i32) -> bool {
    match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        #[allow(deprecated)]
        Some(app) => app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps),
        None => false,
    }
}

/// Post a key down + key up pair to the HID event tap.
/// Requires the Accessibility permission for the responsible process.
pub fn post_key(keycode: CGKeyCode, flags: CGEventFlags) -> Result<(), PasteError> {
    for down in [true, false] {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| PasteError::EventSource)?;
        let event = CGEvent::new_keyboard_event(source, keycode, down)
            .map_err(|_| PasteError::Event)?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

pub fn send_cmd_v() -> Result<(), PasteError> {
    post_key(KEY_V, CGEventFlags::CGEventFlagCommand)
}

pub fn send_cmd_shift_v() -> Result<(), PasteError> {
    post_key(
        KEY_V,
        CGEventFlags::CGEventFlagCommand | CGEventFlags::CGEventFlagShift,
    )
}

pub fn send_escape() -> Result<(), PasteError> {
    post_key(KEY_ESCAPE, CGEventFlags::CGEventFlagNull)
}

/// Re-activate `target` now and send Cmd+V after `delay` on a helper thread,
/// so the frontend can hide its window without blocking.
pub fn paste_into(target: Option<&FrontApp>, delay: Duration) {
    if let Some(app) = target {
        activate_app(app.pid);
    }
    thread::spawn(move || {
        thread::sleep(delay);
        if let Err(err) = send_cmd_v() {
            eprintln!("paste-buff: {err}");
        }
    });
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
}

/// Whether this process (via its responsible app) may post keyboard events.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Like [`accessibility_trusted`], but when the answer is no, asks macOS to
/// show its "allow this app to control your computer" prompt, which adds the
/// binary to System Settings > Privacy & Security > Accessibility. The prompt
/// appears at most once per binary; afterwards the user flips the switch.
pub fn accessibility_trusted_prompting() -> bool {
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_CFType_pairs(&[(
        key.as_CFType(),
        CFBoolean::true_value().as_CFType(),
    )]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}
