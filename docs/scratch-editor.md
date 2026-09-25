# Interactive scratch editor

## Scope

This increment replaces the static center pane with a stateful, in-memory scratch
buffer. It does **not** open, overwrite, or save project files. Closing the app
loses scratch contents. The tab and status bar state that explicitly.

File navigation, terminal execution, language diagnostics, and debugging remain
unimplemented. Bottom-panel tabs change their explanatory placeholder content;
they do not start services. Fake filenames, terminal prompts, Git branch labels,
and error counts have been removed.

## Implementation

- `editor-core` owns the rope, scalar-indexed selections, grapheme navigation and
  deletion, UTF-16 conversions, and a 256-entry undo history. It has no GPUI
  dependency. Native IME preedit updates are grouped into one undo operation.
- `editor-view` is a focused GPUI entity. A native `EntityInputHandler` accepts
  text and composition updates. Keyboard events handle editing commands only;
  printable text is not reconstructed from physical keycodes.
- A custom element shapes visible lines, paints selections and a steady caret,
  and caches the same geometry for mouse hit testing. Tabs expand only in the
  display mapping. The old 200-line display cutoff is removed.
- Wheel scrolling and caret reveal share viewport state. Line/column status is
  derived from the buffer rather than a hard-coded label.

GPUI integration was written against the published **0.2.2** interfaces, not the
newer upstream `gpui_platform` API:

- https://docs.rs/crate/gpui/0.2.2/source/examples/input.rs
- https://docs.rs/gpui/0.2.2/gpui/trait.EntityInputHandler.html
- https://docs.rs/ropey/1.6.1/ropey/struct.Rope.html

## Controls

Click the center pane to focus and position the caret. Drag or Shift-click to
select. Use arrows, Shift+arrows, Home/End, Ctrl+Home/End, Enter, Tab,
Backspace/Delete. Ctrl+A/C/X/V select all/copy/cut/paste. Ctrl+Z undoes;
Ctrl+Shift+Z or Ctrl+Y redoes. On macOS use Command for the clipboard/history
shortcuts. The caret is steady, not blinking, in this increment.

## Verification

The new direct `unicode-segmentation` dependency requires a one-time update of the
workspace lockfile; do not delete an existing lockfile or `target/` directory.

```sh
cargo test -p ale-editor-core
cargo test --locked -p ale-editor-view
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo run --locked -p a-less-awful-editor
```

The core suite includes the two original tests and regression cases for empty
buffers, multiline edits, reversed selections, undo/redo invalidation, combining
characters, emoji, CRLF boundaries, vertical column retention, surrogate-pair
ranges, composition undo, invalid ranges, and edits beyond line 200. Renderer
unit tests exercise tab/Unicode mapping and empty-line caret mapping.

**These added tests and the new GUI have not been executed in the authoring
environment.** It has no Rust toolchain and cannot resolve package-host DNS.
Static API review is not compilation or runtime verification. Keep this increment
on its feature branch until the commands above and the manual checks below pass.

### Manual acceptance

Type multiple lines, move the caret, select in both directions, replace a
selection, paste multiline Unicode text, undo and redo. Test `e` plus a combining
accent, a joined emoji, and pasted CRLF text. Confirm scrolling reaches lines
beyond 200 and the caret returns to view after a keyboard edit. Test native input
composition on the target desktop rather than assuming the protocol integration
is sufficient. Click each bottom tab and check that its placeholder changes.
Resize, close, and confirm the process exits normally.

### Remaining limitations

No file persistence/recovery, syntax highlighting, multi-cursor, word navigation,
soft wrapping, selection autoscroll outside the pane, or accessibility adapter.
Tab stops count source scalars, not full Unicode display-cell widths. Complex
bidirectional selection geometry and platform IME behavior need further testing.
Very long individual lines are shaped/scanned in full; no giant-file or latency
claim is made. The history limit bounds undo entries, not total retained bytes.
