# fxfm UI critique report (2026-11-26)

Target: fxfm 0.1.14 (`2406bb8`): the surfaces added since the 2026-11-21 report and a recheck of
its findings. Audience: one keyboard-driven Total Commander user on macOS. Evidence: source walk
of the new surfaces, screenshots of an isolated instance taken during this week's work (see
CRITIQUE_INTERNAL.md, 2026-11-26), benchmark of Total Commander, Double Commander, FreeFileSync
and ForkLift 4 from their docs (CRITIQUE_BENCHMARK.md, 2026-11-26). Keyboard paths were not
exercised live in this pass (the owner was using the machine): **static walk for keys**.
Scales: shared lifecycle scale.

## Findings

### Contradictions

| ID | Finding | Evidence | User impact | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|---|
| K1 | In the Synchronize dirs list **Delete means "skip"**, while Delete deletes in the panels | J1, `ui/syncwin.js:420` | A TC user pressing Delete to drop a row expects the opposite of "delete the file"; today it is harmless (skip) but the meaning flips between windows | medium | trivial | high | polish (Delete/Backspace out; Space only) |
| K2 | **Alt+Left/Right in the command line** go to the folder history instead of moving the caret by word | J4, `cmdlineKeyGoesToPanel` | Editing a typed command with Option+arrows jumps folders | medium | trivial | moderate (static) | polish (keep Alt+Left/Right in the field) |
| K3 | Right-click **Properties…** opens Change attributes only | J2 | The label promises size, dates, kind; the dialog shows only permissions | nice | trivial | high | this-pass candidate (rename to "Change attributes…") |
| K4 | Branch view stops with **Ctrl+B**, not Esc | J5 | A long scan of a big tree cannot be stopped the usual way | nice | trivial | high | polish |

### Unjustified overlaps

None found. Right-click items reuse the existing commands (same handlers as the menu and keys).

### Industry gaps (relevant to a TC user)

| ID | Finding | Benchmark | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| H1 | **Clipboard keys**: Copy/Cut/Paste have no default keys (TC: Ctrl+C/X/V in the panels) | J3; TC model | medium | trivial | high | polish (bind Ctrl+C/X/V; Control is free on macOS) |
| H2 | **Sync row filters**: only "Show equal"; no show →, ←, ≠ (not copied), singles, duplicates | TC, Double Commander | medium | small | high | evolve |
| H3 | **Sync: change several rows at once** (select rows, then →/←/skip/reverse) | Double Commander | medium | small | high | evolve |
| H4 | Folder **tabs** (from 2026-11-21, G1) | all five earlier targets | medium (owner: not a priority) | large | high | evolve (deferred by owner) |
| H5 | **Content search** in Find files (G2) | TC, ForkLift, Commander One, Nimble | medium | medium | high | evolve |
| H6 | **Operation queue** (G6) | Marta, TC partial | medium | medium | moderate | evolve |
| H7 | **Thumbnails** (G8) | TC, Commander One, Nimble | nice | medium | moderate | evolve |

### Obsolete patterns

None supported.

### Missing safety nets

| ID | Finding | Evidence | Sev. | Effort | Conf. | Route |
|---|---|---|---|---|---|---|
| N1 | **Undo** of the last move/rename/delete is still missing (S1); Synchronize can now delete (to the Trash) as well | README, ROADMAP | medium | medium | high | evolve |
| N2 | **Screen reader** labels (S2) unchanged; the new sync window and menus add more unlabeled controls | source | medium | medium | moderate | polish |
| N3 | **In-app help** stops at the key table; feature rules (sync modes, quick filter, branch view) live only in the README | J8 | nice | small | high | evolve |

## Recheck of the 2026-11-21 findings

| ID | Finding | Now |
|---|---|---|
| C1 | Change attributes refused local files | fixed (0.1.4) |
| C2 | Empty Start and Help menus | fixed (0.1.7): Start gone, Help → Keyboard shortcuts |
| C3 | Selects smaller than the dialog font | fixed (0.1.2) |
| C4 | F8 never named the item | fixed (0.1.7) |
| G1 | Folder tabs | open (H4) |
| G2 | Content search | open (H5) |
| G3 | Quick filter | fixed (0.1.8, Ctrl+S) |
| G4 | Directory compare / synchronize | fixed (0.1.9–0.1.12) |
| G5 | Folder history | fixed (0.1.8) |
| G6 | Operation queue | open (H6) |
| G7 | Branch view | fixed (0.1.13, Ctrl+B) |
| G8 | Thumbnails | open (H7) |
| S1 | Undo | open (N1) |
| S2 | Accessibility | open (N2) |
| S3 | Dark mode | fixed (0.1.7) |

## UX baseline (local copy)

| Pattern | Status |
|---|---|
| Command palette (Cmd/Ctrl-K) | ABSENT (not a TC-user baseline) |
| Dark mode | PRESENT (system appearance for chrome; panel colors configurable) |
| Keyboard navigability for every primary action | PARTIAL (clipboard commands have no default keys, H1; sync rows one at a time) |
| Empty states with a CTA | PARTIAL (sync window: "Press Compare." / "No differences."; panels unchanged) |
| Inline edit instead of modal-then-save for single fields | PRESENT (F2 renames in place) |
| Bulk select + bulk action on list views | PARTIAL (panels yes; sync list no, H3) |
| Optimistic updates with rollback on error | N/A (jobs report per-item errors) |
| Undo for destructive actions, rather than a confirm modal | ABSENT for file ops (Trash + confirm) |
| Global search across the app | PRESENT (Find files over the index) |
| Saved views / filters as first-class objects | ABSENT (sync mode and sort are remembered; no saved comparisons or filters) |
| API access / webhook surface | N/A |
| Audit log for multi-user state changes | N/A |
| OS-level integrations | PARTIAL (Quick Look, Show in Finder, Open with, clipboard with Finder, Trash; no Services/notifications) |
| Accessibility: keyboard nav, focus rings, ARIA, contrast, screen reader | PARTIAL (keyboard and focus rings; ARIA largely missing) |

## Counts

Contradictions 4, overlaps 0, industry gaps 7, obsolete 0, safety nets 3.
Routes: polish 5 (K1, K2, K4, H1, N2), evolve 8 (H2–H7, N1, N3), this-pass 1 (K3).
Recheck: 10 of 15 earlier findings fixed, 5 open.

## Next work (evidence-based)

1. Polish batch, all trivial: K1, K2, K4, H1 (and K3 if the rename is wanted).
2. Sync list: row filters (H2) and multi-row actions (H3) — the two things TC and DC both have.
3. Undo of the last file operation (N1), now more useful since sync and paste exist.
