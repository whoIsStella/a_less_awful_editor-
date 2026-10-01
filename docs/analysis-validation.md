# Analysis integration validation — 2026-10-01

This continuation implements function candidates, control-flow graphs and code
references. It does not complete the full systems capability contract or connect
the native decompiler to the IDE.

## Changes and causal checks

- Replaced repeated whole-function scans during block construction with indexed
  leader membership, and linear reference deduplication with an ordered set.
  No speed claim or benchmark result is implied.
- Connected the Flow tab and function/symbol navigator. References use a selected
  function offset set rather than rescanning its instructions for every reference.
- Applying a patch now invalidates/rebuilds analysis, as undo/redo already do.
  GPUI regression checks cover cancelled completion, analysis replacement after
  patching, undo recovery, and retained source/instruction/interior-byte locations.
- Nine pure fixtures cover diamonds, backward branches, calls, indirect jumps,
  overlapping instructions/mappings, traversal limits, invalid decoding,
  cancellation and unsupported interpretation. The initial diamond test expected
  six instructions but its bytes contain five reachable instructions; the count
  was corrected while preserving the block/edge assertions.
- The existing independent GNU oracle now also compares recursive recovery with
  `objdump` instruction bytes/addresses and branch targets on compiled code.

The deliberate offline lockfile update adds the already-resolved `serde_json`
dependency to the UI package for the saved worker-integration draft. No package
version changed; subsequent Cargo checks use `--locked`.

## Automated validation

Commands run:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p a-less-awful-editor
python3 scripts/check-editor-core-boundary.py
git diff --check
python3 tools/native-decompiler/test_worker.py --worker /tmp/ale-ghidra-native-lhtf818e/ale-native-decompiler
```

Format, workspace check, Clippy, application linking, and both GPUI-free dependency
boundaries passed. The final workspace rerun passed all 85 tests, including the
added UI patch/undo and cancellation assertions: 17 editor-core, 4 editor-view,
25 systems-core, one independent GNU oracle and 38 UI/I/O tests. Local validation
is not remote CI evidence.

The existing opt-in native worker passed typed pseudocode, conditionals, calls,
UTF-8 token intervals, mapped instruction addresses, inferred signatures and
recoverable invalid-request cases. GNU `objdump` independently confirmed call
mapping at `0x401113` and `0x40111e`. Fixtures were compiled but never executed.
This uses an existing pinned native cache, not a fresh reproducible backend build.

## Native acceptance and limitations

Built application on isolated Xvfb `:103`, Marco, GTK portal and Mesa lavapipe,
using a temporary compiled ELF under `/tmp/ale-flow-accept-1q3qh8nm`:

- Native picker opened the temporary binary, discovered ten function candidates
  and 17 references; selecting `choose` rendered six control-flow blocks.
- Selecting instruction `0x401113` in the graph and switching to Bytes retained
  exact file offset `0x1113`.
- Preview/apply changed `b8 05 00 00 00` to `b8 06 00 00 00`; the graph rebuilt
  and showed `mov eax,6`. Undo restored `mov eax,5` and the clean marker; redo
  restored `mov eax,6` and the dirty marker.
- References showed the conditional/jump edges and incoming call. Clicking target
  `0x40111d` opened Assembly at that address. Resize to 800x600 retained location.
- Native close displayed the binary export/discard/cancel guard. Before the
  cancellation/approved-close check, Xvfb exited and the private D-Bus session
  terminated. The two isolated test application processes were explicitly stopped.
  Normal approved-close exit is therefore **not verified in this run**.

The default Vulkan driver first failed with missing DRI3/PlatformNotSupported;
explicit `/usr/share/vulkan/icd.d/lvp_icd.json` allowed launch. Portal initialization
was delayed and emitted service/FUSE warnings. None proves a target-desktop
regression or passing hardware-GPU/Wayland behavior. No global settings or system
packages were changed. Screenshot: `screenshots/workbench-flow.png`.

## Remaining work

Native decompiler process isolation, cancellation/reaping, bounded I/O and response
validation, checked mapped-memory snapshots, ABI gating, and snapshot-bound
pseudocode/token navigation remain unfinished. Graph routing can overlap, large
function lists and analysis selection still need scale testing, and candidate
membership does not resolve shared-tail ownership or indirect control flow.
The broader capability contract, cross-platform/IME acceptance and remote CI
remain open. The next integration step is the checked snapshot and bounded
native worker runner, followed by pseudocode UI and end-to-end mapping tests.
