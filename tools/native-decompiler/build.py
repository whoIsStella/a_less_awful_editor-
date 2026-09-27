#!/usr/bin/env python3
"""Opt-in build of the pinned native worker in an explicit external cache.

Never installs dependencies or modifies global configuration. The existing
source-verification experiment fetches only pinned native and x86 sources.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

REVISION = "c4273522017788fb67c30058ffd5bbdf291fcc40"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True, help="External pinned-source/build cache")
    parser.add_argument("--jobs", type=int, choices=range(1, 9), default=3)
    parser.add_argument("--build-timeout", type=int, default=900)
    args = parser.parse_args()
    if not 1 <= args.build_timeout <= 3600:
        parser.error("--build-timeout must be between 1 and 3600 seconds")
    directory = args.work_dir.expanduser().resolve()
    source = Path(__file__).resolve().parent
    repository = source.parents[1]
    try:
        # This verifies the pinned source manifest, prepares/reuses native
        # objects and x86 SLA, then exercises a tiny independently checked fixture.
        subprocess.run([sys.executable, str(repository / "scripts/verify-native-decompiler.py"),
            "--work-dir", str(directory), "--jobs", str(args.jobs),
            "--build-timeout", str(args.build_timeout)], check=True)
        cpp = directory / "Ghidra/Features/Decompiler/src/decompile/cpp"
        languages = directory / "Ghidra/Processors/x86/data/languages"
        objects = sorted(path for path in (cpp / "com_opt").glob("*.o") if path.name != "consolemain.o")
        if not objects or not (cpp / "com_opt/libdecomp.o").is_file():
            raise RuntimeError("Native library objects are absent; use a complete cache from the source experiment")
        compiler = shutil.which("c++")
        if not compiler:
            raise RuntimeError("Missing C++ compiler; no tools will be installed")
        backend = directory / "ale-native-decompiler-backend"
        # Objects are reused from the pinned build; this is not an attestation
        # that a user-writable cache has not been tampered with.
        digest = hashlib.sha256((source / "worker.cc").read_bytes())
        for path in objects:
            digest.update(path.name.encode())
            digest.update(hashlib.sha256(path.read_bytes()).digest())
        fingerprint = digest.hexdigest()
        stamp = directory / "ale-native-worker-build.json"
        previous = json.loads(stamp.read_text()) if stamp.exists() else {}
        if not backend.is_file() or previous.get("fingerprint") != fingerprint:
            temporary = directory / "ale-native-decompiler-backend.new"
            with (directory / "worker-build.log").open("w") as log:
                subprocess.run([compiler, "-std=c++11", "-O1", "-I" + str(cpp), str(source / "worker.cc"),
                    *map(str, objects), "-lz", "-o", str(temporary)], stdout=log, stderr=subprocess.STDOUT,
                    check=True, timeout=min(args.build_timeout, 180))
            temporary.replace(backend)
            stamp.write_text(json.dumps({"revision": REVISION, "fingerprint": fingerprint,
                "backend_sha256": hashlib.sha256(backend.read_bytes()).hexdigest()}, indent=2) + "\n")
        launcher = directory / "ale-native-decompiler"
        # A generated Python launcher uses argv, never shell interpolation.
        launcher.write_text("#!" + sys.executable + "\nimport os, sys\nos.execv(" + repr(sys.executable) + ", [" +
            repr(sys.executable) + ", " + repr(str(source / "worker.py")) + ", '--backend', " +
            repr(str(backend)) + ", '--languages', " + repr(str(languages)) + "] + sys.argv[1:])\n")
        launcher.chmod(0o755)
        subprocess.run([str(launcher), "--version"], check=True, timeout=10)
        print(f"Worker ready: {launcher}\nSet ALE_DECOMPILER to that absolute path. Nothing was installed.")
        return 0
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"Native worker build failed: {error}\nInspect worker-build.log in the work directory.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
