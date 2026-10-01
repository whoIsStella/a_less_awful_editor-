# Systems slice validation — 2026-09-27

This verifies the linked source/assembly/bytes slice, not the complete
[Ghidra-level capability contract](systems-capabilities.md).

## Local checks

All of these passed against the final Rust changes:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p a-less-awful-editor
python3 scripts/check-editor-core-boundary.py
git diff --check
```

There are 76 tests: 17 editor-core, 4 editor-view, 16 systems-core, one independent
GNU toolchain oracle, and 38 UI/persistence/I/O tests. The oracle compiled a
temporary C fixture, then compared symbols, section/virtual/file mappings,
instruction boundaries/bytes/mnemonics/branches and DWARF source intervals with
`readelf`, `objdump` and `addr2line`. The fixture was never executed. NASM tests
used the real installed assembler. Pure-core dependency traversal found four
transitive dependencies for editor-core and ten for systems-core, with no GPUI.

The GPUI tests exercise shared locations, actual source round trips at later
instructions and interior bytes, patch history, no-op/malformed previews,
input/location changes during assembly, captured export snapshots, new edits
during export, cancellation, failed writes, and independent text/binary close
decisions. Existing text selection, Unicode/CRLF, IME-handler simulation and
scrolling/resize regressions continue to pass.

Independent review found and prompted fixes for stale previews, duplicate GPUI
element IDs, source round trips that lost exact offsets, modal mouse occlusion,
and close decisions outliving their document/revision. Close decisions now carry
content revision and binary entity identity, are cleared on cancellation/abort,
and are rechecked after asynchronous export. A stale decision cannot authorize
discarding a different binary whose numeric revision happens to match.

## Native acceptance

Used the built application on isolated X11 (`Xvfb :99` and `:100`, 1600x1100, Marco,
Mesa lavapipe and GTK portals), with controlled files under
`/tmp/ale-systems-accept-0rl9r_c1`. This is a real native-window run on software
rendering, not proof of hardware-GPU, Wayland or cross-platform behavior.

- Opened a compiled ELF through the native binary picker while retaining a dirty
  text document.
- Used virtual address `0x401110` / file offset `0x1110`, moved through assembly
  and DWARF source line 3, then returned to the same instruction rather than the
  first instruction on that source line.
- Cancelled source replacement and attempted background clicks/typing during
  the Linux confirmation. Clipboard byte comparison confirmed exact retention
  of combining Unicode, emoji, multiline text and CRLF.
- Saved the retained text through native Save As after paste and undo/redo;
  compared the entire saved file with the expected UTF-8/CRLF bytes.
- Assembled `add eax, 2` over `add eax, 1`; copied the actual native preview and
  checked `83 c0 02` before applying it in memory.

- Applied the preview, confirmed the changed byte in Bytes and dirty marker,
  undid/redid it with the clean/dirty indicator following history.
- Cancelled the native close prompt with patches retained.
- Exported to `patched.elf`, compared the entire file against an independently
  constructed expected image, and checked the entire original remained unchanged.
  The sole change was file offset `0x1112`, from `01` to `02`.
- Reopened that copy through the native picker and decoded `add eax, 2` at the
  same location. No imported or patched executable was run.

Native testing found that unchanged input focus/unmark notifications dismissed
assembly previews. Invalidation now follows the input content revision; regression
coverage distinguishes focus/clipboard changes from actual edits.

Clean native Alt+F4 initially exposed a GPUI 0.2.2 X11 RefCell panic. That backend
holds its state borrow through the should-close callback. The callback now queues
the existing guarded close flow on the foreground executor; a simple defer would
flush too early. Two regressions check callback ordering and dirty text/binary
protection. Rebuilt native clean Alt+F4 exited with status 0. A second rebuilt run cancelled
an export picker, retained dirty patches, cancelled native close, then approved
Discard patches via the keyboard. Its shell supervisor ended with status 143,
so that run does not establish a normal dirty-close process exit. Fresh bounded
runs then separately verified dirty text Discard and dirty binary Discard patches
through native Alt+F4, both with captured application exit status 0.

## Native visual review

The UI pass replaces the large pill controls with compact Inter chrome, filename
tabs, aligned assembly columns, a grouped hex grid and inline command/preview
rows. Secondary controls remain under Details. Shell placeholders start hidden.
No webview, UI framework, copied asset or runtime dependency was added.

Inspected the real native application at 1280x800 and 640x450 logical content
sizes, plus the preceding 800x550-equivalent 2x display. At 640x450, Details,
Panels and a preview stayed reachable together; the optional bottom panel shrinks
instead of covering controls. Increased assembly type to 20px and verified that
addresses, byte encodings and instructions no longer overlap. Clicking the active
binary document tab retained Bytes rather than resetting to Assembly. A regression
also checks all lenses and retained scroll, plus a real DWARF source round trip.

[Assembly screenshot](screenshots/workbench-assembly.png) and
[byte screenshot](screenshots/workbench-bytes.png) show the actual native build
on controlled fixtures. This records visual inspection, not user aesthetic
acceptance or a performance benchmark.

## Separate backend evidence

The opt-in `scripts/verify-native-decompiler.py` experiment was run with its
existing pinned cache and passed pseudocode-to-instruction mappings at `0x1000`
and `0x1003`. This is native Ghidra feasibility, not IDE decompiler integration.
Installed Rizin static analysis also worked; its `pdgj` command was absent.
See the [backend inventory and experiment](systems-capabilities.md) for exact
revision, probes, remaining type/protocol work and Cutter loader incompatibility.

## Residuals

No new remote CI run was submitted. Earlier CI failed before assigning a runner
because of account billing/spending limits; local checks are the current evidence.
One earlier isolated X server terminated during acceptance; the older process
reported Vulkan surface loss. Display-server loss recovery was not established.
GTK portal dialogs sometimes needed a click to gain focus in this isolated setup;
keyboard-only portal focus behavior on the user's desktop is not established.
Native IME candidate windows, Wayland, macOS, Windows, hardware rendering,
power-loss/disk-full durability and hostile-file process isolation are unverified.
The tested preview/export interleavings use the GPUI test executor; native disk
timing was not used to claim exhaustive concurrency coverage.

General analysis cancellation, saved layouts/annotations, broad processor modes,
control-flow analysis, decompilation/type recovery, debugger/emulator, comparison,
and the remaining capability-contract rows are still open. The first slice must
not be described as completing the user's full systems-workstation goal.
