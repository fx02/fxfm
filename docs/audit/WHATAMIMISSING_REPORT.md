# What am I missing — report (2026-11-26)

Target: fxfm local `main` (0.1.18; 0.1.19 being built). Assumed use: one user on one Mac,
large folders, servers and shares. Evidence routes are in WHATAMIMISSING_NOTES.md.

## Gaps

| # | Gap | Why it matters | Impact | Effort | Confidence | Fix via |
|---|---|---|---|---|---|---|
| 1 | **Core warnings go nowhere** (new). `crates/core` calls `log::warn!`/`error!` 28 times (e.g. "kept: it changed after it was copied" in a cross-device move, Trash and attribute failures), but no logger is installed (absent: `src-tauri/src`, both Cargo manifests — no env_logger/oslog/plugin, no `set_logger`). Source: the code itself reports these conditions for someone to see. | When a move keeps a source item or a Trash/attribute step fails, nothing records why; support and debugging have no trail. | medium | small | high | polish (a tiny stderr/file logger, no new dependency) |
| 2 | **Published copy and release lag behind** (new). Local 0.1.18/0.1.19 vs. public repo 0.1.12 (`fxfm-github` HEAD `271655e`) and latest GitHub release v0.1.6 (`gh release list`). Source: the project publishes through fxfm-github and GitHub releases (README release notes). | Anyone using the public download misses the security and data-loss fixes of 0.1.14–0.1.18 (F3 PDF, server names, staging, SFTP replace). | medium | trivial | high | release (on request) |
| 3 | **No automated tests for the UI flows** (new). Only `ui/lib.test.mjs` (pure helpers, 239 tests); no runner for `panel.js`, `dialogs.js`, `app.js`, `syncwin.js` (absent: `ui/*.test.*` beyond lib, no webdriver/playwright/tauri-driver config). Source: AUDIT_REPORT.md findings 6–8 and 12 were in exactly that untested code. | Regressions in paste, history, refresh races or the sync window are found by hand or not at all. | medium | large | high | prove |
| 4 | **In-app help covers keys only** — known (critique N3). | Rules of sync modes, quick filter, branch view, Undo live in the README. | nice | small | high | evolve |
| 5 | **ROADMAP Next not built**: folder tabs, content search, operation queue, thumbnails — known (ROADMAP, critique H4–H7). | Gaps against comparable file managers. | medium | medium–large | high | evolve |
| 6 | **Screen-reader labels** — known (ROADMAP, critique N2). | VoiceOver users cannot use the panels. | medium | medium | moderate | polish |
| 7 | **Checks only possible on real servers**: SMB domain/Kerberos/Shares, Keychain — known (ROADMAP Servers/SMB). | Unverified paths on real networks. | medium | small | moderate | prove (manual) |

## Dimension results

| Dimension | Result |
|---|---|
| Product / feature | gap 5 (known) |
| User journey | gap 4 (known); first run (index prompt, translocation) and empty states checked — none found |
| Robustness | gap 3 (new) |
| Security / privacy | none found — asset scope, viewer type check, server names, vault (AUDIT_REPORT 1–5 fixed); Keychain unverified (gap 7) |
| Ops / deploy | gaps 1 and 2 (new) |
| Docs / content | none found beyond gap 4 — README sections cover every command in the key table |
| Accessibility / performance | gap 6 (known); performance: the index's 2x RAM during a rebuild is by-design (ROADMAP) |
| Promised-but-undone | gap 5, gap 7 (known) |
| Lifecycle drift | gap 2; 0.1.19 (font, icon styles, status columns) not yet verified live at the time of writing |

## By design (not gaps)

- Undo keeps only the last local operation for the session; copies, servers, archives and
  permanent deletes are not undoable (README Undo).
- No saved sync comparisons; content compare only for local files (ROADMAP).
- English-only UI (project rule).
- The app is not notarized (README release notes); signing with an Apple developer account is
  an owner decision, listed here as an open question, not a gap.

## Totals

Unique gaps 7: medium 6, nice 1. New 3 (1, 2, 3), known 4. Primary dimensions: ops 2,
robustness 1, product 1, journey 1, accessibility 1, promised 1.
