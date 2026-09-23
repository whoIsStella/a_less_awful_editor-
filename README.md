# a_less_awful_editor-

A fast, native, local-first IDE that keeps the useful parts and removes the bullshit.

## Status

**M0 — native shell foundation**

The repository now has the first architectural boundary in code:

- Rust workspace
- GPUI application shell
- UI-independent rope-backed editor core
- separate GPUI editor renderer
- placeholder file tree, editor, bottom panel, and status bar
- CI for formatting, core tests, workspace compilation, and Clippy

The editor core intentionally has **no GPUI dependency**.

## Run

Requires the latest stable Rust toolchain and GPUI's platform dependencies.

```bash
cargo run -p a-less-awful-editor
```

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
