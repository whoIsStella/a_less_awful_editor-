# Optional native decompiler

The IDE can call a separately built, pinned Ghidra C++ worker. This route uses no
Java, shell command evaluation, target execution, account or network service.
The worker is trusted executable code selected by the user; process limits are
not a security sandbox. The default editor works without it.

Install Python 3, a C/C++ compiler, make, binutils, and zlib development files.
On Debian/Ubuntu-derived Linux systems:

```sh
sudo apt-get install build-essential binutils zlib1g-dev python3
```

Run these commands from the project folder to build the worker in a separate cache:

```sh
python3 tools/native-decompiler/build.py --work-dir /tmp/ale-native-cache
python3 tools/native-decompiler/test_worker.py --worker /tmp/ale-native-cache/ale-native-decompiler
ALE_DECOMPILER=/tmp/ale-native-cache/ale-native-decompiler cargo run --locked -p a-less-awful-editor
```

The build downloads the pinned native sources when absent. It does not install
packages or use sudo. Keep the generated cache available at the configured path.
The build verifies the downloaded sources before compilation. The pinned
upstream revision is `c4273522017788fb67c30058ffd5bbdf291fcc40` (Apache-2.0).

Open a binary with Inspect, choose a recovered function, select Pseudocode, then
Decompile. Mapped tokens select the same file location as Assembly and Bytes.
Copy code copies the complete result. Pseudocode is recovered, with inferred
names/types; it is not the original source. Worker diagnostics remain visible.

## Supported inputs and limits

- Linux process integration; ELF64 little-endian x86-64 System V/Linux executable
  or shared object. Headers cannot establish function-specific ABI overrides.
- At most 256 unambiguous file-backed regions and 16 MiB mapped bytes. Read-only
  comes from ELF flags. BSS is not invented, relocations/import resolution are not
  applied, and other formats/architectures report unsupported input.
- Requests time out after 20 seconds. Diagnostic messages explain failed or unsupported requests.
- Cancel, closing the binary, patching, or selecting another function clears the previous result. You can run Decompile again for the new selection.
- The worker runs with resource limits, but remains a trusted executable. Only configure a worker you trust.
- Pseudocode is limited to 1 MiB / 100,000 tokens; native output to 16 MiB and
  stderr to 64 KiB. Display pages contain 120 lines. Very long lines and dense token layouts may slow down the view.
