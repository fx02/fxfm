# What am I missing — report (2026-11-27)

Target: fxfm local `main` at `34daf38` (0.1.22 + Find files sorting), with uncommitted content-search
backend work in the tree; public copy fx02/fxfm at `85630b0` (0.1.22). Assumed use: one user
on one Mac, large folders, servers and shares. Evidence routes are in WHATAMIMISSING_NOTES.md.
Previous report: 2026-11-26 (7 gaps), reconciled below.

## Gaps

| # | Gap | Why it matters | Impact | Effort | Confidence | Fix via |
|---|---|---|---|---|---|---|
| 1 | **The download is three versions of features behind the front page** (known, narrowed — was gap 2). The public repo is now 0.1.22, but the latest GitHub release is still v0.1.6 (`gh release list`). Source: README "Download: Builds are on the Releases page" next to features that came after 0.1.6 (server groups, Copy network path, Undo, Synchronize filters, permanent delete on shares). | Someone who reads the README and downloads gets 0.1.6: none of those features, and none of the 0.1.14–0.1.18 security and data-loss fixes. | high | trivial | high | release (owner says "napravi release") |
| 2 | **Reordering by keyboard uses keys macOS takes** (new). Connect to server, Directory hotlist and Define colors by file type move a row with Ctrl+↑/↓ only (`ui/servers.js:1681`, `ui/hotlist.js:493`, `ui/colors.js:353`). fxfm's own conflict table says macOS uses ⌃↑ for Mission Control (`ui/lib.mjs:1386`), and MANUAL Notes says Control+arrows are intercepted. Absent: any other reorder key in those three dialogs. Source: MANUAL tells the user to use Ctrl+↑/↓ there. | With the default keys (Ctrl = Control) and macOS defaults, the keyboard reorder does nothing or opens Mission Control. Only dragging works. | medium | small | moderate (depends on the Mac's shortcut settings; the owner's Mac has no override for them) | polish (also accept Alt+↑/↓, as Connect already uses Alt+←/→) |
| 3 | **Core warnings go nowhere** (known — 2026-11-26 gap 1, still open). There are 28 `log::warn!`/`error!` calls in `crates/core/src`. No logger is installed (absent: `set_logger`/logger crates in `src-tauri/src` and both manifests). | When a move keeps a source or a Trash or attribute step fails, nothing records why. | medium | small | high | polish |
| 4 | **Content search is half done** (new, lifecycle drift; ROADMAP Next). `search/content.rs` with tests and the backend's `walk_content` are uncommitted. Find files has no "Containing text" field yet. Source: ROADMAP Next "Content search in Find files", critique H5. | The work exists but cannot be used. Uncommitted changes are easy to lose or mix into an unrelated build. | medium | small | high | evolve (in progress) |
| 5 | **No automated tests for the UI flows** (known — 2026-11-26 gap 3). There is still only `ui/lib.test.mjs` and no runner for `panel.js`, `dialogs.js`, `servers.js`, `search.js` or `syncwin.js`. | Today's two UI bugs were found by eye, not by a test: the server tree took the sync grid, and the Connect dialog hid its buttons. | medium | large | high | prove |
| 6 | **Screen-reader labels** (known — ROADMAP, critique N2). There are 5 aria/role attributes in all of `ui/`. | VoiceOver users cannot use the panels. | medium | medium | moderate | polish |
| 7 | **Checks only possible on real servers**: SMB domain, Kerberos and Shares, and the Keychain (known — ROADMAP). | These paths are unverified on real networks. | medium | small | moderate | prove (manual) |
| 8 | **Test runs share the installed app's WebKit storage** (new). A temp HOME isolates fxfm's settings but not `~/Library/WebKit/hr.pvt.fxfm`, which holds the Find files, Synchronize and Connect layouts (absent: a `data_directory` for the webview in `src-tauri/src` and `tauri.conf.json`). Observed: a smoke test opened the Connect dialog at the owner's saved 602×424. Source: the project's test rule (read-only towards the real system) and its temp-HOME smoke tests. | Smoke tests can change the owner's saved window layouts and inherit them, so a test does not show the default state. | nice | small | high | ops (or polish: a data-dir webview store) |
| 9 | **In-app help covers keys only** (known — critique N3). | Sync modes, quick filter, branch view, Undo and groups are explained only in docs/MANUAL.md. | nice | small | high | evolve |

## Dimension results

| Dimension | Result |
|---|---|
| Product / feature | gap 4 (and ROADMAP Next: tabs, operation queue, thumbnails, known, by owner priority) |
| User journey | gap 2 (new), gap 9 (known); first run, network delete question and the Connect window were checked, nothing more found |
| Robustness | gap 5 (known); content search skips FIFOs and devices, cloud-only placeholders and symlinks (checked in `find.rs`) |
| Security / privacy | none found. Publish scans of today's pushes were clean, and an independent critic found no leak. Keychain is still unverified (gap 7). |
| Ops / deploy | gap 1 (known, narrowed), gap 3 (known), gap 8 (new) |
| Docs / content | none found. The README and MANUAL split is done, every README claim was checked against MANUAL by the critic, and the key-table test reads MANUAL. |
| Accessibility / performance | gap 6 (known). Performance: content search reads files one after another on one thread, which is acceptable for a folder search and not measured on big trees. |
| Promised-but-undone | gap 4, gap 7 |
| Lifecycle drift | gap 4. The 0.1.22 rebuild with Find files sorting is in the build and not yet smoke-tested. The README feature list has no line for content search or sorting yet (add them when they ship). |

## Closed since 2026-11-26

- Gap 2 (reorder keys): Alt+↑/↓ moves rows in Connect to server, Directory hotlist and colors by file type (0.1.23).
- Gap 3 (core warnings): `fxfm_core::logging` writes `~/Library/Logs/fxfm/fxfm.log` (0.1.23).
- Gap 4 (content search): shipped in 0.1.22.

- The public repo lagging the local one (0.1.12 against 0.1.18): fx02/fxfm is at 0.1.22 since today. Only the release part remains (gap 1).

## By design (not gaps)

- Content search only in *Current folder* (reading every indexed file would take hours), and UTF-8 text only (no UTF-16).
- Undo keeps only the last local operation; network-share deletes are permanent and say so.
- English-only UI; not notarized (owner decision).

## Totals

Unique gaps: 9.
- By impact: high 1, medium 6, nice 2.
- New 3 (2, 4, 8); known 6 (1 narrowed).
- Primary dimension: ops 3, journey 2, product 1, robustness 1, accessibility 1, promised 1.
