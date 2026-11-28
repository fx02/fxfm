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

# Orientation notes (2026-11-26)

- Target: fxfm 0.1.14 (`2406bb8`). Focus: surfaces added since 2026-11-21 — right-click menu and
  clipboard, quick filter (Ctrl+S), folder history (Alt+Left/Right/Down), branch view (Ctrl+B),
  Synchronize dirs window, Help → Keyboard shortcuts, dark mode — plus a recheck of the open
  findings of the 2026-11-21 report.
- Audience and category unchanged (one TC power user on macOS; orthodox file managers).
- Benchmarks for the new surfaces: Total Commander (the model), Double Commander (TC clone with
  current docs), FreeFileSync (sync specialist), ForkLift 4 (macOS). See CRITIQUE_BENCHMARK.md.
- Live evidence: screenshots of an isolated instance taken during this session's work (context
  menu, filter box, Help table, F8 text, sync window in both layouts, branch view, F3 viewer).
  The owner was working on the machine, so no keystrokes were sent for this pass: keyboard paths
  (filter keys, history keys, sync list keys) are from source reading only.

## 2026-11-28

- **Target:** fxfm local `main`, version 0.1.25 (`232868f`, plus the Linux port work in progress). Also fx02/fxfm 0.1.23.
- **Surfaces new since the 2026-11-26 report:**
  - right-click *Copy network path…*;
  - F8 on a network share;
  - Connect to server window (move/resize/maximize, groups, Alt+arrows);
  - Find files *Text* field and column sorting;
  - Background button, F5/F6 *Queue*, the operations strip and the quit question;
  - the panel splitter;
  - the Linux build (Debian 12, GTK).
- **Audience:** one keyboard-driven Total Commander user. The owner now also targets Linux.
- **Live evidence (2026-11-27, isolated instance, temp HOME, demo data):**
  - Connect dialog with groups (screenshots);
  - Find files text search and the Folder sort (screenshots);
  - a queued F5 copy (file arrived, strip gone afterwards);
  - the 25/75 split restored;
  - the Linux arm64 package started under Xvfb (screenshot).
- **Not exercised live:**
  - a long background job (the strip while running, clicking it back to the window);
  - the quit question with jobs running;
  - Alt+↑/↓ in the dialogs (the guard blocked keys: another app was frontmost);
  - drag in the Linux build;
  - every Linux GTK feature on a real desktop.
- **Benchmark candidates:** Total Commander, Double Commander, ForkLift 4 (same as before, comparable keyboard/two-panel tools).
