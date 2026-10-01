# Linked systems views

The first systems-workstation slice extends the existing Rust/GPUI editor. The
original text document and one binary snapshot coexist. Nothing imported is
executed, and the original binary is never a save destination.

## Controls

| Action | Control |
| --- | --- |
| Open UTF-8 text | Ctrl+O or Open |
| Inspect binary | Ctrl+Shift+O or Inspect |
| Return to retained text | Ctrl+1 or the text file tab |
| Binary overview / assembly / bytes | Ctrl+2 / Ctrl+3 / Ctrl+4 |
| Show/hide shell panels | Ctrl+B or Panels |
| Save text / export binary copy | Ctrl+S, according to the active document |
| Save text as / export binary copy | Ctrl+Shift+S |
| Source for selected machine address | Source in systems toolbar |
| Go to address | Go to, enter hex virtual address or `@hex` file offset, Run/Enter |
| Search | Find, symbol/string text, Run/Enter |
| Preview instruction edit | Select instruction, Assemble, Intel instruction, Run/Enter |
| Preview byte edit | Select byte, Patch bytes, two-digit hex bytes, Run/Enter |
| Apply / reverse patches | Apply patch / Undo / Redo |
| Export | Export copy; choose a new path |

On macOS the application shortcuts use Command. Native acceptance has only been
performed on Linux; other platform behavior must be separately validated.

One file offset anchors Overview, Assembly, Flow, Bytes and Strings. Symbols, sections,
branches and entropy bins navigate that location; Back restores earlier offsets.
Assembly is a bounded linear decode, not recovered control flow. Next page moves
through the file. Details reveals symbol visibility, A-/A+ binary font size, and
8/16-byte row controls. These view choices currently last for the open binary,
not across application restarts. Layout docking and keymap customization remain
open requirements.

## Presentation

The native shell uses compact document tabs, subdued chrome and a shared accent
for active locations. Inter is preferred for UI text (system fallback otherwise);
monospace code uses the existing platform font. Symbols and instructions are flat
lists; addresses, encodings and mnemonics occupy aligned columns. Bytes use a
fixed-cell grid with an eight-byte group gap and a separate ASCII column.

Go to, Find, Assemble and Patch bytes share a compact native command input.
Previews remain inline and require Apply patch. Placeholder shell panels are
hidden initially and available through Panels. Details holds secondary controls
and complete messages. No layout/keymap persistence is implied by this restyling.

## Source correspondence

For supported DWARF line information, Source opens the referenced UTF-8 file at
its build-time line. A dirty text document receives the existing Save/Discard/
Cancel decision first. When that source is already open, navigation preserves its
edits and undo history. Returning from a mapped source line to a binary view
retains the exact selected instruction or interior byte if it still belongs to
that line, otherwise selects its first mapped instruction. No mapping means the
previous binary location is retained with an explanation.

Line maps describe the compiled artifact. Editing the source does not compile it,
and neither source timestamps nor content are guaranteed to match the build.
Optimized code can map one line to several addresses. Stripped binaries do not
provide original source. PDB, split/compressed DWARF, relocatable-object DWARF,
inline-call reconstruction, and ambiguous line intervals are not supported yet.
An optional native Pseudocode view is available for the restricted ELF/x86-64 contract described below.

## Formats, bounds and patches

The UI accepts regular files up to 64 MiB. Recognized ELF, PE and Mach-O headers
are parsed; malformed recognized images report errors. Other bytes remain raw.
Sections and symbols carry distinct virtual addresses and file offsets. BSS,
overlapping mappings, and unmapped bytes have no invented virtual address.
Segment-only images are not mapped yet. Raw bytes require the explicit
Interpret x86-64 action (base zero); broader raw processor/base controls remain
outstanding. Other recognized architectures keep bytes/metadata available but
report unavailable disassembly.

Assembly uses `iced-x86`; file parsing uses `object`; DWARF uses `gimli`. The
display decodes at most 512 bytes/64 instructions and paints at most 64 byte rows.
Metadata and DWARF extraction have separate resource limits and visible warnings.
Search returns at most 128 matches, with the first 100,000 bounded printable ASCII
strings scanned. Overview entropy is Shannon byte entropy, not proof that data
is packed, encrypted or meaningful. No performance claim is made.

NASM assembly supports a conservative list of common x86 instructions. It accepts
one instruction, disables preprocessing, rejects directives/labels/macros, limits
source/output/diagnostics, and terminates after two seconds. No shell is invoked.
Instruction edits must keep the selected instruction's byte length; use byte
patching for other fixed-length edits. Preview does not mutate anything. Apply
reparses the candidate; failed candidates preserve the previous snapshot. Patches
never insert/delete bytes or update relocations, signatures, checksums or code
references automatically. They can therefore make an executable invalid.

Patch undo holds up to 32 snapshots and 128 MiB of prior byte content. Analysis
metadata and outstanding workers can add memory beyond that byte-history limit.
An unchanged patch is a no-op. Dirty state follows revisions, including undo back
to an exported revision. Export captures one immutable snapshot; later patches
remain dirty. Close/quit or replacement prompts for unexported patches even when
the text view is active. Export cancellation/failure retains both documents.

## Filesystem guarantees and limits

Export uses a same-directory temporary file, flush, file sync, create-only commit,
and on Unix directory sync. Existing destinations are refused, including a file
created after the initial check. The original path is never truncated or replaced.
Copies have private temporary-file permissions, not executable permission copied
from the source. Symlinks and symlink ancestors are rejected. A directory-sync
failure reports that the output exists but durability is uncertain; patches remain
dirty so the user can recover deliberately.

Reads compare metadata before and after bounded I/O. These checks are not a
transaction against concurrent writers and do not prevent all ancestor-directory
replacement races. There is no power-loss, network-filesystem, or adversarial
parser sandbox guarantee. Parsing, search, entropy, patch reparsing, file I/O and
assembly run off the UI thread; bounded decoding and navigation run in the view.
General worker cancellation, worker-process isolation and analysis persistence
remain obligations in the [full capability contract](systems-capabilities.md).

## Function analysis and Flow

The Flow tab exposes bounded recursive x86/x86-64 control-flow analysis on declared
executable file-backed regions. Function candidates come from entry points,
executable symbols and direct calls. The navigator switches between functions and
symbols; graph instructions, edge destinations, and the References list navigate
the same location used by Assembly, Bytes and Source. References include direct
branches/calls and explicitly unresolved indirect transfers.

Analysis runs on an immutable snapshot in the background. Apply, undo and redo
invalidate and rebuild it; cancellation and generation/revision checks prevent a
late result from replacing newer state. Raw bytes, ambiguous mappings and other
architectures report unsupported analysis rather than guessing executable code.
Defaults bound work to 128 functions, 4,096 blocks, 100,000 instruction rows and
8 MiB of decoded bytes. Shared tails count against each function's row budget.
The graph displays at most 64 blocks with six instruction lines per card; Listing
opens the full linear view. The reference list displays at most 256 matches.

This is candidate recovery, not proof of code/data separation or full program
semantics. Indirect target recovery, exception flow, data references and user
function definitions remain open. Graph routes can overlap and function membership
can be ambiguous for shared tails. The optional native decompiler now connects to the Pseudocode tab; its broader
processor and program-model coverage remains unfinished.

## Optional pseudocode

Set `ALE_DECOMPILER` to an absolute path to the pinned native worker before launch.
Choose a function, open Pseudocode and press Decompile. Click a mapped token to
select its machine location, then switch to Assembly or Bytes; Copy code copies
the full recovered result. Cancel retains the document. New bytes or a different
function clear the old pseudocode. Missing configuration and unsupported inputs
show recoverable explanations. Names and types are inferred, not original source.

See [worker setup](../tools/native-decompiler/README.md) and
[validation and limits](decompiler-validation.md). Linux ELF64/x86-64 is the initial
contract; this does not establish broad decompiler parity or native visual acceptance.
