# Paused analysis continuation — 2026-09-27

The user requested a stopping point during the next analysis milestone. The
last verified runnable checkpoint is `4e9ffe3` on `codex/systems-workbench`:
76 workspace tests, formatting, check, Clippy, linked application, pure-core
boundary, and native export/reopen/close/compact-layout acceptance passed there.

This branch preserves **unfinished work**, not an accepted application build.
Do not merge it or interpret the new API/UI as working features. The user's full
goal remains in `docs/systems-capabilities.md`; the latest goal attachment repeats
the native Rust/GPUI, local-first and safe-persistence constraints. The latest
instruction is to pause, not to complete the entire capability contract now.

## Saved work

- `crates/systems-core/src/analysis.rs` and reexports: initial bounded recursive
  x86 function/control-flow/reference analysis. Formatting ran; locked Cargo
  check stopped before compilation because the new UI dependency needs a deliberate
  lockfile update. No new analysis fixtures ran. Block construction and reference
  deduplication contain quadratic scans; overlap/limit/cancellation behavior needs
  audit and tests before use.
- `crates/ui/src/systems_analysis.rs` and `systems.rs` hooks: draft background
  analysis state, function sidebar, control-flow graph and reference rendering.
  The sidebar/tab/patch invalidation integration is incomplete. This draft has
  not been compiled or exercised. Do not expose it as an accepted feature.
- `crates/ui/Cargo.toml`: proposed direct `serde_json = "1"` dependency; not yet
  resolved into the application lockfile. Retain the existing lockfile and update
  it deliberately when integration resumes, then use `--locked`.
- The Rust decompiler runner was designed but not written before stopping. The
  proposed interface passes an immutable image, revision and function address
  to an explicitly configured `ALE_DECOMPILER` worker; it returns pseudocode and
  UTF-8 token ranges with native instruction addresses. Implement bounded pipes,
  process-group timeout/cancellation/reaping, protocol/version validation and
  fail-closed ELF x86-64 GCC ABI checks before connecting it. A pure-core checked
  mapped-range accessor is still needed; do not infer mappings from section names.
- `tools/native-decompiler/{worker.cc,worker.py,build.py,test_worker.py}`: opt-in
  native Ghidra C++ driver, JSON adapter and external-cache build. No Java or
  target execution. Native sources remain pinned to
  `c4273522017788fb67c30058ffd5bbdf291fcc40`.

## Native experiment evidence

`python3 tools/native-decompiler/build.py --work-dir /tmp/ale-ghidra-native-lhtf818e`
completed with status 0 using the existing pinned cache and verified the version
response. A direct typed fixture produced `uint4 add_one(uint4 value)` returning
`value + 1`; native operation references mapped the addition to `0x1000` and
return to `0x1003`. The temporary evidence file is
`/tmp/ale-ghidra-native-lhtf818e/worker-simple-result.json`.

The conditional/call acceptance script is written but **not run**. Worker resource
limits, protocol error handling, cancellation, complete instruction/address
mapping, and Rust/UI integration remain unverified. Worker/runner output limits
also need alignment. Cached artifacts are development evidence, not a shipped
runtime, security sandbox, or reproducible-build attestation.

## Resume deliberately

Inspect both branch heads and status before switching. Audit the pure analysis
implementation and add controlled fixtures; finish the mapped-memory API and
bounded worker integration; then integrate flow/decompiled views with stale-result
invalidation and location preservation. Rerun the full locked validation surface
and native acceptance. Do not continue automatically while the user has asked to
pause. Preserve the working UI checkpoint, the lockfile and `target/`.
