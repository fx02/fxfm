# What am I missing — notes

## 2026-11-27

- **Target:** fxfm local `main` at `34daf38`. The tree also has uncommitted content-search work: `crates/core/src/search/content.rs` and `src-tauri/src/search{.rs,/find.rs}`. Public fx02/fxfm is at `85630b0`.
- **Read:** docs/MANUAL.md, README.md, docs/ROADMAP.md, docs/audit/AUDIT_REPORT.md, docs/critique/UI_CRITIQUE_REPORT.md, and the previous report.
- **Purpose, audience, stage:** unchanged. A personal Total Commander-style manager for one power user, published as source plus dmg releases.
- **Routes for each claim:**
  - **Release lag:** `gh release list -R fx02/fxfm` (latest v0.1.6), and `git log` of fxfm-github (0.1.22) compared with README's Download line.
  - **Reorder keys:**
    - Grep for `ctrl && (e.key === "ArrowUp"` in `ui/`, which finds three dialogs and no other reorder key.
    - fxfm's own macOS conflict table (`ui/lib.mjs:1386`) and MANUAL Notes.
    - The Mac's `com.apple.symbolichotkeys` has no entries 32 and 33, so the defaults apply. That is an inference, hence moderate confidence.
  - **Logger:** grep for `set_logger`, env_logger, oslog and tauri_plugin_log in `src-tauri/src`, `crates/core/src` and both manifests (none). There are 28 `log::` call sites.
  - **UI tests:** `ui/*.test.*` holds only lib.test.mjs. There is no playwright, webdriver or tauri-driver config in the tree.
  - **WebKit isolation:** no `data_directory` or `dataDirectory` in `src-tauri` or tauri.conf.json. Observed: the real `~/Library/WebKit/hr.pvt.fxfm/.../localstorage.sqlite3` held `fxfm.connectDialog {"w":602,"h":424}`, and a temp-HOME test run used it.
  - **Aria:** count of `aria-` and `role=` in `ui/` (5).
- **Cross-links:**
  - Gap 2 sits between journey and accessibility (keyboard path).
  - Gap 4 is ROADMAP Next and critique H5.
  - Gap 8 touches the test policy in the owner's rules.

## 2026-11-26

- **Target:** fxfm local `main` (0.1.18 committed, 0.1.19 in the build), README.md, docs/ROADMAP.md, docs/audit/AUDIT_REPORT.md, docs/critique/UI_CRITIQUE_REPORT.md, the public copy (fxfm-github) and its GitHub releases.
- **Purpose, audience, stage:** a personal Total Commander-style file manager for one power user on macOS. It is feature-complete for daily use and published as source plus dmg releases on GitHub.
- **Routes used for absence claims:**
  - **Publish drift:** `git log` and `tauri.conf.json` in both repos, plus `gh release list`.
  - **UI tests:** `ui/*.test.*`, test runners and configs in the tree (webdriver, playwright, tauri-driver), and what `ui/lib.test.mjs` imports (only `lib.mjs`).
  - **Logging:** logger crates and initialisation in `src-tauri/src`, both Cargo manifests, and the `log::` call sites in `crates/core/src` (28).
- **Cross-links:** the in-app help gap is the critique's N3. Tabs, content search, queue, thumbnails and VoiceOver are ROADMAP "Next" and critique H4–H7, N2.
