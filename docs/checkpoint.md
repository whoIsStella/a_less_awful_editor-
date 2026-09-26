# Paused checkpoint — 2026-09-26

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
