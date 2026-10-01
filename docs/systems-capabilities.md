# Systems workstation capability contract

The product goal is a native editor with the systems-analysis abilities of
Ghidra and a more intuitive interface. A binary inspector, hexadecimal view,
or disassembler alone does **not** complete that goal. This document preserves
the broader obligations while implementation proceeds in usable increments.
It is a requirements and backend-research document, not a claim that the
listed capabilities have been implemented or accepted.

The existing editor, safe text persistence, and GPUI-free editing core remain
product invariants. Analysis must be local, optional to ordinary editing, and
unable to stall input. Rust and native GPUI remain the application and interface;
Java, a webview, an account, or a cloud service must not become required. Imported
programs are data: opening or analyzing them must not execute them.

## Interaction contract

The common object is a program location, not a row number in a particular view.
The long-term location model must identify the artifact and revision, address
space, address, and relevant decode context. File offsets, virtual addresses,
registers, relocated runtime addresses, and source positions need explicit
mappings. A mapping can be absent or ambiguous; the interface must show that
rather than invent an address. Raw images require an explicit processor,
endianness, and load address before their bytes can be treated as instructions.

The following are product acceptance requirements, not existing controls:

- **Representation:** move among original source where available, recovered
  pseudocode, intermediate operations, instructions, bytes, and typed data while
  retaining the selected location. Pseudocode must be identified as recovered
  output and must not imply that original source can be reconstructed exactly.
- **Scale:** move from workspace/module to region, function, basic block,
  instruction, byte, and bit without losing the enclosing context. Back and
  forward restore both location and representation; a zoom operation must not
  silently modify the program.
- **Structure:** switch between discovered structure, raw content, and unknown
  regions. Analysis annotations carry origin and confidence, can be corrected,
  and do not overwrite the underlying evidence. An entropy display is a byte
  distribution statistic, not proof of compression, encryption, or correctness.
- **Editing:** text selection, undo, dirty state, and save protection survive
  entering and leaving analysis. Analysis edits and binary patches have their
  own reversible transactions. A data view must never send arbitrary bytes
  through the strict UTF-8 text save path.
- **Customization:** users can hide, resize, arrange, and restore panels; change
  key bindings and display fields; save local layouts; and recover defaults.
  Ordinary editing starts with little chrome. Contextual actions and a searchable
  command surface reveal complexity when needed.

These requirements take inspiration from Ghidra's connected program views,
navigation history, configurable fields, and tool layouts, but acceptance is
defined for this application. [Official introductory guide][intro]

## Capability and acceptance map

Every row is required by the systems-work goal unless explicitly superseded by
the user. A row is complete only after its exercised scope and limits are
recorded. Tests for one architecture or format cannot establish universal
coverage. The current implementation evidence belongs in milestone validation
documents; this table must not turn into a list of implied working features.

| Capability family | Required result | Evidence needed before claiming completion |
| --- | --- | --- |
| Import, loaders, and memory model | Executables, object files, raw firmware, libraries, debug information, rebasing, relocations, mapped/unmapped regions, and processor/compiler selection. Maintain separate file and memory layouts. | A versioned format/architecture matrix; independently generated fixtures with holes, overlays, nonzero bases, malformed tables, stripped metadata, and unsupported variants; address mapping checked against independent tools. |
| Containers and artifacts | Inspect archive/filesystem/firmware contents, navigate nested artifacts, import selected members, and export them safely. | Nested fixtures with resource bounds and path traversal cases; exact extraction bytes; cancellation leaves the editor and original artifacts intact. Ghidra exposes this separately from program loading. [Filesystem browser][filesystem] |
| Disassembly and processor semantics | Broad processor coverage, mixed modes, context-sensitive decoding, instruction operands, flow, and explicit code/data/unknown boundaries; extensible processor descriptions. | Pin processor definitions; exercise each supported mode and endianness against a separate decoder plus architecture fixtures, including invalid/truncated instructions and delay slots where applicable. SLEIGH represents assembly and instruction semantics as p-code. [Language specifications][languages] |
| Analysis and references | Discover and edit functions, branches, calls, data references, imports/exports, symbols, strings, namespaces, and library identities; rerun or cancel selected analyzers with visible provenance. | Known-answer binaries, indirect-flow and embedded-data cases, manual corrections that survive reanalysis, and partial/failure states. A linear decode is not control-flow recovery. [Introductory analysis workflow][intro] |
| Decompilation and types | Readable recovered control flow; calling conventions, parameters, locals, structures, unions, enums, pointers, arrays, and function signatures; edits to names/types propagate consistently. | Controlled optimized/unoptimized fixtures, semantic witnesses for selected functions, consistent address/token mapping, user corrections and invalidation, and explicit unsupported operations. C headers and reusable type libraries are part of the type workflow. [Intermediate guide][intermediate], [C header parser][cparser] |
| Graphs and navigation | Function control-flow, call, and reference graphs connected to listing and pseudocode; scoped search of symbols, strings, values, bytes, and instruction patterns. | Known graph edges, unresolved targets represented honestly, cycles and large graphs, keyboard navigation, reversible filtering, and location preservation across views. [Introductory guide][intro] |
| Assembly and binary patching | Assemble instructions at a selected location, preview changed bytes and affected analysis, patch bytes/data, undo, compare, and export a defined snapshot safely. | Assemble/decode round trips, variable-length and out-of-range edits, relocation/checksum effects, external conflicts, atomic export failures, and exact bytes after reopen. Instruction patching is a distinct workflow in Ghidra. [Advanced exercises][advanced] |
| Analysis persistence | Save/reopen programs, annotations, functions, types, bookmarks, references, layouts, and provenance; migrations and recovery; preserve original artifact bytes unless explicitly exporting patches. | Restart round trips, interrupted writes, schema migration tests, concurrent/stale result handling, and reviewable undo/redo for analysis changes. This is separate from the existing text-document save implementation. |
| Debugging and runtime state | Local/native and supported remote backends; launch/attach, breakpoints, stepping, threads, stacks, registers, memory, expressions, modules, static/runtime mappings, trace snapshots, and detach. | Controlled target programs; deterministic stop events; failure/reconnect/exit handling; explicit target identity; no execution before user intent; platform-specific acceptance. Ghidra links dynamic listing, memory, registers, and watches. [Machine state][machine] |
| Emulation and semantic inspection | Step instructions and intermediate operations, inspect/override state, model external calls/devices, and explore snapshots without confusing emulation with the live target. | Register/memory transitions against an independent emulator or hardware for each supported semantics subset; unsupported user operations halt clearly; bounded runs and deterministic replay. Ghidra documents limitations of using decompiler-oriented p-code for emulation. [Emulation][emulation], [Modeling][modeling] |
| Comparison and version tracking | Compare programs and functions; review matches, differences, and correspondence; selectively transfer analysis between versions. | Known changed/unchanged functions, false and ambiguous matches, confidence visibility, rejectable transfers, undo, and persistence. [Version tracking course][versions] |
| Library identification and similarity | Recognize known library functions and search a local corpus for structurally similar functions, including imperfect matches. | Versioned signature corpus, held-out positives/negatives, collision handling, similarity versus confidence, and user acceptance of applied metadata. Ghidra Function ID and BSim solve different matching problems. [Function ID][fid], [BSim][bsim] |
| Automation and extensibility | Local scripts, headless/batch analysis, configurable analyzers, processor/loader extensions, stable programmatic queries, progress/cancellation, and reproducible exports. | A real batch script over fixtures, repeatable results, versioned interfaces, bounded failures, and editing usable after a worker crash. Capability equivalence does not require executing Java plugins. [Scripting course][scripting] |
| Native editor and workspace integration | Preserve existing editing; connect source, build artifacts, filesystem navigation/search, structural source views, Git, terminal, and language/debug services without forcing analysis panels into everyday work. | Original editor/persistence regression suite plus end-to-end source-to-artifact navigation and failure isolation. The existing [architecture](architecture.md) describes these long-term subsystems, not completed integration. |

Ghidra also has multi-user project/server features. Existing product constraints
exclude required accounts, collaboration UI, and cloud infrastructure. Local
analysis history, version tracking, import/export, and reproducibility remain
required; a Ghidra collaboration server is not an implicit product dependency.
This is a recorded product constraint, not a claim of literal compatibility with
every Ghidra plugin, project database, or deployment mode. [Official guide][intro]

## Native backend feasibility

**Supported by upstream source, not integration-verified here:** Ghidra's
decompiler and SLEIGH include C++ implementations. The native Makefile declares
`libsla.a`, `libdecomp.a`, `sleigh_opt`, and standalone `decomp_opt` targets. Its
SLEIGH example demonstrates native disassembly, p-code translation, and a small
emulated program. A Rust application can therefore investigate a native adapter
without embedding the Java application. This is a feasibility inference from
source, not evidence that the native targets build on this host or that their
integration is complete. [Native Makefile][makefile], [SLEIGH example][example]

There are two materially different native integration routes:

1. Wrap a pinned C++ library build behind a small native interface, supplying
   memory, context, language/compiler specifications, symbols, types, and errors.
   A worker process offers a place to isolate crashes and enforce cancellation
   and resource budgets. A subprocess alone is not a security sandbox.
2. Reimplement the client side of the native Ghidra decompiler protocol. Its
   architecture interface queries a client for bytes, p-code, types, symbols,
   comments, registers, and related context. Launching the native `decompile`
   executable by itself does not replace that client or the program database.
   [Native client interface][protocol]

The library route is the leading experiment because it can keep processor
translation and decompilation together under one explicit program model.
The protocol route remains a comparator, not a second backend to scaffold now.
Before selection, build a pinned native target without a JVM, feed it controlled
function bytes and specifications, obtain actual pseudocode with address mapping,
change a type/name and observe the updated result, then exercise cancellation,
malformed input, worker death, and restart. Record toolchain, source revision,
licenses/notices for included components, packaged processor coverage, and
runtime dependencies. The upstream Makefile has host-specific assumptions and
optional native libraries, so source availability does not prove portable builds.

**Not inherited from native decompiler reuse:** Ghidra's broad Java analyzers,
loaders, project database, UI, debugger integrations, and extension API still
need native equivalents or explicit adapters. Its public assembler entry point
uses the Java `SleighAssemblerBuilder`; using the C++ decoder does not supply
assembly patching. [Assembler source][assembler] Full Ghidra and PyGhidra are
not acceptable hidden runtime substitutes for this product's no-Java constraint.
[Official build and launch requirements][ghidra]

Native Rust decoding can deliver useful exploration before this experiment is
finished. It must retain explicit processor/mode identity and bounded errors so
later SLEIGH semantics, decompilation, and patch assembly can join the same
location model. Displaying text produced by a decoder must not be mislabeled as
analysis, source recovery, or proven execution semantics.

## Verification and remaining work

This research checked official Ghidra documentation and upstream source on
2026-09-27. Upstream links below use moving documentation/master revisions;
implementation must pin the backend and processor files it actually ships.
No Ghidra/native backend was installed, built, linked, or executed for this
research. There is no parity claim and no performance claim.

The remaining goal includes every unaccepted capability above and the interaction
contract across them. The next dominant technical uncertainty is native
decompiler/semantic integration, while shared location/navigation and resilient
binary loading provide the first user-visible foundation. Acceptance must include
real GUI workflows, temporary-file persistence tests, independent binary-tool
comparisons, and continued editor regressions. Unit tests and `cargo check` alone
cannot prove a usable native workstation or broad Ghidra capability coverage.

[intro]: https://ghidra.re/ghidra_docs/GhidraClass/Beginner/Introduction_to_Ghidra_Student_Guide.html
[intermediate]: https://ghidra.re/ghidra_docs/GhidraClass/Intermediate/Intermediate_Ghidra_Student_Guide.html
[languages]: https://ghidra.re/ghidra_docs/languages/index.html
[filesystem]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Base/src/main/help/help/topics/FileSystemBrowserPlugin/FileSystemBrowserPlugin.html
[cparser]: https://ghidra.re/ghidra_docs/api/ghidra/app/util/cparser/C/CParserUtils.html
[advanced]: https://ghidra.re/ghidra_docs/GhidraClass/Advanced/improvingDisassemblyAndDecompilation.pdf
[machine]: https://ghidra.re/ghidra_docs/GhidraClass/Debugger/A4-MachineState.html
[emulation]: https://ghidra.re/ghidra_docs/GhidraClass/Debugger/B2-Emulation.html
[modeling]: https://ghidra.re/ghidra_docs/GhidraClass/Debugger/B4-Modeling.html
[versions]: https://github.com/NationalSecurityAgency/ghidra/blob/master/GhidraDocs/GhidraClass/Intermediate/VersionTracking.html
[fid]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/FunctionID/src/main/help/help/topics/FunctionID/FunctionID.html
[bsim]: https://ghidra.re/ghidra_docs/GhidraClass/BSim/BSimTutorial_Intro.html
[scripting]: https://ghidra.re/ghidra_docs/GhidraClass/Intermediate/Scripting.html
[makefile]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Decompiler/src/decompile/cpp/Makefile
[example]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Decompiler/src/decompile/cpp/sleighexample.cc
[protocol]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Decompiler/src/decompile/cpp/ghidra_arch.hh
[assembler]: https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Framework/SoftwareModeling/src/main/java/ghidra/app/plugin/assembler/Assemblers.java
[ghidra]: https://github.com/NationalSecurityAgency/ghidra

## Native feasibility experiment — 2026-09-27

A subsequent bounded experiment **did build and run** the native decompiler,
advancing the earlier source-only feasibility finding. This is external backend
experiment evidence; it is not an integrated IDE feature.

- Pinned official Ghidra revision:
  `c4273522017788fb67c30058ffd5bbdf291fcc40`.
- Downloaded only the native C++ subtree and x86 language definitions (319 files)
  to `/tmp/ale-ghidra-native-lhtf818e`; no full clone, installation, or Java build.
- Existing host dependencies: GCC/G++ 14.2.0, make, zlib 1.3.1. BFD headers were
  absent. The temporary Makefile excluded `bfd_arch`, `loadimage_bfd`,
  `analyzesigs`, and `codedata` from `EXTRA` and linked with `BFDLIB=`. The first
  build exposed the additional `analyzesigs` dependency on missing `bfd.h`;
  it was retained in the experiment log. No production source was modified.
- `make -j3 decomp_opt sleigh_opt OPT_CXXFLAGS='-O0' BFDLIB=` completed with exit
  status 0. Generated parser timestamps were refreshed to use the pinned,
  already-generated sources; `sla_opt/` was created for the combined targets.
- `sleigh_opt x86-64.slaspec x86-64.sla` completed with status 0. Upstream warnings
  remained: NOP constructors, two zero-size exports, unused temporaries,
  unnecessary extensions/truncations, and an unreferenced table. These were not
  suppressed or established harmless across the processor definition.
- A controlled source fixture, `unsigned int add_one(unsigned int value) { return
  value + 1u; }`, compiled with `cc -O1 -fno-asynchronous-unwind-tables
  -fno-stack-protector -c`. `objcopy` extracted `.text`; independent GNU `objdump`
  identified `8d4701` as LEA and `c3` as RET. The exact four bytes were imported
  through a `binaryimage` XML fixture at address `0x1000` using
  `x86:LE:64:default:gcc`.
- Native console commands `load file`, `load function add_one`, `decompile`,
  `print C`, `print C xml`, `print raw`, and `quit` completed with status 0.
  Actual recovered output was `int4 add_one(int4 param_1)` with body
  `return param_1 + 1;`. The signed type is recovered output, not proof of the
  original unsigned signature or full semantic equivalence.
- An independent Python XML/log check joined pseudocode `opref` identifiers to
  raw p-code sequence numbers: the `+` token mapped to LEA at `0x1000`, and the
  `return` token to RET at `0x1003`. Both assertions passed. Syntax and declaration
  tokens need not have operation references; this does not prove universal,
  one-to-one source mapping.
- Both processor compilation and decompilation ran with `PATH=/nonexistent` and
  `JAVA_HOME` unset. `ldd decomp_opt` listed only the native C/C++ runtime, zlib,
  math library, and loader. No JVM was used by these direct native commands.

Artifacts retained in that temporary directory include `build.log`, `sleigh.log`,
`add_one.c`, `add_one.bin`, `independent-disassembly.txt`, `image.xml`,
`commands.txt`, `decompile.log`, and `verification.txt`. The fixture SHA-256 is
`988f6eff6d1f65732216c81eb703dbba35796e62b33cbc948c8402f57dff8a1a`.
Temporary artifacts are not durable repository fixtures.

Residual: two optional signature-injection attempts through `parse line` failed
(`unsigned int`: syntax error; `uint4`: "Not sure what to do with this type").
Those logs remain as `typed-decompile.log` and `typed-decompile-native-types.log`;
no type-editing claim is made. Other processors, executable loaders, malformed
input isolation, cancellation, worker restart, licensing/package audit, persisted
analysis, GUI integration, and actual type/name editing remain unverified. This
experiment proves native pseudocode and two instruction mappings for one tiny
fixture; broad decompiler parity remains open.

### Reproduce the native experiment

[`scripts/verify-native-decompiler.py`](../scripts/verify-native-decompiler.py)
preserves the successful fixture as an opt-in experiment. It is not part of
application startup or required editor validation, and does not integrate the
decompiler into the IDE.

```sh
python3 scripts/verify-native-decompiler.py --help
python3 scripts/verify-native-decompiler.py --work-dir /tmp/ale-native-check --jobs 3
```

Choose an empty directory outside the repository, or reuse an existing experiment
directory with the exact revision marker. Python 3.9+, Linux x86-64, C/C++
compilers, make, binutils, and zlib development files must already be available.
The script never installs packages, invokes sudo, or changes global settings.
Fresh runs fetch only the pinned C++/x86 sources and upstream license; source
files are checked against a pinned manifest digest and Git blob hashes. Download
attempts, response sizes, retries, and subprocess runtimes are bounded. A build
timeout stops its entire process group. `--build-timeout` defaults to 900 seconds.

Cached native binaries and the compiled processor specification are reused.
Their reuse is recorded with the native binary hash; it is not an attestation
that arbitrary preexisting binaries were reproducibly built from those sources.
The script recompiles only the tiny controlled fixture on each verification,
checks GNU disassembly, obtains actual pseudocode, and verifies both instruction
mappings. It emits logs, `UPSTREAM-LICENSE`, and
`native-experiment-result.json` inside the selected directory. Generated sources,
processor specifications, and binaries must not be committed to this repository.

Script validation on 2026-09-27: `--help` passed; running with
`--work-dir /tmp/ale-ghidra-native-lhtf818e` passed both operation mappings while
reusing the prior native build. Syntax and whitespace checks passed. Controlled
failure checks confirmed preservation of an unrelated nonempty directory,
clear missing-compiler failure, rejection of an altered source manifest,
subprocess timeout cleanup, and exactly two download attempts on repeated
timeout. A second full native rebuild was intentionally not performed; the
fresh-build route combines the earlier successful native build procedure with
the checked download/build orchestration in this script.

### Installed backend inventory — 2026-09-27

The host already contains Rizin 0.8.2, Cutter package 2.3.4, Ghidra package
10.3.2, and Ghidra data package 10.4. Presence is not equivalent to usable
integration:

- `rizin -N -q -e scr.color=0 -e cfg.debug=false -e io.exec=false -c Lcj --`
  listed only the `java` and `dex` core plugins. `pdg?` failed, and an actual
  `aa;aflj;pdgj` probe reported that `pdgj` does not exist. Rizin nevertheless
  exited with status 0, so adapters must validate command errors and response
  shape in addition to process status.
- Static `aa` analysis of the controlled acceptance fixture discovered ten
  functions, including `sym.add_one` at `0x401106`. No debugger, emulation,
  target execution, or write mode was used; the fixture SHA-256 was unchanged.
  Probe outputs are retained as `rizin-plugins-probe.json` and
  `rizin-analysis-probe.json` in the native experiment directory.
- Cutter's ELF dependencies require `librz_*.so.0.7`; `ldd /usr/bin/cutter`
  reported those libraries missing in the current loader environment, which
  has Rizin 0.8 libraries. Its package inventory contained no bundled
  rz-ghidra shared plugin. Cutter was not launched or repaired.
- Installed Ghidra includes native `decompile` and `sleigh` executables under
  `/usr/share/ghidra/Ghidra/Features/Decompiler/os/linux_x86_64/`. Its decompiler
  still needs a compatible program-model/protocol client; it is not a standalone
  replacement for the proven XML-loader experiment.

The immediate reuse candidate is a bounded Rizin worker for already exercised
static analysis, alongside the proven native decompiler experiment. An
rz-ghidra worker could reduce integration work if a matching native plugin is
made available and verified. Official rz-ghidra uses native C++, provides
`pdgj` JSON, requires Rizin-version compatibility, and documents LGPLv3 licensing;
none of that proves the plugin exists on this host. [Official rz-ghidra source](https://github.com/rizinorg/rz-ghidra)
`-N` disables user scripts/settings but permits plugins; `-NN` disables scripts
and dynamic plugins. Capability detection must distinguish these modes.
