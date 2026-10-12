# paste-buff

A clipboard-history popup for macOS, built to replace Raycast's Clipboard
History for one job: press **⌘⇧V**, pick something you copied in the last
24 hours, and it gets pasted into the app you were using.

The repo started as a bake-off: the same app on several UI stacks, measured
against Raycast with memory as the headline number. Round one (Rust + Tauri
versus Rust + GPUI, `results/baseline-2026-10-10/`) went to GPUI by a wide
margin. Round two (`results/appkit-2026-10-11/`, summarised under
[Results](#results)) put plain AppKit against GPUI and AppKit idles at half
the footprint and settles at a third, so **the AppKit frontend is now the
one being developed and released**. GPUI stays in the tree and in the
default build as the benchmark comparison; Tauri stays for reference only
and is not built by default.

Reference UI: `docs/raycast-clipboard-history.png` (Raycast's panel).

## Layout

```
crates/
  paste-buff-core/   history store (24h TTL, JSON on disk), pasteboard watcher,
                     paste-into-previous-app, AppKit helpers, SIGUSR1 toggle hook
  paste-buff-appkit/ the released frontend: AppKit via objc2, an NSPanel subclass
                     with NSTableView rows and an NSTextView/NSImageView preview,
                     ordered out while hidden
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
removes an entry, Esc or clicking elsewhere hides, ⌘Q quits.

## Install

Prebuilt binaries for Apple Silicon and Intel Macs are attached to each
[GitHub release](https://github.com/jjroush/paste-buff/releases). The install
script picks the right one, verifies its SHA-256, and puts `paste-buff` in
`~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/jjroush/paste-buff/main/scripts/install.sh | sh
```

Set `PASTE_BUFF_VERSION=v0.1.0` to pin a release or `PASTE_BUFF_BIN_DIR` to
install elsewhere. While the repository is private, GitHub needs a token to
serve both the script and the assets, so use the `gh` CLI instead:

```sh
gh release download --repo jjroush/paste-buff --pattern 'install.sh' -O - | GITHUB_TOKEN=$(gh auth token) sh
# or grab the tarball directly
gh release download --repo jjroush/paste-buff --pattern "paste-buff-$(uname -m | sed s/arm64/aarch64/)-apple-darwin.tar.gz"
```

The script also registers a launchd agent (`~/Library/LaunchAgents/
io.roush.paste-buff.plist`) that starts `paste-buff` right away and at every
login, restarts it if it crashes, and logs to `~/Library/Logs/paste-buff.log`.
Quitting with ⌘Q inside the panel stops it until the next login or
reinstall. Set `PASTE_BUFF_NO_AGENT=1` to get just the binary, and run
`install.sh --uninstall` to remove the agent and the binary (history stays).

On first launch macOS asks for Accessibility access so paste-buff can press
⌘V in the target app. Raycast's Clipboard History also uses ⌘⇧V; turn that
hotkey off in Raycast or the two will fight over the panel.

The binary is ad-hoc signed, not notarised. Files fetched with `curl` carry
no quarantine flag so Gatekeeper leaves them alone; if you download the
tarball with a browser instead, clear it with
`xattr -d com.apple.quarantine ~/.local/bin/paste-buff`.

Releases are cut by pushing a tag that matches the workspace version in
`Cargo.toml`; `.github/workflows/release.yml` builds both targets and
publishes the tarballs plus a `SHA256SUMS` file:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

## Build and run

```sh
cargo build --release
./target/release/paste-buff-appkit
# the GPUI frontend, kept for comparison:
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
  both will fire. Whichever panel gets key status last wins the keyboard,
  and paste-buff hides itself when it loses key status, so with both bound
  you will mostly see Raycast. Change or disable Raycast's Clipboard History
  hotkey, or quit Raycast.
- **Ad-hoc signatures and Accessibility**: macOS ties the Accessibility
  grant to the binary's code signature. Every new build has a new ad-hoc
  signature, so after an upgrade the existing switch in System Settings may
  stop working; toggle it off and on (or remove and re-add the entry). A
  Developer ID signature would make the grant survive upgrades.
- **GPUI shaders** (GPUI frontend only): the GPUI crate is built with the `runtime_shaders`
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
  ./target/release/paste-buff-bench run --launch target/release/paste-buff-appkit --out results/appkit

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

Round two, 2026-10-11: AppKit against GPUI, both re-run the same evening.
Apple M2 Max, 32 GB, macOS 27.0.1, rustc 1.98.0, release builds. Protocol:
20 s idle → 20 × (open, hold 1 s, close, wait 1 s) → 20 s cooldown, sampled
every 0.5 s, launched by the harness and toggled with SIGUSR1, each run from
a fresh copy of the seed (202 entries with a 1600×1000 PNG newest, or 200
text entries with `--no-media`). Raycast and Tauri rows are round one's,
text-only seed, same protocol.

| target | procs | idle footprint | peak (panel up) | after 20 cycles | cycling CPU | binary |
|---|---:|---:|---:|---:|---:|---:|
| paste-buff-appkit, text-only seed | 1 | 14.6 MB | 17.6 MB | 17.3 MB | 4.4% | 0.7 MB |
| paste-buff-appkit, image newest | 1 | 14.7 MB | 17.9 MB | 17.8 MB | 3.9% | 0.7 MB |
| paste-buff-gpui, text-only seed | 1 | 30.1 MB | 51.0 MB | 45.0 MB | 2.2% | 6.0 MB |
| paste-buff-gpui, image newest | 1 | 35.7 MB | 50.4 MB | 45.7 MB | 2.9% | 6.0 MB |
| paste-buff-tauri (round one) | 4 | 66.3 MB | 111.6 MB | 73.5 MB | 5.2% | 5.5 MB |
| Raycast (round one) | 6 | 116.4 MB | 131.1 MB | 126.8 MB | 7.0% | – |

What the numbers say:

- **AppKit idles at half of GPUI and settles at a third.** The AppKit panel
  is ordered out when hidden, so WindowServer drops its backing store. GPUI
  keeps the window, three Retina drawables (16 MB of IOSurface) and the
  Metal renderer alive between shows, which is the 15 MB it never gives back
  after the first open.
- **AppKit with the panel up uses less than GPUI hidden.** Part of that is
  accounting: an AppKit window's pixels live in WindowServer, roughly 5.5 MB
  for this panel at 2×, and are charged there rather than to paste-buff,
  whereas GPUI's IOSurfaces are charged to paste-buff. Adding it back still
  leaves AppKit's panel-up total under GPUI's hidden baseline.
- **Images cost AppKit nothing at idle.** NSImage decodes on the first show,
  not at startup, and the image view is cleared on hide. GPUI paints the
  hidden window once at startup and caches the decoded image.
- **Cycling CPU is higher for AppKit** (4% against 2–3%) because each show
  rebuilds the table and lays out the text view; GPUI's element tree is
  warm. Idle CPU is at the noise floor for every frontend.
- **The binary is 0.7 MB against 6.0 MB.** No shader compiler, image
  decoders or font shaper of our own; everything comes from the OS.
- **Against Raycast**, AppKit idles at an eighth of its footprint in one
  process instead of six. Raycast is a whole launcher, so this is the cost
  of its clipboard history being bundled, not a like-for-like comparison.

Raw samples, summaries and stderr logs for round two are in
`results/appkit-2026-10-11/`, with its own `RESULTS.md`. Round one's GPUI
runs are in `results/RESULTS.md` and the original three-way comparison in
`results/baseline-2026-10-10/`. Regenerate any table with
`paste-buff-bench report <dir>/*.json`.

## Design notes

- One shared core keeps the comparison honest: the frontends differ only in
  windowing, rendering and hotkey plumbing.
- The AppKit frontend is one `NSPanel` subclass that owns all state. It is
  borderless and non-activating, overrides `canBecomeKeyWindow`, takes every
  key press in `sendEvent:` so it does not matter which subview is first
  responder, and hides itself in `resignKeyWindow` so a click elsewhere
  dismisses it. Dismiss is `orderOut:`; the app never activated, so the
  target still has focus and the window's backing store is released.
- The GPUI window is a `WindowKind::PopUp`, which GPUI maps to a
  non-activating `NSPanel`; the Tauri window is a regular undecorated
  `NSWindow`. Both hide the whole app on dismiss (`[NSApp hide:]`) because
  that reliably returns focus to the previous app, and both keep the window
  alive while hidden.
- Tauri's UI is plain HTML/CSS/JS with no bundler so the frontend cost is
  WebKit itself, not a framework.
- Neither Rust frontend has a text-input widget; the filter box is a string
  fed by key-down events, which is all a filter needs.

## Candidates for round three

- A small Swift/SwiftUI app as the "what Apple would do" baseline. It can
  ship as a bare executable like the Rust builds; expect it to land between
  AppKit and GPUI.
- `egui`/`eframe` or `iced`: other Rust-native renderers, likely to share
  GPUI's kept-warm GPU surface problem.
