# fxfm UI critique report (2026-11-21)

Target: fxfm 0.1.2, whole UI. Audience: one keyboard-driven Total Commander user on macOS.
Evidence: live walk of an isolated instance (menus, Keyboard, Options, Ctrl+F, Ctrl+D, Ctrl+M,
F5, Alt+F5, Change attributes, Find files window) and source reading — see
[CRITIQUE_INTERNAL.md](CRITIQUE_INTERNAL.md); benchmark of Total Commander, ForkLift 4, Marta,
Commander One and Nimble Commander from their current docs — see
[CRITIQUE_BENCHMARK.md](CRITIQUE_BENCHMARK.md). Not exercised live: F3 viewer, F8 confirmation,
job progress/error dialogs, server sessions. Benchmark limits: fetched pages were summarised by the
fetch tool; several TC wiki pages returned 404; commander.com was unreachable; nothing was
installed. Scales: shared lifecycle scale (severity, effort, confidence).

## Findings

Sorted by severity, then effort, then confidence.

### Contradictions

| ID | Finding | Evidence | User impact | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|---|
| C1 | Files → *Change attributes…* is offered everywhere but refuses local files ("works on SFTP and SCP servers") | I1, live | The most common case (chmod / dates on local files, TC Alt+A) does nothing useful | high | medium | high | evolve (local attributes) |
| C2 | Menu bar *Start* and *Help* are empty; help exists only as small per-dialog texts | I2, I3, live menu dump | Empty menus look broken; no place for the key reference | medium | small | high | polish (fill Help with the key table / README link, drop or fill Start) |
| C3 | Native selects render smaller than the rest of each dialog | I5, screenshots | Dialogs look unfinished; small text is harder to read | nice | trivial | high | unslop |
| C4 | F8 confirmation never names the item ("1 file(s)/dir(s)") | I6, `deleteConfirmText` | Less certainty about what is deleted | nice | trivial | high | polish |

### Unjustified overlaps

None found. Undo appears in two places (Edit → Undo for text fields, *Undo last rename* in
Multi-rename) but they do different things; the gap is covered under safety nets.

### Industry gaps (relevant to a TC user)

| ID | Finding | Benchmark | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| G1 | No folder tabs per panel | All five targets have tabs (TC, ForkLift, Marta, Commander One, Nimble) | high | large | high | evolve |
| G2 | Find files searches names only, no content search | TC, ForkLift, Commander One, Nimble | medium | medium | high | evolve |
| G3 | Quick search only jumps; no quick filter that narrows the list (TC Ctrl+S toggles a live filter) | TC (wiki Quick_search) | medium | small | moderate | evolve |
| G4 | No directory compare / synchronize | TC, ForkLift | medium | large | high | evolve |
| G5 | No folder history back/forward | TC (Alt+Left/Right, forum source only) | medium | small | moderate | evolve |
| G6 | No operation queue (run copies in the background, one after another, pause) | Marta (queue with pause/abort), TC partial | medium | medium | moderate | evolve |
| G7 | No branch view / flatten | TC Ctrl+Shift+B, Marta Flatten | nice | medium | moderate | evolve |
| G8 | No thumbnails / brief view | TC, Commander One, Nimble | nice | medium | moderate | evolve |

Command palette: only Marta has one (ForkLift partial), so it is not a baseline for this audience.

### Obsolete patterns

None supported: no pattern was found replaced in two comparable targets. "Feed to listbox" and the
F-key bar match TC and Nimble Commander.

### Missing safety nets

| ID | Finding | Evidence | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| S1 | No undo for move/rename/delete; Trash via NSFileManager, so Finder's *Put Back* does not work for fxfm deletions | README Notes, I4; ForkLift has undo entries | medium | medium | high | evolve |
| S2 | Accessibility: panel rows and most controls have no roles or labels (4 aria/role uses in `ui/`); VoiceOver use is unlikely to work | source grep | medium | medium | moderate | polish / evolve |
| S3 | System dark mode is not followed: dialogs stay light while the title bar is dark | screenshots, no `prefers-color-scheme` in `style.css`; Nimble and ForkLift follow appearance | nice | medium | moderate | unslop |

## UX baseline (local copy)

| Pattern | Status |
|---|---|
| Command palette (Cmd/Ctrl-K) | ABSENT (not a TC-user baseline; Marta only) |
| Dark mode | PARTIAL (panel colors configurable; dialogs ignore system appearance) |
| Keyboard navigability for every primary action | PRESENT (every command has keys or a menu item; dialogs keyboard driven) |
| Empty states with a CTA, not just "Nothing here" | PARTIAL (Find files first run offers Build index; empty panels show `..` only) |
| Inline edit instead of modal-then-save for single fields | PARTIAL (F2 rename is a dialog, TC renames in place) |
| Bulk select + bulk action on list views | PRESENT (marks, select group, Feed to listbox) |
| Optimistic updates with rollback on error | N/A (file operations report per-file errors with Retry/Skip) |
| Undo for destructive actions, rather than a confirm modal | ABSENT for file ops (Trash + confirm only); PRESENT for multi-rename |
| Global search across the app, not per-view | PRESENT (Find files over the index) |
| Saved views / filters as first-class objects | ABSENT (no saved searches; rename presets exist) |
| API access / webhook surface for automatable data | N/A (single-user desktop tool) |
| Audit log for multi-user state changes | N/A (single user) |
| OS-level integrations (tray, notifications, file assoc.) | PARTIAL (Quick Look button, Trash, drag to Finder; no Services/notifications) |
| Accessibility: keyboard nav, focus rings, ARIA labels, contrast, screen-reader semantics | PARTIAL (keyboard and focus rings yes; ARIA/screen reader largely missing) |

## Counts

Contradictions 4, overlaps 0, industry gaps 8, obsolete 0, safety nets 3.
Routes: evolve 10, polish 3 (C2, C4, S2 shared), unslop 2 (C3, S3).

## Next work (evidence-based)

1. C1 local *Change attributes* (high, the menu already promises it).
2. G1 folder tabs (high, every comparable product has them).
3. C2 fill the Help menu with the key table (cheap, removes a broken-looking surface).
4. G3 quick filter and G5 history (small, TC muscle memory).
5. S1 undo of the last file operation.
