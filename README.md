# A Less Awful Editor

I wanted less bullshit between opening a file and understanding what is in it. A Less Awful Editor is a native Rust/GPUI editor project growing into a systems workbench: source, assembly, bytes, and patch/export tools in the same workspace.

**Working systems-workbench slice, unfinished IDE.** This checkout includes the
interactive UTF-8 editor, file tabs, linked source/assembly/byte/control-flow
views, binary metadata, bounded x86 analysis, patch preview/history, safe copy
export, and an optional native decompiler worker with a linked pseudocode view.
It is not a finished IDE or Ghidra replacement.

NEW EDITOR, Minimal IDE, and A Less Awful Editor are this same project.
NEW EDITOR is the authoritative project-state record.
[Checkpoint details and historical screenshot](docs/project-state.md).

Inspect a binary with **Ctrl+Shift+O**. Overview, x86/x86-64 assembly, bytes,
symbols/strings, control flow, pseudocode, and DWARF source navigation share a
file location. Instruction and byte patches are previewed, applied in memory,
and exported to a **new** file. The text buffer survives switching views. See
[controls and limits](docs/systems-workbench.md), [analysis evidence](docs/analysis-validation.md),
[decompiler evidence](docs/decompiler-validation.md), and the
[full capability contract](docs/systems-capabilities.md).

The compact native UI uses file tabs, aligned instruction columns, a grouped hex
grid and an inline command/patch area. Secondary controls live under Details;
placeholder panels start hidden.

The editor core has **no direct or transitive GPUI dependency**.
The separate binary-analysis core has the same boundary.
See [implemented behavior and limitations](docs/scratch-editor.md) and
[local validation evidence](docs/single-file-validation.md).

## What you can build from this checkout

- A Ropey text model with grapheme-aware navigation, selection, clipboard,
  undo/redo, and UTF-16 conversion at the native text-input boundary.
- GPUI rendering and viewport/scroll behavior in a separate view crate.
- Open, Save, and Save As with dirty-document decisions, captured save snapshots,
  external-change checks, and background file operations.
- Recoverable open/write failures that retain the current document.
- Bounded binary inspection, function/control-flow analysis, patch/export tools,
  and opt-in snapshot-bound native decompilation.
- Dependency checks that keep GPUI out of both pure cores.

The checked-in validation records separate local automated tests, native X11
acceptance, independent toolchain oracles, and unverified platform/UI boundaries.
They are local evidence, not passing remote CI or cross-platform acceptance.

## Run

Requires stable Rust, the checked-in lockfile, and GPUI's native dependencies:

```bash
cargo run --locked -p a-less-awful-editor
```

On Debian/Ubuntu-derived Linux systems:

```bash
sudo apt-get install build-essential clang cmake binutils nasm libasound2-dev \
  libfontconfig-dev libglib2.0-dev libssl-dev libvulkan1 \
  libwayland-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Use the portal backend appropriate to your desktop. Launch from a graphical
session with D-Bus and a Vulkan-capable driver. The file picker reports failures
without replacing the document. Build and run Cargo as your normal user; never
run Cargo with `sudo`.

Instruction assembly optionally uses `/usr/bin/nasm` (Debian/Parrot package
`nasm`). Inspection and byte patching work without it. The application passes one
validated instruction directly to NASM, with no shell or target execution.
GNU `binutils` and `cc` enable the independent ELF/DWARF oracle test; an explicit
skip is reported if they are unavailable.

## Architecture of this checkout

| Crate | Responsibility |
| --- | --- |
| `editor-core` | Rope, cursor/selection, history, text snapshots; no GPUI |
| `editor-view` | Native input and rendering boundary |
| `systems-core` | Pure binary metadata, decoding, source maps, and bounded analysis; no GPUI |
| `ui` | Application shell, prompts, background persistence, systems views, and external-worker coordination |
| `app` | Process startup and native window |

File I/O stays outside the pure text model. Saving a snapshot does not declare
later edits saved; closing during a save rechecks the dirty state.

## Verify

```bash
cargo fmt --all -- --check
python3 scripts/check-editor-core-boundary.py
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p a-less-awful-editor
```

`cargo check` alone does not demonstrate application linking or a native launch.

## Limits of this checkout

Strict UTF-8 regular files up to 16 MiB; symlinks are rejected. Existing-file
overwrite is intentionally Linux-only. The final external-change comparison
still has a race with another writer; this is not a filesystem transaction.
Power loss, full disks, network filesystems, macOS, Windows, Wayland, and native
IME candidate selection are not covered by the recorded acceptance.

[Implemented behavior](docs/scratch-editor.md) documents the details.
[Architecture](docs/architecture.md) describes this milestone's built boundary.
[Project state](docs/project-state.md) records the published checkpoints. The old
[checkpoint](docs/checkpoint.md) is historical context, not a current instruction
about a running desktop window.
