# Single-file editor

The editor starts with an empty **Untitled** buffer. Nothing is written until
Save or Save As succeeds. A `*` in the document label and native title indicates
unsaved edits. The label shows the full selected path; the title identifies the open document.

Use Open to choose a file. The Output panel displays recoverable file errors.
The editor does not include a file browser, terminal, Git client, or debugger.

## Controls

- Ctrl+O opens a single file through the native file picker.
- Ctrl+S saves; an untitled document asks for a destination.
- Ctrl+Shift+S saves under another path. Existing destinations require explicit
  confirmation, even if the platform picker already asked.
- Ctrl+W, Ctrl+Q, the application's Quit menu, and the native window close button
  use the same Save / Discard / Cancel guard. Saving before continuing rechecks
  edits that arrived during the save. Cancelling any step keeps the document.
- On macOS the application shortcuts use Command.

Click to position the caret, drag or Shift-click to select. Arrows,
Shift+arrows, Home/End, Ctrl+Home/End, Enter, Tab, Backspace/Delete, and
Ctrl+A/C/X/V work in the editor. Ctrl+Z undoes; Ctrl+Shift+Z or Ctrl+Y redoes.
The caret is steady. Wheel scrolling and caret reveal have no 200-line cutoff.

Linux confirmations use a small GPUI dialog with wrapping, scrollable detail,
and keyboard support. Escape cancels. Tab/Shift+Tab or Up/Down choose a button;
Enter activates it. Cancel is selected initially. Native file pickers require
a functioning desktop portal (see [Linux setup](../README.md#linux-setup)).

## Saving while editing

You can continue editing during a save. Edits made after saving begins remain
unsaved and keep the `*` indicator. Undoing back to the saved version clears it.
Retyping the same content can still leave the document marked as changed.

## File safety and limits

Only regular strict UTF-8 text up to **16 MiB** is supported. Invalid UTF-8,
NUL and control characters other than Tab/CR/LF are rejected with an error.
UTF-8 BOMs, Unicode, existing LF/CRLF/CR, and mixed endings round-trip unchanged.
Enter uses the first existing line-ending style (LF for a new buffer); pasted
text keeps its supplied endings. Encoding conversion is not implemented.

A failed or cancelled Open leaves the existing document intact. A failed or
cancelled Save leaves its buffer, path, and saved revision intact. Errors are
shown in the status bar and the scrollable Output panel.

Saving writes a uniquely named temporary file in the destination directory,
flushes and syncs it, checks the destination again, then installs the temporary
file. A new destination uses create-only persistence; an existing destination
uses atomic replacement. On Unix the parent directory is synced after install.
A directory-sync failure explicitly reports that the file was replaced but its
power-loss durability is uncertain, and leaves the buffer dirty.

Conflict detection compares disk bytes, modification time, and Unix identity /
permission metadata against the version read or last saved. External changes,
replacement, deletion, or a Save As collision require an explicit Overwrite
choice. That choice is tied to the observed version; a subsequent change causes
another conflict. Cancel always leaves the external file untouched.

Symbolic links, including symlink parent directories, are rejected. Read-only
files, multiply linked files, special permission bits, and ownership/group that
cannot be retained are rejected for overwrite. Ordinary Unix mode bits are
preserved; new files use tempfile's restrictive permissions. On Linux, files
with extended attributes/ACLs (or unreadable attribute lists) are rejected rather
than silently losing metadata. Existing-file overwrite is explicitly rejected
on other operating systems until their metadata preservation policy is implemented. Use Save As to a new regular file when a case is
unsupported.

**Concurrency/durability limits:** the last comparison and replacement are not
one compare-and-swap operation. Another writer can race between them; parent
path components can also race. There is no cross-process lock, watcher, backup,
crash recovery, or protection against hostile directory mutation. Reads cannot
promise a coherent snapshot against writers changing data in place while
restoring metadata. Atomic rename/sync behavior depends on the filesystem;
durability depends on your storage and filesystem. Process
kill, desktop session termination, and GPUI's already-committed shutdown callback
cannot be cancelled; the application guards its Quit action and window-close
request *before* shutdown. Input-method behavior may vary with your desktop.

## Editing limits

No syntax highlighting, multi-cursor, word navigation, soft wrapping, selection
autoscroll outside the pane, or accessibility adapter. Tab stops count scalars,
not full Unicode display-cell widths. Selection in mixed right-to-left and left-to-right text may be inaccurate. Very long lines are shaped/scanned in full; very long lines can slow down editing. The history limit bounds entries, not retained bytes.
