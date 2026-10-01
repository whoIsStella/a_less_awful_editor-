# Editor architecture

This page describes the checked-in single-file milestone of A Less Awful
Editor, also called NEW EDITOR / Minimal IDE. The later local systems workbench
is the same product and architecture, not a separate fork. See
[project state](project-state.md) for its reported checkpoint and publication gap.
The older [design archive](design/ide-plan.md) is historical planning, not a
current feature inventory.

## Built boundary

| Layer | Code | Owns |
| --- | --- | --- |
| Text model | `crates/editor-core` | Ropey text, cursor/selection, grapheme navigation, undo/redo, saved revisions, text snapshots |
| Native view | `crates/editor-view` | GPUI input protocol, UTF-16 conversion, rendering and viewport geometry |
| Application shell | `crates/ui` | Document coordination, dirty-state prompts, picker requests, background persistence |
| Startup | `crates/app` | Native application and window creation |

The core depends on Ropey and unicode-segmentation, not GPUI. The boundary script
checks Cargo's transitive dependency graph, rather than just inspecting imports.

## File lifecycle

Opening a file performs bounded reads and strict UTF-8 validation before
replacing the document. The current milestone accepts regular files up to
16 MiB and rejects symlinks.

Saving captures a text snapshot and runs file work on the background executor.
The coordinator records the revision actually saved, so edits made during I/O
remain dirty. A pending close rechecks that state after completion.

Disk-version comparisons detect many external changes. Conflict decisions,
picker cancellation, and read/write errors retain the document. The final
comparison and replacement are not atomic with respect to other writers;
filesystem races and durability limits remain documented.

See [implemented behavior and persistence limits](scratch-editor.md) and
[the recorded native acceptance](single-file-validation.md).

## Beyond this checkout

Tree-sitter, LSP, DAP, project navigation, Git integration, and a terminal are
design directions or placeholders in this milestone. The inspected remote
branches do not contain the later local binary views, linked locations, patch
history, and copy-export work. Their absence here does not mean the wider
project stopped at single-file editing. The analysis/decompiler integration
is still separate unverified WIP; do not infer a working product decompiler.
