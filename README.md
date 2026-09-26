# a_less_awful_editor-

A fast, native, local-first IDE that keeps the useful parts and removes the bullshit.

## Status

**Interactive editor + safe single-file persistence**

The existing Rust/GPUI application supports native text editing, selection,
clipboard, undo/redo, Unicode grapheme navigation, scrolling, and Open/Save/Save As.
Dirty documents are protected before replacement or closing. File navigation,
terminal, diagnostics, Git, and debugging remain honest placeholders.

The editor core has **no direct or transitive GPUI dependency**.
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

## Workspace

```text
crates/
├── app/          # process startup + native window
├── ui/           # GPUI application shell
├── editor-core/  # text/editor state; no UI framework
└── editor-view/  # GPUI rendering boundary
```

## Design

See [docs/architecture.md](docs/architecture.md) for the system design and architectural invariants.
