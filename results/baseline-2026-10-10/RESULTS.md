# Baseline — 2026-10-10 (text-only build, all three targets)

**Machine:** Apple M2 Max, 32 GB, macOS 26.5.1, rustc 1.98.0. Release builds
(thin LTO, `codegen-units = 1`, stripped). GPUI built with `runtime_shaders`.

**Protocol (per target):** 20 s idle → 20 × (open, hold 1 s, close, wait 1 s)
→ 20 s cooldown, sampled every 0.5 s. The paste-buff frontends were launched
by the harness with an identical 200-entry seeded history
(`scripts/seed-history.py`) and toggled with SIGUSR1. Raycast was attached by
name and driven through its Clipboard History deeplink, closed by
re-activating the previous app.

| target | procs | idle footprint | peak (cycling) | after cycles | idle RSS | idle CPU | cycling CPU | binary |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| paste-buff-gpui | 1 | 30.6 MB | 47.9 MB | 43.9 MB | 54.1 MB | 0.0% | 1.7% | 4.0 MB |
| paste-buff-tauri | 4 | 66.3 MB | 111.6 MB | 73.5 MB | 191.4 MB | 0.1% | 5.2% | 5.5 MB |
| Raycast | 6 | 116.4 MB | 131.1 MB | 126.8 MB | 239.1 MB | 0.1% | 7.0% | – |

Process trees:

- paste-buff-gpui: `paste-buff-gpui`
- paste-buff-tauri: `paste-buff-tauri`, `com.apple.WebKit.WebContent`,
  `com.apple.WebKit.Networking`, `com.apple.WebKit.GPU`
- Raycast: `Raycast`, the same three WebKit helpers, and
  `com.apple.audio.SandboxHelper` (a sixth process appeared in one sample)

Raw samples: `raycast.csv`, `tauri.csv`, `gpui.csv`; summaries in the
matching `.json` files. Regenerate the table with
`paste-buff-bench report results/*.json`.

## Reading it

- **GPUI** is one process at about 31 MB hidden and 48 MB with the panel up.
  It keeps roughly 12 MB after the first show (glyph atlas, Metal pipeline,
  runtime-compiled shaders) and then stays flat across 20 cycles.
- **Tauri** costs the WebKit process trio even while hidden: 66 MB idle,
  110 MB peak, settling at 73 MB. Its RSS (189 MB) is almost three times its
  footprint because every WebKit process maps the same shared framework pages;
  footprint is the number to compare.
- **Raycast** idles at 116 MB across six processes and peaks at 131 MB. Its
  cycling CPU (7%) is the highest, but it is also rendering a richer UI.
- **Idle CPU**: both frontends sit at the noise floor. GPUI's idle samples
  read 0.01–0.03% (one process); Tauri's 0.1% is spread over four. The only
  periodic work left is the 250 ms pasteboard `changeCount` poll, which
  NSPasteboard gives no notification for. Hotkey presses arrive through the
  hotkey crate's callback into a channel the GPUI task sleeps on, and
  SIGUSR1 through a self-pipe read by a blocked thread. An earlier build
  that polled both from a 50 ms timer measured 0.3% idle on GPUI.

## Caveats

- Raycast is a launcher with extensions, AI, window management and more; its
  baseline is not just clipboard history. It had been restarted about four
  minutes before this run (it read 240 MB right after launch and settled to
  ~122 MB by the idle phase), and its history holds real data of unknown
  size while paste-buff had 200 seeded entries.
- The Raycast panel could not be visually confirmed from this session (the
  terminal lacks Screen Recording permission); the CPU and memory deltas
  during cycling show the deeplink was exercising its UI.
- The terminal also lacks Accessibility, so the ⌘V paste step was not
  exercised in these runs. It does not affect the memory numbers.
- One run per target. Re-run with `--cycles 100 --cooldown 60` to look for
  slow leaks before trusting the "after cycles" column.
