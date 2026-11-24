# A Less Awful Editor

A native editor and binary workbench. Source, recovered C, assembly and bytes
belong in the same workspace. Moving between them should not lose your work.

Built in Rust with GPUI. Local files, local tools, no account.

The direction is Ghidra’s depth, Zed’s speed and Emacs’s reach: a compact IDE for
ordinary code, large refactors, hardware logic, firmware and systems work.
QEMU/KVM management, hardware design tools and project-wide refactoring are
still ahead. They are not features you can use today.

## What works now

Binary inspection does not execute the input. Patches stay in memory until you
export a separate copy. Recovered code is an approximation, not original source.

## Run

Requires stable Rust and the native libraries listed below. From the project folder:

```bash
cargo run --locked -p a-less-awful-editor
```

Open text files, a compiler location, a binary, or a folder directly:

```bash
cargo run --locked -p a-less-awful-editor -- src/main.c "src/input parser.c"
cargo run --locked -p a-less-awful-editor -- --line 12 --column 4 src/main.c
cargo run --locked -p a-less-awful-editor -- --binary build/firmware.elf
cargo run --locked -p a-less-awful-editor -- --folder .
```

Line and column numbers start at 1. Columns count Unicode scalar values.
On Unix, pass `--reuse-window` first to send text paths to a running editor on
the same display. This requires a private `XDG_RUNTIME_DIR`. For example,
`a-less-awful-editor --reuse-window --line 12 --column 4 src/main.c` navigates
in the open tab and preserves unsaved edits. A text launch accepts up to 32 paths.
A plain path opens text. Use `--binary` for executable files or raw bytes,
and `--folder` for the Files sidebar. Relative paths use the launch directory;
quote paths containing spaces. Use `-- --name.c` for a text filename beginning
with `-`.

Run with `--help` for usage without a graphical session. Invalid arguments
exit with status 2. Missing files, unsupported inputs, and read failures appear
in the window. New files start from the empty buffer and **Save As**.

### Linux setup

On Debian/Ubuntu-derived systems:

```bash
sudo apt-get install build-essential clang cmake libasound2-dev \
  libfontconfig-dev libglib2.0-dev libssl-dev libvulkan1 \
  libwayland-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Use the portal backend appropriate to your desktop. Launch from a graphical
session with D-Bus and a Vulkan-capable driver. Run Cargo as your normal user.
If the file picker fails, check that your desktop portal is running.

Instruction assembly also requires `/usr/bin/nasm` (`sudo apt-get install nasm`).
Inspection and byte patching work without it.

## Controls

| Task | Control |
| --- | --- |
| Open menus | F10, arrows, Enter; Escape closes |
| New / open text | Ctrl+N / Ctrl+O |
| Save / Save As | Ctrl+S / Ctrl+Shift+S |
| Close current document / quit | Ctrl+W / Ctrl+Q |
| Next text tab | Ctrl+Tab |
| Find / next / previous | Ctrl+F / F3 / Shift+F3 |
| Find a file by partial path | Ctrl+P |
| Search open folder | Ctrl+Shift+F |
| Go to Line | Ctrl+G |
| Move / select by word | Ctrl+Left/Right / Ctrl+Shift+Left/Right |
| Focus editor / terminal | Ctrl+Alt+E / Ctrl+Alt+T |
| Inspect binary | Ctrl+Shift+O |
| Text / overview / assembly / bytes | Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 |
| Browse a folder | File → Open Folder |
| Export patches | Binary → Export Copy |

Application shortcuts use Command on macOS. Text editing uses the bindings in
[the editing guide](docs/scratch-editor.md). In the terminal, control keys go to
the shell; Ctrl+Alt+E returns focus to the editor. Ctrl+C interrupts and
Ctrl+Shift+V pastes. Restart and Stop live in the Terminal menu.

Opening a folder does not run anything. Run your build command yourself.

## Limits: TODO

Up to 32 text documents per window, 16 MiB per input text file and 64 MiB per
binary. Text must be regular UTF-8. Symlink paths are rejected. Saving over an
existing text file is supported on Linux; other systems can Save As to a new file.
Unsaved documents are checked when closing or quitting. Reopen the same folder to restore its text tabs and recover unsaved drafts. Tab switching pauses during file operations so a save stays attached
to its document.

Find is literal and case-sensitive in the active text buffer. It scans a snapshot
in the background, counts all matches and navigates the first 10,000. Workspace Search honors ignore rules for disk files and open buffers.
It searches open unsaved buffers in place of disk contents. Case, whole-word and regex
options and comma-separated include/exclude globs are available. Results identify
incomplete scans. Search stops at 50,000 files and retains up to 10,000 matches. Files larger
than 16 MiB, binary files, `target` and `node_modules` are excluded. A named buffer remains searchable after its file is removed from disk.

To replace text, open Workspace Search, choose the search scope, enter search and
replacement text, and run the search. Deselect any matches to keep. Choose Preview
to review the edits, then Apply. Changed files appear as unsaved tabs. Save each
tab to write it to disk. Undo all restores the tabs that still contain the applied
revision. Files changed externally since preview are reported and skipped. If
the preview reaches its 32 MiB limit, narrow the search or deselect matches.

Quick Open indexes up to 50,000 filenames beneath the open folder using the workspace ignore rules.
Type part of a relative path and press Enter or click a result. While the picker is open,
it rescans two seconds after each scan completes. Escape closes it.

The folder tree loads on demand and uses explicit Refresh. It bounds listings,
cached folders, depth and visible rows. The terminal has resize and bounded
scrollback; output is monochrome, with no mouse text selection or full IME support.
Changing folders does not move an existing shell. Restart uses the selected
folder. Stop sends hangup to the shell and foreground group; detached daemons
are outside cleanup.

Syntax highlighting, language-server diagnostics, Git UI and a debugger are
unfinished. Ordinary byte patches do not repair relocations, signatures or
checksums. Function replacement has explicit format and relocation limits.

See the [text editing guide](docs/scratch-editor.md),
[binary workbench guide](docs/systems-workbench.md),
[native round-trip guide](docs/native-roundtrip.md) and
[decompiler setup](tools/native-decompiler/README.md).

## Check the build

```bash
scripts/check-editor.sh
```

This runs formatting, core dependency boundaries, workspace tests, Clippy and
application linking. The native decompiler acceptance test still requires the
pinned local worker; see the round-trip guide. A passing build is not a substitute
for opening the app and checking it on your desktop.
