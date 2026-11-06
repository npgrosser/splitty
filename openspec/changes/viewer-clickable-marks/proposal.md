# Proposal

## Why

The marks of annotated passages in a viewer pane lead nowhere: to read or answer a thread, the user finds its entry in the list, which has to be open. Hiding the list leaves the marks, and hiding the marks is a button inside the list, so both can only be switched while the list is open. A mark should lead to its thread, and marks and list should be switchable on their own.

Follow-up of roadmap item 111 (viewer annotations, `viewer-annotations`); no milestone of its own.

## What Changes

- A click on a marked passage, rendered or in the source, on a marked block of the rendered view or on a bar beside the source lines opens the annotation list if it is closed, chooses that annotation and gives its entry the keyboard. The document does not scroll: the passage is where the user pointed.
- Where marks overlap, the shortest passage is chosen first, a passage of the shown view before lines marked for the other view; further clicks on the same marks go through the others.
- The pointer over a mark shows the mark a click chooses in a stronger style and names the annotations under it (`#2 · Comment`) as a tooltip.
- A click that ends a selection, a drag, a double click, a click with a modifier key and a click on a link of a rendered document keep their current behavior.
- The **Hide Marks** button moves from the list's header to an eye button in the pane's bar beside the list's button, so marks and list can be switched while the list is closed. The context menu keeps its entry.
- While the marks are hidden, the passage of the annotation chosen in the list since then is marked, so choosing an entry still shows where it is.

Out of scope: keyboard navigation between marks, a thread popover at the passage, and anything that crosses the protocol: the change is window-only.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `viewer-annotations`: Annotations In The Pane: a mark leads to its thread, and hidden marks still show the passage chosen in the list.

## Impact

- `frontend/src/panes/viewer/annotation_layer.ts`: click and pointer handling over the document, the eye button, the passage chosen while the marks are hidden.
- `frontend/src/panes/viewer/annotation_list.ts`: the list header loses its Hide Marks button.
- `frontend/src/lib/viewer/mark_hits.ts` (new): which annotation a click on marks chooses.
- `frontend/src/lib/viewer/source_view.ts`, `frontend/src/lib/viewer/render.ts`: the marks under the pointer, and a style for the mark under it.
- `docs/files-and-editors.md`, `docs/architecture.md`.

## Architecture impact

None in the architecture model; `docs/architecture.md` names how the window finds the marks under the pointer.

## Decisions

- **D1 From a mark to its thread** (provisional, tbd): a click opens the list on the thread and moves the keyboard to its entry; overlapping marks go shortest first and further clicks on the same marks cycle through them; the pointer previews the mark a click chooses. The list already holds the thread with its reply box, so the click leads there instead of to a popover that would cover the text being read. Alternative: a popover with the thread at the passage, or a menu listing overlapping annotations.
- **D2 Hidden marks**: hiding the marks hides all of them, also the one chosen before; choosing an entry in the list, or saving an annotation, marks that one passage until the marks are shown or hidden again. Hiding then still answers where an annotation is.
- **D3 The eye in the pane's bar**: the switch for the marks sits beside the list's button and holds its state while the list is closed; the list keeps the switch for resolved marks, which only matters with the list open.
- **D4 Finding the marks under the pointer**: a source passage's decoration carries its annotation's ID, so the marks under the pointer are the decorations around its target; the rendered view tests the passages' client rectangles, falls back to the engine's caret position where nothing is laid out, and keeps which annotations marked each block. No mark gets DOM of its own in a rendered document, which stays as sanitized.
