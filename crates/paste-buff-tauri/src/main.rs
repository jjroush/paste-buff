//! paste-buff, Tauri flavour: a hidden WKWebView window summoned by Cmd+Shift+V.

use paste_buff_core::{
    clipboard,
    history::{self, History},
    paste::{self, FrontApp},
    signal, watcher, DEFAULT_TTL, PASTE_DELAY, POLL_INTERVAL,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const WINDOW: &str = "main";

struct AppState {
    history: Arc<Mutex<History>>,
    /// The app that owned the keyboard when the window was summoned.
    target: Mutex<Option<FrontApp>>,
}

#[derive(Serialize, Clone)]
struct EntryDto {
    id: u64,
    preview: String,
    text: String,
    copied_at_ms: u64,
    times_copied: u32,
    source_app: Option<String>,
    chars: usize,
    words: usize,
    lines: usize,
    bucket: &'static str,
    age: String,
    is_url: bool,
}

impl EntryDto {
    fn new(entry: &history::Entry, now: u64) -> Self {
        Self {
            id: entry.id,
            preview: entry.preview(80),
            text: entry.text.clone(),
            copied_at_ms: entry.copied_at_ms,
            times_copied: entry.times_copied,
            source_app: entry.source_app.clone(),
            chars: entry.char_count(),
            words: entry.word_count(),
            lines: entry.line_count(),
            bucket: history::age_label(entry.copied_at_ms, now),
            age: history::relative_time(entry.copied_at_ms, now),
            is_url: entry.is_url(),
        }
    }
}

#[tauri::command]
fn get_history(state: State<'_, AppState>, query: String) -> Vec<EntryDto> {
    let now = history::now_ms();
    let h = state.history.lock().unwrap_or_else(|e| e.into_inner());
    h.search(&query)
        .into_iter()
        .map(|e| EntryDto::new(e, now))
        .collect()
}

#[tauri::command]
fn target_name(state: State<'_, AppState>) -> Option<String> {
    state
        .target
        .lock()
        .ok()?
        .as_ref()
        .map(|t| t.name.clone())
}

#[tauri::command]
fn choose(app: AppHandle, state: State<'_, AppState>, id: u64) -> bool {
    let text = {
        let h = state.history.lock().unwrap_or_else(|e| e.into_inner());
        h.get(id).map(|e| e.text.clone())
    };
    let Some(text) = text else {
        return false;
    };
    let target = state
        .target
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    hide_window(&app);
    clipboard::write_string(&text);
    paste::paste_into(target.as_ref(), PASTE_DELAY);
    true
}

#[tauri::command]
fn dismiss(app: AppHandle) {
    hide_window(&app);
}

#[tauri::command]
fn remove(state: State<'_, AppState>, id: u64) -> bool {
    let mut h = state.history.lock().unwrap_or_else(|e| e.into_inner());
    let removed = h.remove(id);
    if removed {
        let _ = h.save();
    }
    removed
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn show_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    // Capture the paste target before we steal focus.
    *state.target.lock().unwrap_or_else(|e| e.into_inner()) = paste::frontmost_app();
    let _ = app.show();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("show", ());
    eprintln!(
        "paste-buff-tauri: shown (target: {})",
        state
            .target
            .lock()
            .ok()
            .and_then(|t| t.as_ref().map(|t| t.name.clone()))
            .unwrap_or_else(|| "none".into())
    );
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
    // Hiding the app hands activation back to whoever had it before.
    let _ = app.hide();
    eprintln!("paste-buff-tauri: hidden");
}

fn is_shown(app: &AppHandle) -> bool {
    app.get_webview_window(WINDOW)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn toggle(app: &AppHandle) {
    if is_shown(app) {
        hide_window(app);
    } else {
        show_window(app);
    }
}

fn main() {
    let history = Arc::new(Mutex::new(History::load(
        paste_buff_core::history_path(),
        DEFAULT_TTL,
    )));
    watcher::spawn(Arc::clone(&history), POLL_INTERVAL);

    let shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV);

    tauri::Builder::default()
        .manage(AppState {
            history,
            target: Mutex::new(None),
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, fired, event| {
                    if event.state == ShortcutState::Pressed && fired.id() == shortcut.id() {
                        toggle(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_history,
            target_name,
            choose,
            dismiss,
            remove,
            quit
        ])
        .setup(move |app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            if let Err(err) = app.global_shortcut().register(shortcut) {
                eprintln!(
                    "paste-buff-tauri: could not register Cmd+Shift+V ({err}). \
                     Another app (Raycast?) may own it; SIGUSR1 still toggles the window."
                );
            }
            if !paste::accessibility_trusted() {
                eprintln!(
                    "paste-buff-tauri: no Accessibility permission; Enter will copy \
                     the entry but cannot send Cmd+V to the target app."
                );
            }
            let handle = app.handle().clone();
            signal::listen(move || {
                let app = handle.clone();
                let _ = handle.run_on_main_thread(move || toggle(&app));
            })?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(false) = event {
                let _ = window.hide();
                let _ = window.app_handle().hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running paste-buff-tauri");
}
