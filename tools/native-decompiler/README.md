# Optional native decompiler

The IDE can call a separately built, pinned Ghidra C++ worker. This route uses no
Java, shell command evaluation, target execution, account or network service.
The worker is trusted executable code selected by the user; process limits are
not a security sandbox. The default editor works without it.

Build outside the repository using the existing source-verified experiment:

```sh
python3 tools/native-decompiler/build.py --work-dir /tmp/ale-native-cache
python3 tools/native-decompiler/test_worker.py --worker /tmp/ale-native-cache/ale-native-decompiler
ALE_DECOMPILER=/tmp/ale-native-cache/ale-native-decompiler cargo run --locked -p a-less-awful-editor
```

The build downloads the pinned native sources when absent. It does not install
packages or use sudo. Existing compiler/make/binutils/zlib requirements and source
verification are described in `docs/systems-capabilities.md`. Do not commit the
external cache, generated processor specifications or native binaries. The pinned
upstream revision is `c4273522017788fb67c30058ffd5bbdf291fcc40` (Apache-2.0).

Open a binary with Inspect, choose a recovered function, select Pseudocode, then
Decompile. Mapped tokens select the same file location as Assembly and Bytes.
Copy code copies the complete result. Pseudocode is recovered, with inferred
names/types; it is not the original source. Worker diagnostics remain visible.

## Current contract

- Linux process integration; ELF64 little-endian x86-64 System V/Linux executable
  or shared object. Headers cannot establish function-specific ABI overrides.
- At most 256 unambiguous file-backed regions and 16 MiB mapped bytes. Read-only
  comes from ELF flags. BSS is not invented, relocations/import resolution are not
  applied, and other formats/architectures report unsupported input.
- Each request captures immutable bytes and function candidates. The worker runs
  in a private temporary directory with a minimal environment and its own process
  group. Nonblocking pipes bound response and diagnostics. CPU, address-space,
  file-size and descriptor limits supplement a 20-second wall deadline.
- Cancel, document drop, patch/history changes and function changes invalidate the
  generation; late output cannot replace newer state. Group termination and direct
  child reaping happen on return. A deliberately malicious configured worker can
  escape its group; this is not containment against untrusted executable plugins.
- Response protocol/backend revision, entry, UTF-8 ranges, token text and machine
  instruction addresses are validated before presentation. Unmapped syntax tokens
  remain text; claimed addresses must belong to recovered instructions.
- Pseudocode is limited to 1 MiB / 100,000 tokens; native output to 16 MiB and
  stderr to 64 KiB. Display pages contain 120 lines. Extremely long individual
  lines and dense token layouts still need native responsiveness acceptance.

Run actual native integration tests explicitly:

```sh
ALE_DECOMPILER_TEST=/tmp/ale-native-cache/ale-native-decompiler cargo test --locked --workspace
```

Without that variable, the two actual-native acceptance tests report a skip;
protocol, process-failure, memory-mapping and existing editor tests still run.
The backend cache must already exist. These tests never execute inspected code.
