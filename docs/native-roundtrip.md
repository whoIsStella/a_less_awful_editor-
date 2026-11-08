# Edit recovered C and rebuild a function

The native round-trip path replaces a named function in a Linux x86-64 ELF
image. Choose a fixed-span replacement or opt in to a bounded relocated
replacement when the new function needs more space or supported calls and data. It uses the pinned native decompiler, the normal text editor,
the host C compiler, a byte preview, and create-only binary export. It is an
initial part of a broader systems workbench, not a general binary relinker.

## Prepare

Build and configure the [native decompiler](../tools/native-decompiler/README.md).
The compiler path is `/usr/bin/cc`. No compiler packages are installed by the
editor. This path currently requires Linux and x86-64 ELF output.

## Round trip

1. Use **Open Folder** to browse your project and open its source. Save source
   edits, then open **Terminal** and run your project's build command. Folder
   browsing does not build files automatically.
2. **Inspect** the resulting ELF executable or shared object. Select a function
   and use **Flow → Calls** to explore its callers and outgoing calls. A call
   location opens its instruction in **Assembly**; **Bytes** shows its bytes.
3. Select the function to replace, open **Pseudocode**, then **Decompile**.
4. Choose **Edit recovered C**. The recovered text opens as a separate text
   document. Existing text stays in its tab with its unsaved edits.
5. Edit the function. Preserve its name and review the original calling
   convention, parameter types, return type and effects. The compiler accepts
   basic Ghidra scalar type aliases, but does not recover or verify the ABI.
   Use Save As to retain the recovered/edited C separately.
6. For a replacement that needs more space or supported calls and data, switch
   **Binary → Relocated Replacement** on. The status message reports the mode.
   Leave it off for the fixed-span path, then choose **Binary → Compile Recovered C / Cancel**.
   The result is a preview, not an immediate binary mutation. The same menu item
   cancels an active compile. Diagnostics explain refused inputs.
7. Review the instructions and **Apply replacement**. In relocated mode, review the
   entry, body, added sections, relocations and warnings; **View body** shows
   the replacement instructions. The edit is applied in memory;
   binary undo/redo remains available. Changing source invalidates its preview;
   compile again after an edit. After a binary patch, undo or redo, decompile
   the current binary and choose **Edit recovered C** again before compiling.
8. **Binary → Export Copy** to a new destination. A compiled native replacement exports
   with owner-only read/write/execute permissions (0700). The editor does not
   run the result. Existing destinations and the original binary are not replaced.
9. Reopen the exported binary to inspect the changed code. For a relocated
   replacement, the entry stub points to the new body; use **Go to replacement
   body** and **Decompile**, or select the `__ale_body_` function. After a
   patch, recover from the new binary snapshot before another replacement.
   Source and binary documents retain their own save state.

## Placement and supported inputs

Both modes require an exact, nonzero, nonoverlapping ELF text-symbol span of at
most 64 KiB and an unambiguous contiguous mapping. Stripped or inferred bounds
are not accepted. Preprocessor directives, pragmas and inline assembly are not
accepted in this C path.

With relocated mode off, replacement code must fit the original span; shorter
code is padded with NOPs. Allocated extra sections, relocations, external or
indirect calls, indirect branches, RIP-relative data references and branches
outside the replacement are rejected. The fixed-span path can inspect supported
shared objects as well as executables.

Relocated mode supports qualified Linux x86-64 ELF executables and dynamically
linked PIE executables. The original function must have at least five bytes for
an entry jump. The editor keeps that entry address and adds a read-execute body
and a separate read-only region for program headers and constant pools. It can
resolve supported PC-relative calls and data references to unambiguous defined
symbols in the same image, including eligible existing file-backed mutable
objects. A replacement may contain read-only constant pools. Supported compiler
object relocation forms are `R_X86_64_PC32`, `R_X86_64_PLT32`, and no-op
`R_X86_64_NONE`, subject to symbol, instruction and range checks. It does not
construct a dynamic import or a new PLT or GOT entry.

Relocated mode refuses shared objects, static PIE, unresolved or ambiguous
symbols, new writable globals or BSS, TLS, IFUNCs, unsupported relocation forms,
indirect calls or jumps, insufficient entry space, unsafe known interior
references, and images with unsupported unwind or CET requirements. It also
rejects a second relocated replacement in the same image. If a diagnostic
refuses an input, preserve the original and use a supported fixed-span edit or
rebuild the program from source; do not assume a refused image was rewritten.
Malformed or unrecognized rewrite metadata disables trusted body navigation and
source mappings. Replaced bytes do not retain reliable original source mappings.

Relocated-mode limits include 64 MiB input and output images, 64 KiB C source,
2 MiB compiler objects, 1 MiB each of replacement text and read-only data, 128
object sections, 4,096 object symbols, 16,384 relocations, 256 input program
headers and 16,384 section headers. Individual checks can reject an input before
it reaches a size limit.

Compilation checks syntax and placement. It does not prove behavior, inferred
types, calling-convention compatibility, or the absence of unknown computed
interior entry points. Unwind and exception behavior through replacement code is
not established. Signatures and checksums are not repaired. Kernel, firmware
and other ABIs are outside these profiles. Review the preview and test an
exported copy in its intended environment.

The compiler and decompiler are trusted host tools with resource limits, not
security sandboxes. No inspected target is executed automatically.
