# Editor architecture

This page describes the checked-in architecture of A Less Awful Editor, also
called NEW EDITOR / Minimal IDE. The single-file editor and systems workbench
are one product and architecture, not separate forks. See
[project state](project-state.md) for checkpoint and evidence boundaries.
The older [design archive](design/ide-plan.md) is historical planning, not a
current feature inventory.

## Implemented boundary

The diagrams below describe the long-term design, not currently connected
subsystems. The implemented crates are `app`, `ui`, `editor-core`, and
`editor-view`, plus `systems-core` for the first binary-workstation slice.
Single-file persistence lives in `ui::persistence` and runs on
background workers; it does not introduce a workspace service or filesystem
operations in the pure editing core. See [current behavior](scratch-editor.md).

`systems-core` owns immutable bytes, ELF/PE/Mach-O section mappings, symbols,
bounded x86 decoding and DWARF line maps. It has no GPUI, filesystem, or process
operations. `ui::systems` renders bounded views of one selected file offset;
`ui::systems_shell` owns binary/source lifecycle and independent dirty guards;
`ui::systems_io` performs bounded reads, create-only exports and optional NASM
assembly on background workers; `ui::systems_analysis` owns snapshot-bound
background function/control-flow analysis; and `ui::systems_decompiler` owns the
bounded optional external-worker lifecycle. Source metadata does not automatically
open a file. A source-navigation action uses the guarded text-open workflow.

The implementation provides bounded direct control-flow recovery and optional
native decompilation for its documented Linux x86-64 contract. It does not provide
a persistent analysis database, indirect control-flow recovery, broad processor/
ABI coverage, or debugging. See [implemented systems behavior](systems-workbench.md),
[analysis evidence](analysis-validation.md), [decompiler evidence](decompiler-validation.md),
and [full systems capability obligations](systems-capabilities.md).

## Built boundary

| Layer | Code | Owns |
| --- | --- | --- |
| Text model | `crates/editor-core` | Ropey text, cursor/selection, grapheme navigation, undo/redo, saved revisions, text snapshots |
| Native view | `crates/editor-view` | GPUI input protocol, UTF-16 conversion, rendering and viewport geometry |
| Binary/analysis model | `crates/systems-core` | Immutable snapshots, binary mappings, decoding, source maps, function candidates, control flow and references |
| Application shell | `crates/ui` | Document coordination, dirty-state prompts, persistence, systems views, patch/export work, and external-worker coordination |
| Startup | `crates/app` | Native application and window creation |

Both pure cores exclude GPUI. The boundary script checks Cargo's transitive
dependency graph, rather than just inspecting imports.

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

## Beyond this checkpoint

Tree-sitter, LSP, DAP, project navigation, Git integration, and a terminal are
design directions or placeholders. The checked-in analysis and decompiler paths
are deliberately bounded and do not establish feature parity with a mature
reverse-engineering platform. Consult the validation records before inferring
native UI, operating-system, processor, ABI, or decompiler acceptance.
