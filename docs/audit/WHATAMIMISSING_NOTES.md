# What am I missing — notes (2026-11-26)

- Target: fxfm local `main` (0.1.18 committed, 0.1.19 in the build), README.md, docs/ROADMAP.md,
  docs/audit/AUDIT_REPORT.md, docs/critique/UI_CRITIQUE_REPORT.md, the public copy
  (fxfm-github) and its GitHub releases.
- Purpose/audience/stage: a personal Total Commander-style file manager for one power user on
  macOS; feature-complete for daily use, published as source + dmg releases on GitHub.
- Routes used for absence claims:
  - publish drift: `git log`/`tauri.conf.json` in both repos and `gh release list`;
  - UI tests: `ui/*.test.*`, test runners/configs in the tree (webdriver, playwright,
    tauri-driver), and what `ui/lib.test.mjs` imports (only `lib.mjs`);
  - logging: logger crates/initialisation in `src-tauri/src`, both Cargo manifests, and the
    `log::` call sites in `crates/core/src` (28).
- Cross-links: the in-app help gap is the critique's N3; tabs, content search, queue,
  thumbnails and VoiceOver are ROADMAP "Next" and critique H4–H7, N2.
