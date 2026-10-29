# Edit recovered C and rebuild a function

This first native round-trip path replaces a named leaf function in a Linux
x86-64 ELF image. It uses the pinned native decompiler, the normal text editor,
the host C compiler, a byte preview, and create-only binary export. It is an
initial part of a broader systems workbench, not a general binary relinker.

## Prepare

Build and configure the [native decompiler](../tools/native-decompiler/README.md).
The compiler path is `/usr/bin/cc`. No compiler packages are installed by the
editor. This path currently requires Linux and x86-64 ELF output.

## Round trip

1. **Inspect** an ELF executable or shared object. Select the function in the
   function list and open **Pseudocode**, then **Decompile**.
2. Choose **Edit recovered C**. The recovered text opens as a separate text
   document. Existing unsaved text gets the normal Save/Discard/Cancel prompt.
3. Edit the function. Preserve its name and review the original calling
   convention, parameter types, return type and effects. The compiler accepts
   basic Ghidra scalar type aliases, but does not recover or verify the ABI.
   Use Save As to retain the recovered/edited C separately.
4. Click **Compile C**. The result appears as an assembly/byte preview, not an
   immediate binary mutation. Compiler diagnostics explain failed builds.
   **Cancel compile** stops an active compilation.
5. Review the instructions and **Apply patch**. The edit is applied in memory;
   binary undo/redo remains available. Changing source invalidates its preview.
6. **Export copy** to a new destination. A compiled native replacement exports
   with owner-only read/write/execute permissions (0700). The editor does not
   run the result. Existing destinations and the original binary are not replaced.
7. Reopen and decompile the exported/current binary to inspect the changed code.
   After applying a patch, recover from the new binary snapshot before starting
   another replacement. Source and binary documents retain their own save state.

## Current placement limits

The original function must have an exact, nonzero ELF text-symbol size, at most
64 KiB, without overlapping function symbols. Inferred/stripped function bounds
are not accepted for replacement. The mapping must be contiguous and unambiguous.

The compiler produces a standalone ELF relocatable function. Replacement code
must fit the original symbol span; shorter code is padded with x86 NOPs. Extra
allocated sections, relocations, external/indirect calls, indirect branches,
RIP-relative data references and branches outside the replacement are rejected.
This excludes many calls, globals, string/constant pools, and larger functions.
Preprocessor directives, pragmas and inline assembly are not accepted in this
C path; assembly/bytes have their own editing tools.

Compilation checks syntax and placement. It does not prove behavior, correct
inferred types, interior entry-point safety, or compatibility with existing
callers. Debug mappings, unwind/exception data, signatures and checksums are not
regenerated. Kernel, firmware and other ABIs are outside this initial placement
profile. Review and test the result in its intended environment.

The compiler and decompiler are trusted host tools with resource limits, not
security sandboxes. No inspected target is executed automatically.
