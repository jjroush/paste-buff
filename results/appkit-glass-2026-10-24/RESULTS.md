# Round three — 2026-10-24, Liquid Glass (0.4.0) and Maccy

**Machine:** Apple M2 Max, 32 GB, macOS 27.0.1, rustc 1.98.0. Release builds
(thin LTO, `codegen-units = 1`, stripped).

**Protocol:** same as round two. 20 s idle → 20 × (open, hold 1 s, close,
wait 1 s) → 20 s cooldown, sampled every 0.5 s. paste-buff was launched by
the harness and toggled with SIGUSR1, each run from a fresh copy of the seed
(202 entries with a 1600×1000 PNG newest, or 200 text entries with
`--no-media`). 0.3.0 was rebuilt from its tag and re-run straight after 0.4.0
as a control, so both share the OS build and background load.

Maccy 2.7.1 (Homebrew cask) was given the same 200 text entries by copying
each one through the pasteboard, then relaunched by the harness and toggled
with `--toggle keys` after its popup shortcut was set to ⌘⇧V.

| target | procs | idle footprint | peak (cycling) | after cycles | idle RSS | idle CPU | cycling CPU | binary |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| paste-buff-appkit 0.4.0 (glass), text-only seed | 1 | 16.4 MB | 20.3 MB | 20.1 MB | 51.4 MB | 0.0% | 2.8% | 0.7 MB |
| paste-buff-appkit 0.4.0 (glass), image newest | 1 | 17.4 MB | 31.6 MB | 27.4 MB | 54.4 MB | 0.0% | 3.4% | 0.7 MB |
| paste-buff-appkit 0.3.0, text-only seed | 1 | 14.2 MB | 17.0 MB | 16.9 MB | 48.1 MB | 0.0% | 1.6% | 0.7 MB |
| paste-buff-appkit 0.3.0, image newest | 1 | 14.1 MB | 18.1 MB | 18.0 MB | 48.1 MB | 0.0% | 1.7% | 0.7 MB |
| Maccy 2.7.1, 200 text clips | 3 | 53.6 MB | 90.8 MB | 63.9 MB | 124.2 MB | 0.8% | 3.4% | 3.6 MB |

Raw samples: `appkit-glass*.csv`, `appkit-030*.csv`, `maccy.csv`; summaries
in the matching `.json`, stderr in `.log`. Regenerate the table with
`paste-buff-bench report results/appkit-glass-2026-10-24/*.json`.

## Reading it

- **Glass costs about 2 MB idle and 3 MB after use** with text clips.
  NSGlassEffectView makes the content view hierarchy layer-backed, and those
  layers outlive the ordered-out window.
- **With an image previewed, 0.4.0 settles 9 MB higher.** Footprint jumps by
  about 12 MB on the first show, then plateaus near 28 MB and holds through
  the cooldown, so it is a retained cache rather than a leak. 0.3.0 gave the
  decoded image back on hide.
- **Maccy idles at about three times paste-buff 0.4.0** and peaks at 91 MB.
  Two of its three processes are system helpers macOS attributes to it
  (`SetStoreUpdateService`, `SafariPlatformSupport.Helper`), counted the same
  way as Raycast's WebKit helpers in round one. A probe run before the
  measured one also had `MTLCompilerService` (36 MB) charged to Maccy; it had
  exited by the measured run.
- Idle CPU is at the noise floor for paste-buff; Maccy averages 0.8% idle.

## Caveats

- Every paste-buff run logged exactly 20 shows and 20 hides.
- During the Maccy run an older paste-buff (0.2.0, from the retired
  `install.sh` launchd agent) was running and also holds ⌘⇧V. Maccy's
  footprint rose on every cycle, so its popup did open, but some cycles may
  have been cut short by the other panel taking key status.
- Maccy keeps its history in SwiftData inside its sandbox container; the
  seeded entries could not be inspected from outside the container, only
  inferred from the copies made while it was running.
