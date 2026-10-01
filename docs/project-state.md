# Project state and screenshot provenance

NEW EDITOR, Minimal IDE, and A Less Awful Editor are names for one project.
NEW EDITOR is the authoritative project-state record. The systems workbench,
control-flow analysis, and optional decompiler integration are published in
draft PR #5; evidence remains bounded by the validation records below.

## Checkpoints

| State | Branch / commit | Evidence |
| --- | --- | --- |
| Single-file foundation | Main milestone `4db630bfdaef06bcd70e561398e0194517df3c39` | Checked-in record reports 35 tests and Linux X11 native acceptance |
| Systems workbench | PR #5 history through `4e9ffe3` | Checked-in record reports 76 tests plus native export/reopen and dirty-close acceptance |
| Control-flow analysis | PR #5 checkpoint `22515fb` | Checked-in record reports 85 tests, independent toolchain checks, and bounded native graph acceptance |
| Optional decompiler integration | PR #5 checkpoint `37d1d1f` | Checked-in record reports 90 tests with the configured worker and real-worker state coverage; native pseudocode-view acceptance remains open |

The current verified workbench slice includes file tabs, linked source/assembly/byte
locations, binary metadata, x86 decoding, aligned assembly/hex views,
patch preview/history, safe copy export, and separate text/binary dirty-close
decisions. The recorded native acceptance includes exact export/reopen checks,
compact and zoomed layouts, cancellation, and clean/approved close.

The validation records are `docs/single-file-validation.md`,
`docs/systems-validation.md`, `docs/analysis-validation.md`, and
`docs/decompiler-validation.md`. They distinguish automated, native, oracle,
and unverified evidence; publication does not strengthen those claims.

Do not substitute these notes for runnable source. IME, other OS/Wayland paths,
durability, and filesystem races remain unverified; the record does not prove
a finished decompiler, broad processor/ABI coverage, or a universal workspace
backend.

## Historical screenshot

![September 25 native scratch editor with an unsaved in-memory buffer](assets/scratch-editor-2026-09-25.png)

This existing September 25 screenshot shows the earlier running scratch editor,
including the explicit unsaved-buffer and nonfunctional-terminal notices.
It predates both the single-file persistence milestone and the local systems
workbench. It demonstrates the native shell, not current binary or export
features. The original pixels are preserved.

A second September 25 image showed an earlier shell with a file tree and
`$ cargo run` display. It was not selected as feature evidence because those
UI placeholders could imply implemented navigation or a working terminal.
