//! Background thread that polls the pasteboard and records new text.

use crate::{clipboard, history::History, paste};
use objc2::rc::autoreleasepool;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How often expired entries are pruned from memory and disk.
pub const PRUNE_EVERY: Duration = Duration::from_secs(60);

/// Start polling. Every change to the pasteboard's plain-text content is
/// pushed into `history` and persisted.
pub fn spawn(history: Arc<Mutex<History>>, interval: Duration) -> JoinHandle<()> {
    thread::Builder::new()
        .name("clipboard-watcher".into())
        .spawn(move || run(history, interval))
        .expect("spawn clipboard watcher")
}

fn run(history: Arc<Mutex<History>>, interval: Duration) {
    let mut last_count = autoreleasepool(|_| clipboard::change_count());
    let mut last_prune = Instant::now();
    loop {
        thread::sleep(interval);
        autoreleasepool(|_| {
            let count = clipboard::change_count();
            if count != last_count {
                last_count = count;
                if let Some(clip) = clipboard::read() {
                    let source = paste::frontmost_app().map(|a| a.name);
                    let mut h = history.lock().unwrap_or_else(|e| e.into_inner());
                    if h.push_clip(clip, source).is_some() {
                        if let Err(err) = h.save() {
                            eprintln!("paste-buff: failed to save history: {err}");
                        }
                    }
                }
            }
            if last_prune.elapsed() >= PRUNE_EVERY {
                last_prune = Instant::now();
                let mut h = history.lock().unwrap_or_else(|e| e.into_inner());
                if h.prune() > 0 {
                    let _ = h.save();
                }
            }
        });
    }
}
