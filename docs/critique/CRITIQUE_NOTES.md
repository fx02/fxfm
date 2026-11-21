# UI critique — orientation notes (2026-11-21)

- Target: fxfm 0.1.2 (main 240ac90 + b0613e8), the whole UI: main window (two panels, drive bar,
  path line, status line, command line, F-key bar), the macOS menu bar, dialogs (F5/F6, Alt+F5 pack,
  F8, Ctrl+M multi-rename, Ctrl+D hotlist, Ctrl+F servers, Configuration → Keyboard / Options,
  Change attributes), Find files window (Alt+F7), F3 viewer and audio strip.
- Audience: one power user (the owner), long-time Total Commander user on Windows who moved to
  macOS and wants TC's keyboard workflow and keys; English UI, Croatian locale/keyboard.
- Category: keyboard-driven dual-panel file managers (orthodox file managers) on macOS.
- Benchmark candidates: Total Commander (the model), ForkLift 4, Marta, Commander One,
  Nimble Commander; Finder only for macOS conventions. See CRITIQUE_BENCHMARK.md.
- Live walk: isolated instance (temp HOME with a copy of the data dir), dialogs opened through the
  menu bar (System Events), screenshots kept outside the repo (they show real folder and server
  names). Not exercised live: F3 viewer, F8 confirmation (avoided on real folders), error states of
  jobs, server connections.
- Excluded: functional correctness (that is prove), visual polish details beyond what affects use
  (unslop).
