# Round two — 2026-10-11, AppKit versus GPUI

**Machine:** Apple M2 Max, 32 GB, macOS 27.0.1, rustc 1.98.0. Release builds
(thin LTO, `codegen-units = 1`, stripped), GPUI built with `runtime_shaders`.

**Protocol:** same as round one. 20 s idle → 20 × (open, hold 1 s, close,
wait 1 s) → 20 s cooldown, sampled every 0.5 s, launched by the harness and
toggled with SIGUSR1. Each run starts from a fresh copy of the seed
(`scripts/seed-history.py`): 202 entries with a 1600×1000 PNG newest, or 200
text entries with `--no-media`. GPUI was re-run the same evening so both rows
share the OS build and background load.

| target | procs | idle footprint | peak (cycling) | after cycles | idle RSS | idle CPU | cycling CPU | binary |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| paste-buff-appkit, text-only seed | 1 | 14.6 MB | 17.6 MB | 17.3 MB | 47.7 MB | 0.0% | 4.4% | 0.7 MB |
| paste-buff-appkit, image newest | 1 | 14.7 MB | 17.9 MB | 17.8 MB | 48.0 MB | 0.0% | 3.9% | 0.7 MB |
| paste-buff-gpui, text-only seed | 1 | 30.1 MB | 51.0 MB | 45.0 MB | 48.8 MB | 0.0% | 2.2% | 6.0 MB |
| paste-buff-gpui, image newest | 1 | 35.7 MB | 50.4 MB | 45.7 MB | 70.4 MB | 0.8% | 2.9% | 6.0 MB |

Raw samples: `appkit.csv`, `appkit-nomedia.csv`, `gpui.csv`,
`gpui-nomedia.csv`; summaries in the matching `.json`, stderr in `.log`.
Regenerate the table with `paste-buff-bench report results/appkit-2026-10-11/*.json`.

## Reading it

- **AppKit idles at half of GPUI and settles at a third.** 14.6 MB hidden
  against 30.1 MB, and 17.3 MB after twenty cycles against 45.0 MB. The
  AppKit panel is ordered out when hidden, so its backing store goes away;
  GPUI keeps the window, its three Retina drawables (16 MB of IOSurface) and
  the Metal renderer alive between shows.
- **Images cost AppKit nothing at idle.** NSImage decodes on the first show,
  not at startup, and the image view is cleared on hide. The remaining
  3 MB after cycles is AppKit's text system and font caches warming up.
- **Peak with the panel up is 17.9 MB, under GPUI's idle.** Part of that is
  accounting: an AppKit window's pixels live in WindowServer, so roughly
  5.5 MB of backing for this panel at 2× is charged to WindowServer rather
  than to paste-buff. GPUI's IOSurfaces are charged to paste-buff. Even
  adding that back, AppKit's panel-up total is below GPUI's hidden baseline.
- **Cycling CPU is higher for AppKit** (4% against 2–3%). Each show rebuilds
  the table and lays out the text view from scratch; GPUI's element tree is
  warm. Idle CPU is at the noise floor for both.
- **The binary is 0.7 MB against 6.0 MB.** No shader compiler, image
  decoders or font shaper of our own; everything comes from the OS.

## Caveats

- Both frontends logged a few more show/hide pairs than the twenty the
  harness requested (22 for the AppKit text run, 23 for the GPUI image run),
  so something on this desktop occasionally took key status away and
  dismissed the panel, which flips the open and close phases for a cycle.
  Peak and after-cycle numbers are unaffected because every cycle still
  shows the panel once.
- The AppKit frontend reproduces the GPUI panel feature for feature (filter,
  arrows, Enter, ⌘⌫, Esc, click-away dismiss, text/file/image previews) but
  its row styling uses the system selection colour rather than GPUI's
  custom one.
