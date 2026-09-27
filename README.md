# a_less_awful_editor-

A fast, native, local-first IDE that keeps the useful parts and removes the bullshit.

## Status

**Interactive editor + safe single-file persistence + linked binary views**

The existing Rust/GPUI application supports native text editing, selection,
clipboard, undo/redo, Unicode grapheme navigation, scrolling, and Open/Save/Save As.
Dirty documents are protected before replacement or closing. File navigation,
terminal, diagnostics, Git, and debugging remain honest placeholders.

Inspect a binary with **Ctrl+Shift+O**. Overview, x86/x86-64 assembly, bytes,
symbols/strings, and DWARF source navigation share a file location. Instruction
and byte patches are previewed, applied in memory, and exported to a **new** file.
The text buffer survives switching views. This is the first systems-workstation
slice; it is not yet a Ghidra replacement. See [controls and limits](docs/systems-workbench.md)
and the [full capability contract](docs/systems-capabilities.md).

The compact native UI uses file tabs, aligned instruction columns, a grouped hex
grid and an inline command/patch area. Secondary controls live under Details;
placeholder panels start hidden.

The editor core has **no direct or transitive GPUI dependency**.
The separate binary-analysis core has the same boundary.
See [implemented behavior and limitations](docs/scratch-editor.md) and
[local validation evidence](docs/single-file-validation.md).

## Run

Requires stable Rust and GPUI's platform dependencies. Keep the checked-in lockfile.

```bash
cargo run --locked -p a-less-awful-editor
```

## Linux setup

On Debian/Ubuntu/Parrot, install missing system packages with your package manager:

```bash
sudo apt-get install build-essential clang cmake libasound2-dev \
  libfontconfig-dev libglib2.0-dev libssl-dev libvulkan1 \
  libwayland-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Use the desktop-appropriate portal backend if it is not GTK. Run from a graphical
login with a working session D-Bus and Vulkan-capable display/driver. Never run
Cargo with sudo. `cargo check` alone does not link or launch the application;
use `cargo build --locked -p a-less-awful-editor` and then launch it. A valid
file-picker request may fail if the desktop portal is unavailable; the app keeps
the document and reports the error.

Instruction assembly optionally uses `/usr/bin/nasm` (Debian/Parrot package
`nasm`). Inspection and byte patching work without it. The application passes one
validated instruction directly to NASM, with no shell or target execution.
GNU `binutils` and `cc` enable the independent ELF/DWARF oracle test; an explicit
skip is reported if they are unavailable.

## Workspace

```text
crates/
├── app/          # process startup + native window
├── ui/           # GPUI application shell
├── editor-core/  # text/editor state; no UI framework
├── editor-view/  # GPUI rendering boundary
└── systems-core/ # pure binary metadata, decoding, source maps, byte statistics
```

## Design

See [docs/architecture.md](docs/architecture.md) for the system design and architectural invariants.
