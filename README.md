# A Less Awful Editor

I wanted less bullshit between opening a file and understanding what is in it. A Less Awful Editor is a native Rust/GPUI editor project growing into a systems workbench: source, assembly, bytes, and patch/export tools in the same workspace.

**Unfinished; this GitHub checkout is behind the local workbench milestone.**
You can build the single-file UTF-8 editor here. The later local checkpoint
reports linked source/assembly/byte views, x86 decoding, patch previews, and
safe copy export, with 76 tests and native acceptance. That source is not yet
on the inspected remote branches; those results have not been rerun here.
The decompiler integration remains separate, unverified work in progress.

NEW EDITOR, Minimal IDE, and A Less Awful Editor refer to this same project.
[Checkpoint details and the historical screenshot](docs/project-state.md)
explain what is available in this checkout and what still needs publishing.

## What you can build from this checkout

- A Ropey text model with grapheme-aware navigation, selection, clipboard,
  undo/redo, and UTF-16 conversion at the native text-input boundary.
- GPUI rendering and viewport/scroll behavior in a separate view crate.
- Open, Save, and Save As with dirty-document decisions, captured save snapshots,
  external-change checks, and background file operations.
- Recoverable open/write failures that retain the current document.
- A dependency check that keeps GPUI out of the editing core.

The checked-in [validation record](docs/single-file-validation.md) reports
35 workspace tests and Linux X11 native acceptance with independent byte checks.
That is prior local evidence, not a passing remote-CI or cross-platform claim.

## Run

Requires stable Rust, the checked-in lockfile, and GPUI's native dependencies:

```bash
cargo run --locked -p a-less-awful-editor
```

On Debian/Ubuntu-derived Linux systems:

```bash
sudo apt-get install build-essential clang cmake libasound2-dev \
  libfontconfig-dev libglib2.0-dev libssl-dev libvulkan1 \
  libwayland-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Use the portal backend appropriate to your desktop. Launch from a graphical
session with D-Bus and a Vulkan-capable driver. The file picker reports failures
without replacing the document. Build and run Cargo as your normal user.

## Architecture of this checkout

| Crate | Responsibility |
| --- | --- |
| `editor-core` | Rope, cursor/selection, history, text snapshots; no GPUI |
| `editor-view` | Native input and rendering boundary |
| `ui` | Application shell, prompts, background persistence |
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
[Project state](docs/project-state.md) records the later workbench checkpoint. The old [checkpoint](docs/checkpoint.md)
is historical context, not a current instruction about a running desktop window.
