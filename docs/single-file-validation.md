# Single-file milestone validation — 2026-09-26

Checkout: existing `a_less_awful_editor-`, branch `feat/interactive-scratch`,
base commit `b9acf2650a3132776a5b24e69273aaaf9d87f3c4`.
Existing Rust-file edits, `target/`, and the previously untracked lockfile were
preserved. The lockfile was updated deliberately for persistence dependencies
and GPUI's test-support feature, then staged for inclusion. No push or merge.

## Local checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo test --locked --workspace` | Passed: 35 tests (17 core, 4 view, 14 UI/persistence) |
| `cargo check --locked --workspace` | Passed |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| `cargo build --locked -p a-less-awful-editor` | Passed, linked executable |
| `python3 scripts/check-editor-core-boundary.py` | Passed: core's four transitive dependencies contain no GPUI |
| `git diff --check` | Passed |

The initial baseline had 15 passing tests; format, check, and Clippy also passed.
New regressions cover line-ending-aware Enter, saved revision/history, background
snapshot completion, composition/focus protocol, scroll/resize geometry, file
errors/conflicts/permissions, confirmation cancellation, and save-before-reopen
of the same file. A GPUI test queues another input event after starting a save
but before completion, and verifies that a pending close asks again rather than
losing the new edit. Filesystem tests use controlled temporary directories,
including a hook that changes the destination just before the final save check.

## Native acceptance actually exercised

The linked executable launched on the existing Parrot X11 desktop. Further
interaction checks used an isolated Xvfb display, Marco window manager, GTK
portal, and Mesa lavapipe Vulkan driver. This exercised the native application,
not a webview or mock renderer. It is software-renderer evidence, not a claim
about every physical GPU or desktop configuration. Default Vulkan on Xvfb could
not create a surface (no DRI3); selecting lavapipe made the isolated run usable.
The first fresh portal session also took time to initialize its file picker.

Only files inside an `ale-acceptance-*` temporary directory were modified.

| Interaction | Observed result |
| --- | --- |
| Open temporary UTF-8 CRLF file with combining accent and joined emoji | Correct text, path, and clean indicator |
| Ctrl+End in a 501-line document | Line 501 visible with caret/status at the end |
| Shift+Home selection, multiline Unicode clipboard paste, Ctrl+Z/Ctrl+Y, Ctrl+S | Disk bytes matched independently constructed expected bytes using `cmp` |
| Close process, relaunch, reopen saved file, Ctrl+A/Ctrl+C | UTF8_STRING clipboard bytes matched expected bytes exactly |
| Untitled Save As; cancel then choose new temporary path | Cancellation preserved dirty buffer; successful save contained exact text |
| Open picker cancellation | Document retained |
| External write followed by Save | Conflict shown; Escape preserved external bytes; explicit Overwrite saved captured buffer |
| Open invalid UTF-8/binary fixture while dirty | Clear error in Output; previous path, text, and dirty state retained |
| Read-only destination write | Recoverable failure; independent disk and clipboard checks confirmed both contents intact and buffer dirty |
| Dirty native close (Alt+F4) and application quit (Ctrl+Q), then Cancel | Window and edits retained; focus returned to editor |
| Undo extra edit back to saved revision | Dirty marker cleared |
| Unmaximize and resize to 1100×850 | Native size verified with `xwininfo`; editor and confirmation rendered |
| Native close, choose Save, approve metadata conflict | Exact final bytes saved; application/session command exited with code 0 |

Linux confirmation detail clipping observed during acceptance was corrected with
a focused GPUI fallback dialog. A subsequent native conflict/close check verified
wrapped text, visible choices, Escape cancellation, Tab/Enter activation, and a
Cancel default. File errors are also available in the scrollable Output panel.

## Evidence limits

- No remote CI run was started; earlier runner-assignment failures are not
  reclassified as passing CI.
- OS-level IME candidate selection was not exercised. Composition UTF-16,
  commit/undo, document reset, and focus behavior have GPUI simulation coverage.
- Edits while a save is in flight were tested deterministically in the actual
  UI coordinator and file layer, not by timing a human edit against disk speed.
- Native Save As collision and a second external change after approval have
  automated coverage; the native external-change prompt was exercised directly.
- No power-loss, full-disk, network-filesystem, hostile directory-race, macOS,
  Windows, or Wayland acceptance claim. The remaining compare/rename race and
  filesystem durability assumptions are documented in [behavior](scratch-editor.md).
- The pre-existing active-desktop test window was left intact after typing was
  observed there. It runs the earlier in-session build; save its work before
  relaunching to use the final binary.
