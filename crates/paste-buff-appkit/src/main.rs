//! paste-buff, AppKit flavour: a non-activating NSPanel holding an
//! NSTableView and an NSTextView, summoned by Cmd+Shift+V.
//!
//! Everything is plain AppKit through objc2: no GPU pipeline of our own, no
//! glyph atlas, no runtime shaders. While hidden the panel is ordered out,
//! so the window server drops its backing store and the process holds only
//! the history and AppKit's own state.

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers as HotMods},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication,
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBorderType, NSBox, NSBoxType, NSColor,
    NSControlTextEditingDelegate,
    NSEvent, NSEventModifierFlags, NSEventType, NSFont, NSImage, NSImageScaling, NSImageView,
    NSPanel, NSPopUpMenuWindowLevel, NSResponder, NSScrollView, NSTableColumn,
    NSTableColumnResizingOptions, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewSelectionHighlightStyle, NSTableViewStyle, NSTextAlignment, NSTextField,
    NSTextView, NSUserInterfaceItemIdentification, NSView, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectNSThreadPerformAdditions,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use paste_buff_core::{
    app as macapp, clipboard,
    history::{self, file_name, human_bytes, Content, Entry, History},
    paste::{self, FrontApp},
    signal, watcher, DEFAULT_TTL, PASTE_DELAY, POLL_INTERVAL,
};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

const WIDTH: f64 = 760.0;
const HEIGHT: f64 = 460.0;
const LIST_WIDTH: f64 = 300.0;
const ROW_HEIGHT: f64 = 34.0;
const HEADER_HEIGHT: f64 = 48.0;
const FOOTER_HEIGHT: f64 = 40.0;
const INFO_HEIGHT: f64 = 160.0;
const PAD: f64 = 14.0;
const PREVIEW_CHARS: usize = 4000;

const BG: u32 = 0x1c1c1e;
const BORDER: u32 = 0x313135;

// Virtual key codes (Carbon `kVK_*`).
const KEY_RETURN: u16 = 36;
const KEY_KEYPAD_ENTER: u16 = 76;
const KEY_ESCAPE: u16 = 53;
const KEY_UP: u16 = 126;
const KEY_DOWN: u16 = 125;
const KEY_BACKSPACE: u16 = 51;
const KEY_FORWARD_DELETE: u16 = 117;
const KEY_Q: u16 = 12;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

fn rgb(hex: u32) -> Retained<NSColor> {
    let c = |shift: u32| ((hex >> shift) & 0xff) as f64 / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(c(16), c(8), c(0), 1.0)
}

/// Single-line label in the system font; `muted` picks the secondary color.
fn label(mtm: MainThreadMarker, text: &str, size: f64, muted: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    l.setTextColor(Some(&text_color(muted)));
    l
}

fn text_color(muted: bool) -> Retained<NSColor> {
    if muted {
        NSColor::secondaryLabelColor()
    } else {
        NSColor::labelColor()
    }
}

/// Multi-line label (newlines in the string become lines).
fn multiline(mtm: MainThreadMarker, size: f64, muted: bool) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
    l.setSelectable(false);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    l.setTextColor(Some(&text_color(muted)));
    l
}

fn separator(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSBox> {
    let b = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    b.setBoxType(NSBoxType::Separator);
    b
}

// ------------------------------------------------------------------ row view

struct RowIvars {
    icon: Retained<NSTextField>,
    title: Retained<NSTextField>,
    time: Retained<NSTextField>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowIvars]
    struct RowView;
);

impl RowView {
    fn id() -> &'static NSString {
        ns_string!("paste-buff-row")
    }

    const TIME_WIDTH: f64 = 54.0;
    const ICON_WIDTH: f64 = 22.0;

    fn new(mtm: MainThreadMarker, width: f64) -> Retained<Self> {
        let y = (ROW_HEIGHT - 18.0) / 2.0;
        let icon = label(mtm, "", 12.0, true);
        icon.setFrame(rect(10.0, y, Self::ICON_WIDTH, 18.0));
        let title = label(mtm, "", 13.0, false);
        let title_x = 10.0 + Self::ICON_WIDTH + 4.0;
        title.setFrame(rect(title_x, y, width - title_x - Self::TIME_WIDTH - 10.0, 18.0));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let time = label(mtm, "", 11.0, true);
        time.setAlignment(NSTextAlignment::Right);
        time.setFrame(rect(width - Self::TIME_WIDTH - 10.0, y, Self::TIME_WIDTH, 18.0));
        time.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);

        let this = Self::alloc(mtm).set_ivars(RowIvars { icon, title, time });
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, width, ROW_HEIGHT)] };
        this.setIdentifier(Some(Self::id()));
        this.addSubview(&this.ivars().icon);
        this.addSubview(&this.ivars().title);
        this.addSubview(&this.ivars().time);
        this
    }

    fn fill(&self, entry: &Entry, now: u64) {
        let icon = match &entry.content {
            Content::Image { .. } => "🖼",
            Content::Files { .. } => "📁",
            Content::Text if entry.is_url() => "🔗",
            Content::Text => "📄",
        };
        self.ivars().icon.setStringValue(&ns(icon));
        self.ivars().title.setStringValue(&ns(&entry.preview(80)));
        self.ivars()
            .time
            .setStringValue(&ns(&history::relative_time(entry.copied_at_ms, now)));
    }
}

// --------------------------------------------------------------------- panel

struct PanelIvars {
    history: Arc<Mutex<History>>,
    table: Retained<NSTableView>,
    header: Retained<NSTextField>,
    preview_scroll: Retained<NSScrollView>,
    preview: Retained<NSTextView>,
    image: Retained<NSImageView>,
    keys: Retained<NSTextField>,
    values: Retained<NSTextField>,
    target_label: Retained<NSTextField>,
    /// Snapshot of the filtered history, newest first.
    rows: RefCell<Vec<Entry>>,
    query: RefCell<String>,
    /// App that owned the keyboard when the panel was summoned.
    target: RefCell<Option<FrontApp>>,
    selected: Cell<usize>,
    shown: Cell<bool>,
    now: Cell<u64>,
}

define_class!(
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelIvars]
    struct PastePanel;

    impl PastePanel {
        /// Borderless windows refuse key status unless told otherwise.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// Like Raycast: losing key status (a click elsewhere, another
        /// panel taking over) dismisses the panel instead of stranding it.
        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            if self.ivars().shown.get() {
                self.hide();
            }
        }

        /// Every key press is ours, whichever subview is first responder.
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::KeyDown {
                self.key_down(event);
            } else {
                let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            }
        }

        /// Entry point for the hotkey and SIGUSR1 (via performSelectorOnMainThread).
        #[unsafe(method(toggle))]
        fn toggle(&self) {
            if self.ivars().shown.get() {
                self.hide();
            } else {
                self.show();
            }
        }
    }

    unsafe impl NSObjectProtocol for PastePanel {}

    unsafe impl NSTableViewDataSource for PastePanel {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSControlTextEditingDelegate for PastePanel {}

    unsafe impl NSTableViewDelegate for PastePanel {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.row_view(table, row).map(Retained::into_super)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_did_change(&self, _notification: &NSNotification) {
            let row = self.ivars().table.selectedRow();
            if let Ok(ix) = usize::try_from(row) {
                if ix != self.ivars().selected.get() {
                    self.ivars().selected.set(ix);
                    self.update_preview();
                }
            }
        }
    }
);

impl PastePanel {
    /// Reuse a row view from the table's pool or make a new one, then fill it.
    fn row_view(&self, table: &NSTableView, row: NSInteger) -> Option<Retained<RowView>> {
        let rows = self.ivars().rows.borrow();
        let entry = rows.get(usize::try_from(row).ok()?)?;
        let recycled = unsafe { table.makeViewWithIdentifier_owner(RowView::id(), None) }
            .and_then(|v| v.downcast::<RowView>().ok());
        let view = recycled
            .unwrap_or_else(|| RowView::new(MainThreadMarker::from(self), table.bounds().size.width));
        view.fill(entry, self.ivars().now.get());
        Some(view)
    }

    fn new(mtm: MainThreadMarker, history: Arc<Mutex<History>>) -> Retained<Self> {
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, WIDTH, HEIGHT));
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setCornerRadius(12.0);
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&rgb(BG).CGColor()));
            layer.setBorderWidth(1.0);
            layer.setBorderColor(Some(&rgb(BORDER).CGColor()));
        }

        // Header: the filter query.
        let header_y = HEIGHT - HEADER_HEIGHT;
        let header = label(mtm, "", 17.0, true);
        header.setFrame(rect(PAD, header_y + 13.0, WIDTH - 2.0 * PAD, 22.0));
        content.addSubview(&header);
        content.addSubview(&separator(mtm, rect(0.0, header_y - 1.0, WIDTH, 1.0)));

        // Left: the list.
        let list_pad = 6.0;
        let list_frame = rect(
            list_pad,
            FOOTER_HEIGHT + list_pad,
            LIST_WIDTH - 2.0 * list_pad,
            header_y - 1.0 - FOOTER_HEIGHT - 2.0 * list_pad,
        );
        let table = NSTableView::initWithFrame(
            NSTableView::alloc(mtm),
            rect(0.0, 0.0, list_frame.size.width, list_frame.size.height),
        );
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("entry"));
        column.setWidth(list_frame.size.width);
        column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setRowHeight(ROW_HEIGHT);
        table.setIntercellSpacing(NSSize::new(0.0, 0.0));
        table.setBackgroundColor(&NSColor::clearColor());
        table.setStyle(NSTableViewStyle::Plain);
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::Regular);
        table.setAllowsEmptySelection(true);
        let list_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), list_frame);
        list_scroll.setDocumentView(Some(&table));
        list_scroll.setHasVerticalScroller(true);
        list_scroll.setAutohidesScrollers(true);
        list_scroll.setDrawsBackground(false);
        list_scroll.setBorderType(NSBorderType::NoBorder);
        content.addSubview(&list_scroll);
        content.addSubview(&separator(
            mtm,
            rect(LIST_WIDTH, FOOTER_HEIGHT, 1.0, header_y - 1.0 - FOOTER_HEIGHT),
        ));

        // Right: preview body above the info pane.
        let right_x = LIST_WIDTH + 1.0;
        let right_w = WIDTH - right_x;
        let info_top = FOOTER_HEIGHT + INFO_HEIGHT;
        let body_frame = rect(right_x, info_top + 1.0, right_w, header_y - 1.0 - info_top - 1.0);
        content.addSubview(&separator(mtm, rect(right_x, info_top, right_w, 1.0)));

        let preview = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            rect(0.0, 0.0, body_frame.size.width, body_frame.size.height),
        );
        preview.setEditable(false);
        preview.setSelectable(true);
        preview.setDrawsBackground(false);
        let mono = NSFont::fontWithName_size(ns_string!("Menlo"), 12.0)
            .unwrap_or_else(|| NSFont::systemFontOfSize(12.0));
        preview.setFont(Some(&mono));
        preview.setTextColor(Some(&NSColor::labelColor()));
        preview.setTextContainerInset(NSSize::new(PAD, PAD));
        preview.setVerticallyResizable(true);
        preview.setHorizontallyResizable(false);
        preview.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        preview.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(container) = unsafe { preview.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        let preview_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), body_frame);
        preview_scroll.setDocumentView(Some(&preview));
        preview_scroll.setHasVerticalScroller(true);
        preview_scroll.setAutohidesScrollers(true);
        preview_scroll.setDrawsBackground(false);
        preview_scroll.setBorderType(NSBorderType::NoBorder);
        content.addSubview(&preview_scroll);

        let image = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            rect(
                body_frame.origin.x + PAD,
                body_frame.origin.y + PAD,
                body_frame.size.width - 2.0 * PAD,
                body_frame.size.height - 2.0 * PAD,
            ),
        );
        image.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        image.setHidden(true);
        content.addSubview(&image);

        // Info pane: two multi-line labels, keys left and values right.
        let info_title = label(mtm, "Information", 11.0, true);
        info_title.setFrame(rect(right_x + PAD, info_top - PAD - 14.0, 200.0, 14.0));
        content.addSubview(&info_title);
        let kv_h = INFO_HEIGHT - PAD - 14.0 - 4.0 - 4.0;
        let keys = multiline(mtm, 13.0, true);
        keys.setFrame(rect(right_x + PAD, FOOTER_HEIGHT + 4.0, 200.0, kv_h));
        content.addSubview(&keys);
        let values = multiline(mtm, 13.0, false);
        values.setAlignment(NSTextAlignment::Right);
        values.setFrame(rect(right_x + PAD, FOOTER_HEIGHT + 4.0, right_w - 2.0 * PAD, kv_h));
        content.addSubview(&values);

        // Footer.
        content.addSubview(&separator(mtm, rect(0.0, FOOTER_HEIGHT, WIDTH, 1.0)));
        let brand = label(mtm, "paste-buff · AppKit", 13.0, true);
        brand.setFrame(rect(PAD, 12.0, 300.0, 17.0));
        content.addSubview(&brand);
        let target_label = label(mtm, "", 13.0, false);
        target_label.setAlignment(NSTextAlignment::Right);
        target_label.setFrame(rect(WIDTH - PAD - 440.0, 12.0, 440.0, 17.0));
        content.addSubview(&target_label);

        let this = Self::alloc(mtm).set_ivars(PanelIvars {
            history,
            table,
            header,
            preview_scroll,
            preview,
            image,
            keys,
            values,
            target_label,
            rows: RefCell::new(Vec::new()),
            query: RefCell::new(String::new()),
            target: RefCell::new(None),
            selected: Cell::new(0),
            shown: Cell::new(false),
            now: Cell::new(history::now_ms()),
        });
        // NonactivatingPanel: the panel becomes key without activating this
        // app, so the paste target keeps its activation.
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let this: Retained<Self> = unsafe {
            msg_send![
                super(this),
                initWithContentRect: rect(0.0, 0.0, WIDTH, HEIGHT),
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        this.setContentView(Some(&content));
        this.setLevel(NSPopUpMenuWindowLevel);
        this.setOpaque(false);
        this.setBackgroundColor(Some(&NSColor::clearColor()));
        this.setHasShadow(true);
        this.setHidesOnDeactivate(false);
        this.setBecomesKeyOnlyIfNeeded(false);
        this.setMovable(false);
        this.setAnimationBehavior(NSWindowAnimationBehavior::None);
        this.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Transient,
        );
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
            this.setAppearance(Some(&dark));
        }
        let ivars = this.ivars();
        unsafe {
            ivars.table.setDataSource(Some(ProtocolObject::from_ref(&*this)));
            ivars.table.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        }
        // No reload here: filling the preview before the first show would
        // decode the newest image while the panel is still hidden.
        this.refresh();
        this.update_header();
        this
    }

    /// Re-run the filter against the live history.
    fn refresh(&self) {
        let iv = self.ivars();
        iv.now.set(history::now_ms());
        let rows: Vec<Entry> = {
            let h = iv.history.lock().unwrap_or_else(|e| e.into_inner());
            h.search(&iv.query.borrow()).into_iter().cloned().collect()
        };
        let len = rows.len();
        *iv.rows.borrow_mut() = rows;
        iv.selected.set(iv.selected.get().min(len.saturating_sub(1)));
    }

    /// Push the rows snapshot into the table and re-apply the selection.
    fn reload(&self) {
        self.ivars().table.reloadData();
        self.apply_selection();
    }

    fn apply_selection(&self) {
        let iv = self.ivars();
        if iv.rows.borrow().is_empty() {
            unsafe { iv.table.deselectAll(None) };
        } else {
            let ix = iv.selected.get();
            iv.table
                .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(ix), false);
            iv.table.scrollRowToVisible(ix as NSInteger);
        }
        self.update_preview();
    }

    fn select(&self, index: usize) {
        let iv = self.ivars();
        let len = iv.rows.borrow().len();
        if len == 0 {
            iv.selected.set(0);
            return;
        }
        iv.selected.set(index.min(len - 1));
        self.apply_selection();
    }

    fn update_header(&self) {
        let iv = self.ivars();
        let query = iv.query.borrow();
        if query.is_empty() {
            iv.header.setStringValue(ns_string!("Type to filter entries…"));
            iv.header.setTextColor(Some(&NSColor::secondaryLabelColor()));
        } else {
            iv.header.setStringValue(&ns(&format!("{query}▏")));
            iv.header.setTextColor(Some(&NSColor::labelColor()));
        }
    }

    fn update_preview(&self) {
        let iv = self.ivars();
        let rows = iv.rows.borrow();
        let now = iv.now.get();
        let Some(e) = rows.get(iv.selected.get()) else {
            iv.preview.setString(ns_string!("Nothing copied in the last 24 hours"));
            iv.preview_scroll.setHidden(false);
            iv.image.setImage(None);
            iv.image.setHidden(true);
            iv.keys.setStringValue(ns_string!(""));
            iv.values.setStringValue(ns_string!(""));
            return;
        };

        let mut keys = vec!["Source", "Content type"];
        let mut values = vec![
            e.source_app.clone().unwrap_or_else(|| "Unknown".into()),
            e.kind_label().to_string(),
        ];
        match &e.content {
            Content::Text => {
                let text: String = e.text.chars().take(PREVIEW_CHARS).collect();
                self.show_text(&text);
                keys.extend(["Characters", "Words", "Lines"]);
                values.extend([
                    e.char_count().to_string(),
                    e.word_count().to_string(),
                    e.line_count().to_string(),
                ]);
            }
            Content::Files { paths } => {
                let text = paths
                    .iter()
                    .take(200)
                    .map(|p| format!("{}\n    {}", file_name(p), p.display()))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.show_text(&text);
                let total: u64 = paths
                    .iter()
                    .filter_map(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .sum();
                keys.extend(["Files", "Size"]);
                values.extend([paths.len().to_string(), human_bytes(total)]);
            }
            Content::Image { file, width, height, bytes } => {
                let image = NSImage::initWithContentsOfFile(
                    NSImage::alloc(),
                    &ns(&file.display().to_string()),
                );
                iv.image.setImage(image.as_deref());
                iv.image.setHidden(false);
                iv.preview_scroll.setHidden(true);
                keys.extend(["Dimensions", "Size"]);
                values.extend([format!("{width} × {height}"), human_bytes(*bytes)]);
            }
        }
        keys.extend(["Times copied", "Copied"]);
        values.extend([
            e.times_copied.to_string(),
            format!(
                "{} · {} ago",
                history::age_label(e.copied_at_ms, now),
                history::relative_time(e.copied_at_ms, now)
            ),
        ]);
        iv.keys.setStringValue(&ns(&keys.join("\n")));
        iv.values.setStringValue(&ns(&values.join("\n")));
    }

    fn show_text(&self, text: &str) {
        let iv = self.ivars();
        iv.image.setImage(None);
        iv.image.setHidden(true);
        iv.preview.setString(&ns(text));
        iv.preview.scrollRangeToVisible(NSRange::new(0, 0));
        iv.preview_scroll.setHidden(false);
    }

    fn update_target_label(&self) {
        let iv = self.ivars();
        let target = match &*iv.target.borrow() {
            Some(t) if !t.name.is_empty() => format!("Paste to {}", t.name),
            _ => "Paste".to_string(),
        };
        iv.target_label
            .setStringValue(&ns(&format!("{target}  ⏎      Delete  ⌘⌫")));
    }

    fn show(&self) {
        let iv = self.ivars();
        // Capture the paste target before we take the keyboard.
        *iv.target.borrow_mut() = paste::frontmost_app();
        iv.query.borrow_mut().clear();
        iv.selected.set(0);
        self.refresh();
        self.update_header();
        self.update_target_label();
        self.reload();
        iv.shown.set(true);
        self.center();
        self.makeKeyAndOrderFront(None);
        eprintln!(
            "paste-buff-appkit: shown (target: {})",
            iv.target
                .borrow()
                .as_ref()
                .map(|t| t.name.as_str())
                .unwrap_or("none")
        );
    }

    fn hide(&self) {
        let iv = self.ivars();
        iv.shown.set(false);
        // Ordering out releases the window's backing store; the target app
        // never lost activation, so there is nothing to hand back.
        self.orderOut(None);
        iv.image.setImage(None);
        eprintln!("paste-buff-appkit: hidden");
    }

    fn choose(&self) {
        let iv = self.ivars();
        let Some(entry) = iv.rows.borrow().get(iv.selected.get()).cloned() else {
            return;
        };
        let target = iv.target.borrow().clone();
        self.hide();
        if clipboard::write(&entry) {
            paste::paste_into(target.as_ref(), PASTE_DELAY);
        } else {
            eprintln!("paste-buff-appkit: could not put entry {} on the pasteboard", entry.id);
        }
    }

    fn remove_selected(&self) {
        let iv = self.ivars();
        let Some(id) = iv.rows.borrow().get(iv.selected.get()).map(|e| e.id) else {
            return;
        };
        {
            let mut h = iv.history.lock().unwrap_or_else(|e| e.into_inner());
            if h.remove(id) {
                let _ = h.save();
            }
        }
        self.refresh();
        self.reload();
    }

    fn set_query(&self, query: String) {
        let iv = self.ivars();
        *iv.query.borrow_mut() = query;
        iv.selected.set(0);
        self.refresh();
        self.update_header();
        self.reload();
    }

    fn key_down(&self, event: &NSEvent) {
        let iv = self.ivars();
        let mods = event.modifierFlags();
        let cmd = mods.contains(NSEventModifierFlags::Command);
        let ctrl = mods.contains(NSEventModifierFlags::Control);
        match event.keyCode() {
            KEY_ESCAPE => self.hide(),
            KEY_RETURN | KEY_KEYPAD_ENTER => self.choose(),
            KEY_UP => self.select(iv.selected.get().saturating_sub(1)),
            KEY_DOWN => self.select(iv.selected.get() + 1),
            KEY_BACKSPACE if cmd => self.remove_selected(),
            KEY_FORWARD_DELETE => self.remove_selected(),
            KEY_BACKSPACE => {
                let mut q = iv.query.borrow().clone();
                q.pop();
                self.set_query(q);
            }
            KEY_Q if cmd => NSApplication::sharedApplication(MainThreadMarker::from(self)).terminate(None),
            _ => {
                if cmd || ctrl {
                    return;
                }
                let Some(chars) = event.characters() else { return };
                let typed = chars.to_string();
                // Function keys arrive as private-use code points; skip them.
                let printable = !typed.is_empty()
                    && typed
                        .chars()
                        .all(|c| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c));
                if printable {
                    let q = format!("{}{}", iv.query.borrow(), typed);
                    self.set_query(q);
                }
            }
        }
    }
}

/// Lets any thread ask the panel to toggle. Only `performSelectorOnMainThread`
/// is ever called through it, which is thread-safe by contract; the panel
/// itself is only touched on the main thread.
#[derive(Clone, Copy)]
struct MainThreadToggle(*const NSObject);

unsafe impl Send for MainThreadToggle {}
unsafe impl Sync for MainThreadToggle {}

impl MainThreadToggle {
    fn new(panel: &PastePanel) -> Self {
        let obj: &NSObject = panel;
        Self(obj)
    }

    fn request(&self) {
        // SAFETY: the panel is retained by `main` for the whole process.
        let obj: &NSObject = unsafe { &*self.0 };
        unsafe { obj.performSelectorOnMainThread_withObject_waitUntilDone(sel!(toggle), None, false) };
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

    let mtm = MainThreadMarker::new().expect("paste-buff must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    macapp::set_accessory_policy();
    if !paste::accessibility_trusted_prompting() {
        eprintln!(
            "paste-buff-appkit: no Accessibility permission; Enter will copy \
             the entry but cannot send Cmd+V to the target app."
        );
    }

    let panel = PastePanel::new(mtm, history);
    let toggle = MainThreadToggle::new(&panel);

    // Carbon hotkey. The handler runs inside the Carbon event dispatcher on
    // the main thread, but queueing through performSelectorOnMainThread keeps
    // one code path for the hotkey and SIGUSR1.
    let hotkey = HotKey::new(Some(HotMods::SUPER | HotMods::SHIFT), Code::KeyV);
    let hotkey_id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed && event.id == hotkey_id {
            toggle.request();
        }
    }));
    let _manager = match GlobalHotKeyManager::new() {
        Ok(m) => {
            if let Err(err) = m.register(hotkey) {
                eprintln!(
                    "paste-buff-appkit: could not register Cmd+Shift+V ({err}). \
                     Another app (Raycast?) may own it; SIGUSR1 still toggles the window."
                );
            }
            Some(m)
        }
        Err(err) => {
            eprintln!("paste-buff-appkit: hotkey manager unavailable: {err}");
            None
        }
    };
    if let Err(err) = signal::listen(move || toggle.request()) {
        eprintln!("paste-buff-appkit: SIGUSR1 listener unavailable: {err}");
    }

    // `run` never returns; `panel` and `_manager` live as long as the process.
    app.run();
    drop(panel);
}
