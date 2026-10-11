//! paste-buff, GPUI flavour: a native, Metal-rendered panel summoned by Cmd+Shift+V.

use futures::{channel::mpsc, StreamExt};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers as HotMods},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gpui::{
    div, img, prelude::*, px, rgb, size, uniform_list, App, Application, AsyncApp, Bounds,
    Context, FocusHandle, KeyDownEvent, MouseButton, ObjectFit, ScrollStrategy, StyledImage,
    UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions,
};
use paste_buff_core::{
    app as macapp, clipboard,
    history::{self, file_name, human_bytes, Content, Entry, History},
    paste::{self, FrontApp},
    signal, watcher, DEFAULT_TTL, PASTE_DELAY, POLL_INTERVAL,
};
use std::ops::Range;
use std::sync::{Arc, Mutex};

const WIDTH: f32 = 760.0;
const HEIGHT: f32 = 460.0;
const LIST_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 34.0;
const PREVIEW_CHARS: usize = 4000;

const BG: u32 = 0x1c1c1e;
const BORDER: u32 = 0x313135;
const TEXT: u32 = 0xe8e8ea;
const MUTED: u32 = 0x8e8e93;
const SEL: u32 = 0x3a3a40;
const KBD: u32 = 0x333338;

struct PasteBuff {
    history: Arc<Mutex<History>>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    query: String,
    /// Snapshot of the filtered history, newest first.
    rows: Vec<Entry>,
    selected: usize,
    /// App that owned the keyboard when the panel was summoned.
    target: Option<FrontApp>,
    shown: bool,
    now: u64,
}

impl PasteBuff {
    fn new(history: Arc<Mutex<History>>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            history,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            target: None,
            shown: false,
            now: history::now_ms(),
        };
        this.refresh();
        this
    }

    fn refresh(&mut self) {
        self.now = history::now_ms();
        let h = self.history.lock().unwrap_or_else(|e| e.into_inner());
        self.rows = h.search(&self.query).into_iter().cloned().collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn select(&mut self, index: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let next = index.min(self.rows.len() - 1);
        // scroll_to_item is a no-op when the row is already visible, so the
        // strategy only decides where an off-screen row lands.
        let strategy = if next > self.selected {
            ScrollStrategy::Bottom
        } else {
            ScrollStrategy::Top
        };
        self.selected = next;
        self.scroll.scroll_to_item(next, strategy);
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Capture the paste target before we take focus.
        self.target = paste::frontmost_app();
        self.query.clear();
        self.selected = 0;
        self.refresh();
        self.shown = true;
        cx.activate(true);
        window.activate_window();
        window.focus(&self.focus_handle);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        eprintln!(
            "paste-buff-gpui: shown (target: {})",
            self.target
                .as_ref()
                .map(|t| t.name.as_str())
                .unwrap_or("none")
        );
        cx.notify();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        self.shown = false;
        // Hiding the app hands activation back to whoever had it before.
        cx.hide();
        eprintln!("paste-buff-gpui: hidden");
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown {
            self.hide(cx);
        } else {
            self.show(window, cx);
        }
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.rows.get(self.selected).cloned() else {
            return;
        };
        let target = self.target.clone();
        self.hide(cx);
        if clipboard::write(&entry) {
            paste::paste_into(target.as_ref(), PASTE_DELAY);
        } else {
            eprintln!("paste-buff-gpui: could not put entry {} on the pasteboard", entry.id);
        }
    }

    fn remove_selected(&mut self) {
        let Some(entry) = self.rows.get(self.selected) else {
            return;
        };
        let id = entry.id;
        {
            let mut h = self.history.lock().unwrap_or_else(|e| e.into_inner());
            if h.remove(id) {
                let _ = h.save();
            }
        }
        self.refresh();
    }

    fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
        self.refresh();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let mods = &keystroke.modifiers;
        match keystroke.key.as_str() {
            "escape" => self.hide(cx),
            "enter" => self.choose(cx),
            "up" => self.select(self.selected.saturating_sub(1)),
            "down" => self.select(self.selected + 1),
            "backspace" if mods.platform => self.remove_selected(),
            "delete" => self.remove_selected(),
            "backspace" => {
                let mut q = self.query.clone();
                q.pop();
                self.set_query(q);
            }
            "q" if mods.platform => cx.quit(),
            _ => {
                if mods.platform || mods.control {
                    return;
                }
                if let Some(ch) = keystroke.key_char.as_deref() {
                    let q = format!("{}{}", self.query, ch);
                    self.set_query(q);
                }
            }
        }
        cx.notify();
    }
}

fn kv(key: &'static str, value: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .py_0p5()
        .child(div().text_color(rgb(MUTED)).child(key))
        .child(div().child(value.into()))
}

fn kbd(label: &'static str) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_sm()
        .bg(rgb(KBD))
        .text_xs()
        .text_color(rgb(TEXT))
        .child(label)
}

fn row(ix: usize, entry: &Entry, selected: bool, now: u64, cx: &mut Context<PasteBuff>) -> impl IntoElement {
    div()
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .px_2p5()
        .rounded_md()
        .when(selected, |d| d.bg(rgb(SEL)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.select(ix);
                cx.notify();
            }),
        )
        .child(
            div()
                .w_4()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(match &entry.content {
                    Content::Image { .. } => "🖼",
                    Content::Files { .. } => "📁",
                    Content::Text if entry.is_url() => "🔗",
                    Content::Text => "📄",
                }),
        )
        .child(div().flex_1().truncate().child(entry.preview(80)))
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(history::relative_time(entry.copied_at_ms, now)),
        )
}

impl Render for PasteBuff {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.rows.len();
        let target_label = match &self.target {
            Some(t) if !t.name.is_empty() => format!("Paste to {}", t.name),
            _ => "Paste".to_string(),
        };

        let header = div()
            .h(px(48.0))
            .flex_none()
            .flex()
            .items_center()
            .px_3p5()
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_lg()
            .child(if self.query.is_empty() {
                div()
                    .text_color(rgb(MUTED))
                    .child("Type to filter entries…")
            } else {
                div().child(format!("{}▏", self.query))
            });

        let list = if self.rows.is_empty() {
            div()
                .flex_1()
                .p_6()
                .text_center()
                .text_color(rgb(MUTED))
                .child("Nothing copied in the last 24 hours")
                .into_any_element()
        } else {
            uniform_list(
                "entries",
                row_count,
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    range
                        .map(|ix| row(ix, &this.rows[ix], ix == this.selected, this.now, cx))
                        .collect()
                }),
            )
            .track_scroll(self.scroll.clone())
            .flex_1()
            .p_1p5()
            .into_any_element()
        };

        let (preview_body, info) = match self.rows.get(self.selected) {
            Some(e) => {
                let body = match &e.content {
                    Content::Text => div()
                        .size_full()
                        .overflow_hidden()
                        .p_3p5()
                        .font_family("Menlo")
                        .text_sm()
                        .child(e.text.chars().take(PREVIEW_CHARS).collect::<String>())
                        .into_any_element(),
                    Content::Files { paths } => div()
                        .size_full()
                        .overflow_hidden()
                        .p_3p5()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .children(paths.iter().take(200).map(|path| {
                            div()
                                .flex()
                                .flex_col()
                                .child(file_name(path))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .truncate()
                                        .child(path.display().to_string()),
                                )
                        }))
                        .into_any_element(),
                    Content::Image { file, .. } => div()
                        .size_full()
                        .p_3p5()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(file.clone())
                                .object_fit(ObjectFit::Contain)
                                .size_full()
                                .rounded_md(),
                        )
                        .into_any_element(),
                };
                let details: Vec<gpui::AnyElement> = match &e.content {
                    Content::Text => vec![
                        kv("Characters", e.char_count().to_string()).into_any_element(),
                        kv("Words", e.word_count().to_string()).into_any_element(),
                        kv("Lines", e.line_count().to_string()).into_any_element(),
                    ],
                    Content::Files { paths } => vec![
                        kv("Files", paths.len().to_string()).into_any_element(),
                        kv(
                            "Size",
                            human_bytes(
                                paths
                                    .iter()
                                    .filter_map(|p| std::fs::metadata(p).ok())
                                    .map(|m| m.len())
                                    .sum(),
                            ),
                        )
                        .into_any_element(),
                    ],
                    Content::Image { width, height, bytes, .. } => vec![
                        kv("Dimensions", format!("{width} × {height}")).into_any_element(),
                        kv("Size", human_bytes(*bytes)).into_any_element(),
                    ],
                };
                let info = div()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .px_3p5()
                    .py_2p5()
                    .text_sm()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .mb_1p5()
                            .child("Information"),
                    )
                    .child(kv("Source", e.source_app.clone().unwrap_or_else(|| "Unknown".into())))
                    .child(kv("Content type", e.kind_label()))
                    .children(details)
                    .child(kv("Times copied", e.times_copied.to_string()))
                    .child(kv(
                        "Copied",
                        format!(
                            "{} · {} ago",
                            history::age_label(e.copied_at_ms, self.now),
                            history::relative_time(e.copied_at_ms, self.now)
                        ),
                    ));
                (Some(body), Some(info))
            }
            None => (None, None),
        };

        let preview = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().overflow_hidden().children(preview_body))
            .children(info);

        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .w(px(LIST_WIDTH))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .child(list),
            )
            .child(preview);

        let footer = div()
            .h(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_3p5()
            .border_t_1()
            .border_color(rgb(BORDER))
            .text_sm()
            .text_color(rgb(MUTED))
            .child("paste-buff · GPUI")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().text_color(rgb(TEXT)).child(target_label))
                    .child(kbd("⏎"))
                    .child(div().ml_2().child("Delete"))
                    .child(kbd("⌘⌫")),
            );

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .rounded_xl()
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT))
            .text_sm()
            .child(header)
            .child(body)
            .child(footer)
    }
}

fn main() {
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("paste-buff {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let history = Arc::new(Mutex::new(History::load(
        paste_buff_core::history_path(),
        DEFAULT_TTL,
    )));
    watcher::spawn(Arc::clone(&history), POLL_INTERVAL);

    Application::new().run(move |cx: &mut App| {
        macapp::set_accessory_policy();
        if !paste::accessibility_trusted() {
            eprintln!(
                "paste-buff-gpui: no Accessibility permission; Enter will copy \
                 the entry but cannot send Cmd+V to the target app."
            );
        }

        // Hotkey presses and SIGUSR1 both land on this channel. The task at
        // the bottom sleeps on it, so an idle app does no work at all.
        let (tx, mut rx) = mpsc::unbounded::<()>();

        // Carbon hotkey. The manager must outlive the app, so it moves into
        // the task below. With a handler installed the crate calls it from
        // the Carbon event dispatcher instead of filling a channel we'd poll.
        let hotkey = HotKey::new(Some(HotMods::SUPER | HotMods::SHIFT), Code::KeyV);
        let hotkey_id = hotkey.id();
        let hotkey_tx = tx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed && event.id == hotkey_id {
                let _ = hotkey_tx.unbounded_send(());
            }
        }));
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => {
                if let Err(err) = m.register(hotkey) {
                    eprintln!(
                        "paste-buff-gpui: could not register Cmd+Shift+V ({err}). \
                         Another app (Raycast?) may own it; SIGUSR1 still toggles the window."
                    );
                }
                Some(m)
            }
            Err(err) => {
                eprintln!("paste-buff-gpui: hotkey manager unavailable: {err}");
                None
            }
        };
        if let Err(err) = signal::listen(move || {
            let _ = tx.unbounded_send(());
        }) {
            eprintln!("paste-buff-gpui: SIGUSR1 listener unavailable: {err}");
        }

        let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    focus: true,
                    show: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| PasteBuff::new(Arc::clone(&history), cx)),
            )
            .expect("open paste-buff window");

        // Wake only when something arrives; the waker hops back to the
        // main thread through GPUI's foreground executor.
        cx.spawn(async move |cx: &mut AsyncApp| {
            let _keep_alive = manager;
            while rx.next().await.is_some() {
                let _ = window.update(cx, |view, window, cx| view.toggle(window, cx));
            }
        })
        .detach();
    });
}
