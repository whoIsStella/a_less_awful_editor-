# Native decompiler integration — 2026-10-01

This implements an optional native pseudocode view on the existing workbench.
It is one bounded part of the full systems capability contract, not Ghidra parity.

## Implemented contracts

`crates/systems-core/src/memory.rs` exposes checked borrowed memory ranges, rejects
virtual overlap/file aliases and caps regions/bytes. The initial decompiler gate
requires System V/Linux ELF64 little-endian x86-64 executable/shared-object headers.
Read-only flags come from ELF metadata; zero-fill bytes and relocations are not
invented. Function-specific ABI overrides remain unknowable from those headers.

`crates/ui/src/decompiler.rs` captures the mapped snapshot, invokes an explicitly
configured absolute worker path off the UI thread, and validates protocol/backend
revision, entry, UTF-8 token ranges/text, diagnostics and recovered instruction
addresses. Linux nonblocking pipes, output limits, a private temporary directory,
minimal environment, process-group termination, direct-child reaping, resource
limits and a wall deadline bound the worker lifecycle. This is not a security
sandbox against a malicious configured executable that escapes its process group.

`crates/ui/src/systems_decompiler.rs` supplies the Pseudocode tab, Decompile/Cancel,
Copy code, 120-line pages and clickable address tokens. Function/location changes,
patch/history refresh and document drop invalidate work. Snapshot revision and
request generation checks reject late results. Large function instruction vectors
are retained through an Arc analysis snapshot rather than copied on the UI thread.

## Local evidence

The expanded workspace suite passed 90 tests with the actual pinned native worker
configured, including these newly exercised checks:

- Exact borrowed bytes, read-only flags, region/byte bounds, unsupported ABI and
  overlapping/aliased mapping rejection.
- Strict UTF-8/token identity, wrong backend revision, wrong entry, overlapping
  tokens and non-instruction address rejection.
- Worker output flooding, timeout, cancellation, failure exit and an inherited
  output pipe held by a descendant. Cleanup kills the request process group and
  waits for its direct child; this is not proof of reaping every descendant.
- Actual native pseudocode and instruction-token mapping for controlled bytes.
- Actual worker through the GPUI state: results retain a shared instruction
  location, cancellation suppresses publication, changed bytes clear the old
  result, and rerunning after the patch yields `+ 2` instead of `+ 1`.

Commands (the two native acceptance tests explicitly skip if the test worker
variable is absent):

```sh
ALE_DECOMPILER_TEST=/tmp/ale-ghidra-native-lhtf818e/ale-native-decompiler cargo test --locked --workspace
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p a-less-awful-editor
python3 scripts/check-editor-core-boundary.py
git diff --check
```

The final rerun after removing the UI-thread function clone passed all 90 tests
with both real-native tests exercised, and passed formatting, workspace check,
Clippy, application linking and both pure-core boundaries. No fixture executable
was run, no dependencies installed, and no lockfile update was needed here.

## Unverified and remaining

The newly integrated pseudocode renderer has **not** been exercised in a real
native window in this timed continuation. GPUI state tests are not native visual
acceptance. Rendering, token hit targets, font/zoom behavior, long individual lines,
dense tokens, full byte/source round trips and approved native close remain checks
for the next run. The earlier native graph run is documented separately.

Other executable formats/processors, zero-fill/relocations/import modeling, custom
calling conventions and persistent editable type/name metadata remain open.
The pinned external cache was reused; it is not a fresh reproducible backend build.
Remote CI and the full systems capability contract remain unverified/incomplete.

Launch setup is documented in `tools/native-decompiler/README.md`. No worker is
bundled or automatically downloaded by application startup.
