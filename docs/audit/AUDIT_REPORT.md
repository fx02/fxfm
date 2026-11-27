# fxfm audit report (2026-11-26, updated by the polish pass)

Target: `main` after 0.1.13 (`8d569b7`). Source: the whole-repo audit of 2026-11-25 (three
independent reviews: core crate, Tauri layer, UI) plus a lead recheck of findings 1–5 in the code.
Assumed load: one user on one Mac, folders with 50k+ items, multi-GB files, flaky servers/shares.
Scales: shared lifecycle scale (severity, effort, risk).

## Status

| # | Finding | Severity | Effort | Status | Evidence |
|---|---|---|---|---|---|
| 1 | F3 rendered any `*.pdf` (HTML too) in a page whose asset protocol could read the whole disk | blocker (security) | small | **fixed** | `viewer.rs` checks `%PDF-`; asset scope empty, `open_viewer` allows only the viewed file; test `pdf_needs_the_pdf_header` |
| 2 | A server-listed name with `/` (`../x`) wrote outside the copy target / deleted outside the folder | blocker (security) | small | **fixed** | `remote/transfer.rs` `unsafe_name` in Downloader and Remover; test `unsafe_listed_names_never_reach_outside` (fails without the fix) |
| 3 | Names differing only in case overwrote each other in staging; a move then deleted both originals; a 7z rewrite swapped content | critical (data loss) | small | **fixed** | `archive/read.rs` Sink refuses an existing file not written under the same name; Downloader refuses an existing target; tests `extract_refuses_names_that_differ_only_in_case`, `names_differing_only_in_case_are_not_written_over_each_other` |
| 4 | SFTP replace without posix-rename removed the target first; a failing rename lost it and the `.part` | critical (data loss) | small | **fixed** | SFTP moves the target to a backup first and restores it; `put_part` keeps the `.part` when the target is gone; `sftp_file_ops_roundtrip` (real local sftp-server) extended |
| 5 | F4 stamp read after the download: a file changing during the copy uploaded over newer data without a conflict | critical (data loss) | trivial | **fixed** | `editwatch.rs` stamps before and after; test `a_file_changed_during_the_copy_is_a_conflict_on_upload` (fails without the fix) |
| 6 | Paste into the own folder ignored hidden/filtered names for "x copy" | high | trivial | **fixed** | `dialogs.js` uses `dst.raw` |
| 7 | Alt+Left twice quickly dropped a history entry (superseded `go` reported ok) | high | trivial | **fixed** | `panel.go` returns `{ok:false, superseded:true}` when overtaken; startup fallback ignores superseded |
| 8 | A refresh-style `go` (after F2/F7/pack/multi-rename) cancelled a navigation the user had started | medium | trivial | **fixed** | `panel.go` with `refresh` does nothing while a user navigation is pending |
| 9 | Disconnect cleanup could delete an edit saved during the hash | medium | small | **fixed** (0.1.17) | `editwatch.rs` rechecks size/mtime under the lock and again right before removing the folder |
| 10 | Cancel while connecting ignored before the attempt is registered | medium | small | **fixed** (0.1.17) | `remote.rs` `connect` registers the attempt for the whole flow and clears it at the end |
| 11 | Moved data not fsynced before the source is deleted | medium | small | **fixed** (0.1.17) | `ops::sync_tree` on staged items before the move out of an archive/server stage |
| 12 | Paste into the own folder ran one job per item and Cancel did not stop it | medium | trivial | **fixed (partly)** | the loop stops on cancel/failure; each job still refreshes both panels |
| 13 | `clipboard_write` / `start_drag` stat files on the main thread (dead share freezes the UI) | medium | small | **fixed** (0.1.17) | both commands async; the checks run on a blocking thread |
| 14 | One watcher thread per F4 copy from an archive, never released | medium | medium | **fixed** (0.1.18) | one shared `notify` watcher for all copies (each copy's folder added and removed), events routed by path |
| 15 | Cross-device move deletes files that appeared in the source during the copy | medium | small | **fixed** (0.1.17) | `ops::remove_moved` deletes only copied names, keeps files changed after the copy and non-empty folders; test `cross_device_move_keeps_what_changed_or_appeared_meanwhile` |
| 16 | Drive menu / Open with menu could open on top of a dialog | medium | trivial | **fixed** | `topModal()` check after the await |
| 17 | Copy name on a results list copied the shown path | nice | trivial | **fixed** (0.1.13) | `dialogs.js` copyNames |
| 18 | Multi-rename preview and drag start are O(n²) with many marked items | medium | small | **fixed** (0.1.17) | per-folder Set of name keys (50k names: ~0.1 s), Map by name in drag start |
| 19 | Archive edits are quadratic on big trees | medium | medium | **fixed** (0.1.18) | `Plan` path index (BTreeSet + file set), runs of deletes in one pass, hard-link check skipped without links; `Planned` sorted set and ranged `merge` in vfs/transfer; tests `batched_deletes_match_one_by_one`, `many_edits_on_a_big_plan_are_fast` (40k edits: 0.4 s debug) |
| 20 | Duplicate UI helpers (`el` ×9, `cycleFocus` ×6, `frame`/`message` ×3, three popup menus) | nice | small | **fixed** (0.1.18) | `ui/dom.js` (`el`, `cycleFocus`, `cycleIn`, `frame`); `message` exported from dialogs.js; the popup menus stay separate (the drive menu has its own keys and start row) |

Also fixed with the Synchronize dirs work (0.1.10): case-insensitive name matching in the
comparison (a server's `README` and a local `readme` copying over each other) and the shared
comparison stop flag.

## Not read in the audit

Search/index (`search/`), audio, NetBIOS/dns-sd in `smb.rs`, the drag image drawing, SFTP/FTP
connect, auth and certificate handling, `config.js`, `colors.js`, the server editor in
`servers.js`, `viewer.js`.

## Polish pass (2026-11-26, after 0.1.16)

Baseline: `fb54db4` (0.1.16), all checks passing (clippy, cargo test 207 + 455, node 237).
Self-review of the newest work found one test-policy defect: the Undo Trash round-trip test
ran by default and touched the user's Trash; it is now `#[ignore]` like the existing Trash test.
Dimensions: correctness, security, tests and performance above; dependencies — `objc2-foundation`
(already in the lock file) added to the core crate for Undo, approved; documentation — README
updated for Undo, clipboard keys, sync Show/picking; dead code — none found beyond item 20;
build/DX — unchanged; UX — see docs/critique.
