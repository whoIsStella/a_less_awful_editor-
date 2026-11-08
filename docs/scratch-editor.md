# Text editor

The editor starts with an empty **Untitled** buffer and retains up to 32 text
buffers in tabs. Each keeps its own edits, selection and undo history. A dot in
the tab and a `*` in the native title mark unsaved changes. Nothing is written
until Save or Save As succeeds. Reopening an open path selects its existing
buffer. Tabs and unsaved text are not restored after a restart.

Use Open to choose a file. The Output panel displays recoverable file errors.
Use **Open Folder** to select a root and **Files** to show or hide its tree.
Click a folder's row or disclosure arrow to expand or collapse its children.
Parent folders and siblings remain visible. Open a text file by selecting its
row; the current file is highlighted. Opening another file retains your unsaved text in its tab.
**Refresh** reloads open branches without collapsing them. Expansion is retained
while browsing in this session, but is not saved across restarts.

The tree reads directories in the background as you expand them. Loading,
empty-folder and read-error messages appear below the affected folder.
Symbolic links are skipped. Limits are 500 entries per directory, 128 cached
folders, 32 levels of nesting and 2,500 visible rows. Collapse folders and refresh
to free the folder cache when its limit is reached.

The Terminal panel provides a Linux PTY shell with interrupt, paste, resize,
stop and restart. Open Folder only browses files: run your build command
explicitly in the terminal. Language diagnostics, a Git client and a debugger
remain unimplemented.
Selecting a different folder leaves the current shell in its existing working
directory. Restart shell starts a new shell in the selected folder.
While the terminal has focus, control keys go to the terminal. Use Ctrl+Alt+E to focus the editor before using Ctrl+S or Ctrl+Q for the document.
Ctrl+Alt+T focuses the terminal.

## Controls

- F10 opens the menu bar. Arrow keys navigate; Enter activates; Escape returns focus.
- Ctrl+N creates a text buffer; Ctrl+O opens a file; Ctrl+Tab cycles text tabs.
- Ctrl+F opens literal, case-sensitive Find in the active buffer. Enter/F3 finds the next
  match; Shift+F3 finds the previous one. Escape closes the input; F3 retains the query.
  Searches use a background snapshot; stale results cannot select another document.
  All matches are counted; navigation is bounded to the first 10,000 matches.
- Ctrl+G accepts a one-based line number; Enter goes there; Escape cancels.
- Ctrl+S saves; an untitled document asks for a destination.
- Ctrl+Shift+S saves under another path. Existing destinations require explicit
  confirmation, even if the platform picker already asked.
- Ctrl+W closes the current text or binary document. Ctrl+Q, File → Quit and the
  native close button check every unsaved text buffer and pending binary patches.
  Saving rechecks edits that arrived during the save. Cancelling a quit retains
  every open buffer, including ones already approved for discard.
- On macOS the application shortcuts use Command.

Click to position the caret, drag or Shift-click to select. Arrows,
Shift+arrows, Ctrl+Left/Right (word movement), Ctrl+Shift+Left/Right (word selection), Home/End, Ctrl+Home/End, Enter, Tab, Backspace/Delete, and
Ctrl+A/C/X/V work in the editor. Ctrl+Z undoes; Ctrl+Shift+Z or Ctrl+Y redoes.
The caret is steady. Wheel scrolling and caret reveal have no 200-line cutoff.

Linux confirmations use a small GPUI dialog with wrapping, scrollable detail,
and keyboard support. Escape cancels. Tab/Shift+Tab or Up/Down choose a button;
Enter activates it. Cancel is selected initially. Native file pickers require
a functioning desktop portal (see [Linux setup](../README.md#linux-setup)).

## Saving while editing

You can continue editing during a save. Edits made after saving begins remain
unsaved and keep the dirty indicator. Tab switching is blocked until the file operation finishes. Undoing back to the saved version clears it.
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

No syntax highlighting, multi-cursor, soft wrapping, selection
autoscroll outside the pane, or accessibility adapter. Tab stops count scalars,
not full Unicode display-cell widths. Selection in mixed right-to-left and left-to-right text may be inaccurate. Very long lines are shaped/scanned in full; very long lines can slow down editing. The history limit bounds entries, not retained bytes.
