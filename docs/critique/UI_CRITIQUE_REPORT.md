# fxfm UI critique report (2026-11-28)

**Target:** fxfm 0.1.25 (local `main`). The surfaces added since the 2026-11-26 report are covered, and its findings are rechecked:
- Copy network path;
- F8 on a share;
- the Connect window and groups;
- Find files text search and sorting;
- Background / Queue / operations strip / quit question;
- the panel splitter;
- the Linux build.

**Audience:** one keyboard-driven Total Commander user, on macOS and now on Linux.

**Evidence:**
- an isolated live instance of 2026-11-27 (see CRITIQUE_NOTES.md for what was and was not exercised);
- a source walk (CRITIQUE_INTERNAL.md J10–J18);
- a benchmark of Total Commander, Double Commander and ForkLift 4 from their docs (CRITIQUE_BENCHMARK.md, 2026-11-28).

**Scales:** the shared lifecycle scale.

## Findings

### Contradictions

| ID | Finding | Evidence | User impact | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|---|
| K5 | **Find files: *In: Everywhere* stays checked while a *Text* search runs in the current folder** | J10; `ui/lib.mjs` searchRequest, `ui/search.js:153` (placeholder only) | The checked choice says one thing and the search does another; a user may think the whole disk was searched | medium | trivial | high | polish (disable Everywhere and show why while Text has a value) |
| K6 | **Linux shows macOS words and keys**: "Show in Finder", "Finder (macOS icons)", "Folder icon (Finder icons)", ⌥⌃⌘ glyphs, the Fn option | J15; `ui/lib.mjs:2263,2743`, `config.js:505`, `comboLabel`; the Rust menu uses the same glyphs (`keymap.rs` label) | A Linux user reads keys that do not exist on their keyboard | medium | small | high | polish (Linux port UI pass, already planned) |

### Unjustified overlaps

None found.

### Industry gaps

| ID | Finding | Benchmark | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| H8 | **No list of the operations**: the strip shows the first background job and a queued count. There is no window that lists each running and queued operation, removes one, reorders or pauses (J11). | DC Operations Viewer (order, pause, stop); TC transfer manager (+/−) | medium | medium | high (DC) | evolve |
| H10 | **Content search options**: case only. No regex, whole words, hex, UTF-16 or other encodings, and nothing inside archives or on servers. | DC: case, hex, regex, encodings, archives; TC: UTF-16 and several encodings at once, FTP search | medium | medium | high | evolve |
| H11 | **Splitter presets**: drag and double-click only. No right-click presets (20/80 … 80/20) and no command or key for 50 %. | DC right-click presets; TC presets, cm_50Percent, Ctrl+drag snap | nice | small | high | evolve |
| H9 | **No key for *Queue* in the F5/F6 dialog or for *Background* in the progress window** (J12, J13) | TC: F2 in the copy dialog (forum-sourced) | nice | trivial | moderate | polish |
| H4 | Folder tabs (from 2026-11-21) | all earlier targets | medium (owner: not a priority) | large | high | evolve (deferred) |
| H7 | Thumbnails (from 2026-11-21) | TC, Commander One, Nimble | nice | medium | moderate | evolve |

### Obsolete patterns

None supported.

### Missing safety nets

| ID | Finding | Evidence | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| N2 | Screen reader labels (still 5 aria/role attributes) | source | medium | medium | moderate | polish |
| N3 | In-app help stops at the key table. Queue, groups, text search, splitter and network delete live only in docs/MANUAL.md. | source | nice | small | high | evolve |

**By design, not findings:**
- A second Quit within 5 s quits without asking (J14). It is the escape hatch for a hung window, and MANUAL documents it.
- Results sorting has no key (J18). No benchmark tool documents one.

## Recheck of the 2026-11-26 findings

| ID | Finding | Now |
|---|---|---|
| K1 | Delete = skip in Synchronize | fixed |
| K2 | Alt+Left/Right in the command line | fixed |
| K3 | "Properties…" opened Change attributes | fixed (renamed) |
| K4 | Esc did not stop a branch view | fixed |
| H1 | Clipboard keys | fixed (Ctrl+C/X/V) |
| H2 | Sync row filters | fixed |
| H3 | Sync several rows at once | fixed |
| H4 | Tabs | open |
| H5 | Content search | fixed (0.1.22); options gap → H10 |
| H6 | Operation queue | fixed partly (0.1.24): Background, Queue, strip; list and control → H8 |
| H7 | Thumbnails | open |
| N1 | Undo | fixed (0.1.16) |
| N2 | Accessibility | open |
| N3 | In-app help | open |

## UX baseline (local copy)

| Pattern | Status |
|---|---|
| Command palette (Cmd/Ctrl-K) | ABSENT (not a TC-user baseline) |
| Dark mode | PRESENT (macOS appearance; Linux follows GTK: UNVERIFIED on a real desktop) |
| Keyboard navigability for every primary action | PARTIAL (Queue, Background, splitter, result sorting are mouse only: H9, J17, J18) |
| Empty states with a CTA | PARTIAL (unchanged) |
| Inline edit instead of modal-then-save for single fields | PRESENT |
| Bulk select + bulk action on list views | PRESENT (panels, sync list, Find results) |
| Optimistic updates with rollback on error | N/A |
| Undo for destructive actions, rather than a confirm modal | PARTIAL (Undo for local moves, renames, Trash; network-share and server deletes are permanent and say so) |
| Global search across the app | PRESENT (Find files by name and by text) |
| Saved views / filters as first-class objects | ABSENT |
| API access / webhook surface | N/A |
| Audit log for multi-user state changes | N/A |
| OS-level integrations | PRESENT on macOS (Finder clipboard, Quick Look, Open with, Trash, drag). Linux equivalents are built but UNVERIFIED on a real desktop. |
| Accessibility: keyboard nav, focus rings, ARIA, contrast, screen reader | PARTIAL (ARIA largely missing) |

## Counts

- **Unique findings:** 11.
  - Contradictions 2 (K5, K6). Overlaps 0. Obsolete 0.
  - Industry gaps 6 (H8, H10, H11, H9, plus H4 and H7 still open).
  - Safety nets 2 (N2, N3 open).
- **Routes:** polish 4 (K5, K6, H9, N2), evolve 7 (H8, H10, H11, H4, H7, N3).
- **Recheck:** of the 14 earlier findings, 10 are fixed (2 of them partly, with the rest now H8 and H10) and 4 are open.

## Next work (evidence-based)

1. **Trivial polish:**
   - K5: Everywhere disabled and explained while Text has a value;
   - H9: F2 = Queue in the copy dialog, B or F2 = Background in the progress window.
2. **K6 as part of the Linux UI pass:**
   - key names Ctrl/Alt/Super;
   - "Show in file manager", "System icons";
   - no Fn option and no macOS folder icons.
3. **H8:** an operations list (running and queued, remove one, move up/down), opened from the strip. Double Commander's viewer is the model.
4. **H10:** text search options (whole words, regex, UTF-16), then archives.
