//! Start at login via `SMAppService.mainApp` (macOS 13+).
//!
//! Registration happens once, on the first launch from an `.app` bundle. A
//! marker file records that it was done, so a user who later switches it off
//! in System Settings > General > Login Items is not overridden on the next
//! launch. A bare binary (`cargo run`, the benchmark harness) never registers:
//! SMAppService only works for bundled apps.

use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::fs;

const MARKER: &str = "login-item-registered";

pub fn register_once() {
    if !NSBundle::mainBundle().bundlePath().to_string().ends_with(".app") {
        return;
    }
    let marker = paste_buff_core::data_dir().join(MARKER);
    if marker.exists() {
        return;
    }
    // SAFETY: plain ServiceManagement calls; no threading requirements.
    let service = unsafe { SMAppService::mainAppService() };
    if unsafe { service.status() } != SMAppServiceStatus::Enabled {
        if let Err(err) = unsafe { service.registerAndReturnError() } {
            eprintln!("paste-buff-appkit: could not add login item: {err}");
            return;
        }
    }
    if let Err(err) = fs::create_dir_all(paste_buff_core::data_dir())
        .and_then(|()| fs::write(&marker, b""))
    {
        eprintln!("paste-buff-appkit: could not write {}: {err}", marker.display());
    }
}
