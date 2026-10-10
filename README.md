# paste-buff

A clipboard-history popup for macOS, built to replace Raycast's Clipboard
History for one job: press **⌘⇧V**, pick something you copied in the last
24 hours, and it gets pasted into the app you were using.

The repo started as a bake-off: the same app on several UI stacks, measured
against Raycast with memory as the headline number. Round one (Rust + Tauri
versus Rust + GPUI, see `results/baseline-2026-10-10/`) went to GPUI by a
wide margin, so **GPUI is now the frontend being developed**. The Tauri
frontend stays in the tree for reference but is not built by default and
does not get new features.

Reference UI: `docs/raycast-clipboard-history.png` (Raycast's panel).

## Layout

```
crates/
  paste-buff-core/   history store (24h TTL, JSON on disk), pasteboard watcher,
                     paste-into-previous-app, AppKit helpers, SIGUSR1 toggle hook
  paste-buff-gpui/   GPUI 0.2 frontend: non-activating NSPanel, uniform_list rows
  paste-buff-tauri/  reference only: Tauri 2 frontend (text clips only)
  paste-buff-bench/  benchmark harness (footprint / RSS / CPU per process tree)
scripts/
  seed-history.py    writes a deterministic history file so frontends start equal
results/             benchmark outputs (gitignored CSV/JSON; keep the .md summaries)
docs/
```

Shared behaviour (all frontends):

- Polls `NSPasteboard.generalPasteboard().changeCount` every 250 ms and records
  text, file and image clips. Files copied in Finder are stored as their
  paths and pasted back as real file URLs; images (screenshots, "Copy Image")
  are normalised to PNG under `~/Library/Application Support/paste-buff/images/`
  and deleted with their entry. When a clip carries both text and an image
  (spreadsheet cells, rich text) the text wins. Clips flagged
  `org.nspasteboard.ConcealedType` or `TransientType` (password managers) are
  skipped, as are blank clips, text over 1 MiB and images over 32 MiB.
- Re-copying existing text moves it to the top and bumps "times copied".
- History lives at `~/Library/Application Support/paste-buff/history.json`,
  written atomically. Entries older than 24 h are pruned on load and every
  minute. Override the path with `PASTE_BUFF_HISTORY=/path/to/file.json`.
- On ⌘⇧V the app remembers the frontmost app, shows its panel, and on Enter
  writes the entry to the pasteboard, hides itself, re-activates that app and
  posts ⌘V.
- `SIGUSR1` toggles the panel. The benchmark harness uses this so it can
  exercise the UI without the Accessibility permission that synthetic
  keystrokes need.

Keys inside the panel: type to filter, ↑/↓ move, ⏎ paste, ⌘⌫ or Delete
removes an entry, Esc hides, ⌘Q quits.

## Build and run

```sh
cargo build --release
./target/release/paste-buff-gpui
# reference frontend, text only:
cargo build --release -p paste-buff-tauri && ./target/release/paste-buff-tauri
```

The app runs as an accessory (no Dock icon) and stays hidden until ⌘⇧V.

Things macOS will ask for:

- **Accessibility** for whatever launches the app (your terminal, or the
  binary itself once it is bundled). Without it the app still copies the
  entry to the pasteboard but cannot send ⌘V, and logs a one-line warning.
- **Hotkey conflicts**: Raycast and paste-buff can both register ⌘⇧V and
  both will fire. Change or disable Raycast's Clipboard History hotkey while
  testing, or quit Raycast.
- **GPUI shaders**: the GPUI crate is built with the `runtime_shaders`
  feature because Xcode 26 ships its Metal compiler as a separate download.
  If you install it (`xcodebuild -downloadComponent MetalToolchain`) you can
  drop the feature in `crates/paste-buff-gpui/Cargo.toml` to precompile the
  shaders and shave a little off startup.

Run the core's tests with `cargo test -p paste-buff-core`.

## Benchmark harness

```sh
# Seed a history: 200 text entries plus a screenshot-sized PNG and a Finder
# file clip as the newest entries (add --no-media for text only)
python3 -I scripts/seed-history.py results/seed-history.json --entries 200

# Raycast (already running): attach by name, drive it through its deeplink
./target/release/paste-buff-bench run --name Raycast --toggle raycast --out results/raycast

# paste-buff: launch, toggle with SIGUSR1, kill when done
PASTE_BUFF_HISTORY=results/seed-history.json \
  ./target/release/paste-buff-bench run --launch target/release/paste-buff-gpui --out results/gpui

# Side-by-side table
./target/release/paste-buff-bench report results/*.json
```

Each run is: `--warmup` (default 3 s, launch mode only) → `--idle` 10 s →
`--cycles` 10 × (open, hold 1 s, close, wait 1 s) → `--cooldown` 10 s. Samples
are taken every `--interval` 0.5 s and written to `<out>.csv`; a summary
goes to `<out>.json`. `paste-buff-bench ps --name Raycast` shows what would
be measured.

What the columns mean:

- **footprint** is `ri_phys_footprint` from `proc_pid_rusage`, the number
  Activity Monitor shows as "Memory" and what the kernel uses for memory
  pressure. **RSS** is reported too but overstates shared framework pages.
- **procs** is the whole process tree. Helpers are attributed with the same
  responsible-pid relationship Activity Monitor uses for its hierarchy view
  (`responsibility_get_pid_responsible_for_pid`), plus parent pids. Raycast
  comes with three WebKit XPC helpers; so does a Tauri app. An app launched
  from a terminal is not its own responsible process, so in `--launch` mode
  any WebKit XPC process that appears after launch is attributed to it.
- **idle** is the average while hidden, **peak (cycling)** the maximum while
  the panel was being opened and closed, **after cycles** the average once
  it was hidden again (a gap between idle and after is memory the UI keeps).
- **CPU** is user+system time over wall time, summed over the tree.

Toggle modes: `signal` (SIGUSR1, paste-buff frontends), `raycast` (opens
`raycast://extensions/raycast/clipboard-history/clipboard-history`, then
re-activates the previous app so Raycast dismisses), `keys` (posts ⌘⇧V and
Esc; needs Accessibility), `none` (idle only).

## Results

`results/RESULTS.md` is the latest run on this machine;
`results/baseline-2026-10-10/` is the original three-way comparison that
picked GPUI.

## Design notes

- One shared core keeps the comparison honest: the frontends differ only in
  windowing, rendering and hotkey plumbing.
- The GPUI window is a `WindowKind::PopUp`, which GPUI maps to a
  non-activating `NSPanel`; the Tauri window is a regular undecorated
  `NSWindow`. Both hide the whole app on dismiss (`[NSApp hide:]`) because
  that reliably returns focus to the previous app.
- Tauri's UI is plain HTML/CSS/JS with no bundler so the frontend cost is
  WebKit itself, not a framework.
- The GPUI frontend has no text-input widget; the filter box is a string fed
  by key-down events, which is all a filter needs.

## Candidates for round two

- AppKit directly through `objc2-app-kit` (NSPanel + NSTableView): the
  floor for a native Rust app.
- `egui`/`eframe` or `iced`: other Rust-native renderers.
- A small Swift/SwiftUI app as the "what Apple would do" baseline.
