# Results — 2026-10-10, GPUI with file and image clips

**Machine:** Apple M2 Max, 32 GB, macOS 26.5.1, rustc 1.98.0. Release build
(thin LTO, `codegen-units = 1`, stripped), GPUI built with `runtime_shaders`.

**Protocol:** 20 s idle → 20 × (open, hold 1 s, close, wait 1 s) → 20 s
cooldown, sampled every 0.5 s, launched by the harness and toggled with
SIGUSR1. The default seed (`scripts/seed-history.py`) now has 202 entries:
a 1600×1000 PNG as the newest entry, a two-file Finder clip, then 200 text
entries. The text-only row used `--no-media`. Raycast and Tauri rows are
copied from `baseline-2026-10-10/` (text-only build, same protocol) for
reference; Tauri is no longer developed.

| target | procs | idle footprint | peak (cycling) | after cycles | idle RSS | idle CPU | cycling CPU | binary |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| paste-buff-gpui, text-only seed | 1 | 30.8 MB | 48.3 MB | 44.3 MB | 54.7 MB | 0.0% | 2.0% | 6.0 MB |
| paste-buff-gpui, image newest | 1 | 51.7 MB | 63.5 MB | 57.0 MB | 74.9 MB | 0.0% | 2.1% | 6.0 MB |
| paste-buff-tauri (baseline) | 4 | 66.3 MB | 111.6 MB | 73.5 MB | 191.4 MB | 0.1% | 5.2% | 5.5 MB |
| Raycast (baseline) | 6 | 116.4 MB | 131.1 MB | 126.8 MB | 239.1 MB | 0.1% | 7.0% | – |

Raw samples: `gpui.csv` (image seed), `gpui-nomedia.csv` (text-only seed),
summaries in the matching `.json`.

## Reading it

- **File and image support cost nothing at idle by itself.** The binary grew
  from 4.0 MB to 6.0 MB (PNG and TIFF decoders behind GPUI's `img`), but with
  a text-only history the app idles at 30.8 MB, the same as before.
- **A decoded image is the new dominant cost.** With a 1600×1000 screenshot
  as the newest entry the app idles at 51.7 MB. GPUI paints the hidden
  window's first frame at startup, so the preview image is decoded (6.4 MB
  RGBA plus the Metal texture and cache bookkeeping) before the panel is ever
  shown, and stays cached. Peak with the panel open is 63.5 MB, still half of
  Tauri's text-only peak and under half of Raycast's.
- **Idle CPU stays at zero**; the image does not change the event-driven
  hotkey or the 250 ms pasteboard poll.

## Next

- Store a preview-sized PNG (say 800 px on the long edge) next to the
  original at capture time and show that in the panel; the full image is
  only needed when pasting. That should bring the image case back near the
  text-only numbers.
- Evict GPUI's image cache (or skip the first paint) while hidden, so an
  image only costs memory while the panel is up.
