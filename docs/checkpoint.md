# Active systems continuation — 2026-10-01

The user explicitly resumed systems work after both saved branches were pushed.
Current branch: `codex/analysis-wip-20260927`. The prior runnable checkpoint stays
on `codex/systems-workbench` at `5affb7c`. No merge has been performed.

The resumed branch now integrates bounded function/control-flow analysis, native
Flow graphs and references with shared locations and patch/history invalidation.
All 85 workspace tests, format, check, Clippy, application linking and GPUI-free
core boundary checks passed. Native X11 tests exercised graph selection, byte
navigation, patches/undo/redo, reference links and resize; the isolated display
terminated before approved-close exit could be verified in this run.

See [analysis validation](analysis-validation.md) for exact evidence and remaining
obligations. The full systems goal is still active. Next: checked mapped snapshots,
bounded native decompiler process runner, pseudocode and address-token navigation.
The native worker fixture passes but is not yet connected to the IDE.

## Historical checkpoints

# Working checkpoint — 2026-09-27

The single-file milestone below was subsequently merged as PR #1. Current work
continues from merge commit `4db630b` on `codex/systems-workbench`.

The first systems slice adds linked source/assembly/byte locations, binary
metadata, x86 decoding, patch preview/history, safe copy export, independent
text/binary close guards, and small view controls. The UI now has compact native
chrome, file tabs, aligned assembly/hex lists and an inline command area; secondary
controls are tucked into Details. The latest local suite passes 76 tests, and
native acceptance covers exact export/reopen, compact/zoomed layouts, cancellation,
and normal process exit after clean close and approved dirty text/binary closes. It preserves the existing
native editor. See [behavior](systems-workbench.md) and
[validation](systems-validation.md). The [capability contract](systems-capabilities.md)
retains the full Ghidra-level goal; this slice does not complete it.

Next integration work should use the verified installed-backend inventory.
Rizin static analysis is available, but `pdgj` is absent; Cutter currently has
unresolved Rizin 0.7 libraries. An opt-in pinned C++ Ghidra experiment verifies
Java-free pseudocode/address-token mapping. No backend or system package was
installed or changed. The experiment is not connected to the IDE yet.

The feature changes are local until explicitly published. Keep the existing
lockfile and target directory, inspect status before continuing, and retain the
evidence boundary between automated tests, native acceptance and remote CI.

```sh
cargo run --locked -p a-less-awful-editor
```

## Historical pause — 2026-09-26

The single-file persistence milestone is implemented on `feat/interactive-scratch`.
The requested local checks pass: format, 35 workspace tests, workspace check,
Clippy with warnings denied, application linking, and the GPUI-free core boundary.
Native X11 acceptance verified open/edit/select/paste/undo/redo/save/reopen with
exact byte comparisons, Save As, cancellation, conflict decisions, read/write
failure retention, resizing, and normal process exit after approved close.

See [validation](single-file-validation.md) for exact commands and evidence, and
[behavior](scratch-editor.md) for implementation and safety limitations.

The existing user edits were preserved and included in this local checkpoint.
`Cargo.lock` is now tracked. `target/` was retained. No push, merge, reset, or
repository replacement was performed. No next milestone has been started.

Remaining acceptance work: target-desktop IME candidate interaction and remote
CI. macOS, Windows, Wayland, power-loss/full-disk behavior, and network filesystems
are unverified; existing-file overwrite is intentionally Linux-only. The final
pre-save comparison does not eliminate races with other writers.

An earlier in-session build remains open on the user's original desktop with
observed typing preserved. Save that window's work before closing it. Relaunch
the final build from the repository with:

```sh
cargo run --locked -p a-less-awful-editor
```

Resume by inspecting Git status and this checkpoint. Do not start Tree-sitter,
LSP, Git, terminal, or debugger work without the next task's scope.
